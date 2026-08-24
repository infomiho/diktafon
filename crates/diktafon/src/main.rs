mod assets;
mod autostart;
mod bench;
mod capture;
mod config;
mod dictation;
mod keymap;
mod mark;
mod paste;
mod permissions;
mod pill;
mod settings;
mod sounds;
mod stats;
mod statusbar;
mod theme;
mod transport;

use anyhow::{Context, Result};
use capture::{Recorder, Session};
use dictation::{Dictation, PhaseEvent};
use diktafon_protocol::{Msg, socket_path};
use global_hotkey::hotkey::{Code, HotKey};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui::{Entity, Global};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use transport::DaemonClient;

// App-level actions, dispatched from any focused diktafon window (cadence's
// bootstrap pattern): Cmd+Q quits, Cmd+W closes the window that has focus.
gpui::actions!(diktafon, [Quit, CloseWindow]);

/// Ships inside the binary (1.8MB, mirroring Handy bundling the same file) so
/// the client needs no model downloads at all.
const SILERO_VAD: &[u8] = include_bytes!("../resources/silero_vad_v4.onnx");

fn ensure_vad_model() -> Result<PathBuf> {
    let path = diktafon_protocol::models_dir().join("silero_vad_v4.onnx");
    materialize_vad_model(&path)?;
    Ok(path)
}

fn materialize_vad_model(path: &Path) -> Result<()> {
    let up_to_date = std::fs::metadata(path).is_ok_and(|m| m.len() == SILERO_VAD.len() as u64);
    if up_to_date {
        return Ok(());
    }
    let dir = path.parent().context("VAD model path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("onnx.tmp");
    std::fs::write(&tmp, SILERO_VAD)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        // A second client instance may have raced us here; both write
        // identical bytes, so a completed destination means someone won.
        let raced_and_won =
            std::fs::metadata(path).is_ok_and(|m| m.len() == SILERO_VAD.len() as u64);
        if !raced_and_won {
            return Err(e).context("installing the VAD model");
        }
    }
    Ok(())
}

/// The diktafond binary to auto-spawn: `DIKTAFOND_BIN` override, or the one
/// sitting next to this executable (cargo target dir, or the app bundle).
pub(crate) fn daemon_bin() -> Option<PathBuf> {
    if let Some(bin) = std::env::var_os("DIKTAFOND_BIN") {
        return Some(PathBuf::from(bin));
    }
    let sibling = std::env::current_exe().ok()?.parent()?.join("diktafond");
    if !sibling.exists() {
        eprintln!(
            "diktafond not found at {}; auto-spawn disabled, start the daemon manually",
            sibling.display()
        );
        return None;
    }
    Some(sibling)
}

/// Swaps the push-to-talk registration live: main thread only, where the
/// Carbon manager lives. The control loop follows via the shared id.
struct HotkeyRebind {
    manager: std::rc::Rc<GlobalHotKeyManager>,
    current: std::cell::Cell<HotKey>,
    record_id: Arc<AtomicU32>,
    /// Makes suspend/resume idempotent: a double register would leak a
    /// Carbon registration that unregister can never remove.
    suspended: std::cell::Cell<bool>,
}

impl HotkeyRebind {
    /// Unregister the chord while a new one is recorded, so pressing the
    /// current hotkey during capture cannot start a dictation.
    fn suspend(&self) {
        if self.suspended.replace(true) {
            return;
        }
        if let Err(e) = self.manager.unregister(self.current.get()) {
            eprintln!("suspending hotkey failed: {e}");
        }
    }

    /// Re-register the unchanged chord after a cancelled capture.
    fn resume(&self) {
        if !self.suspended.replace(false) {
            return;
        }
        if let Err(e) = self.manager.register(self.current.get()) {
            eprintln!("resuming hotkey failed: {e}");
        }
    }

    /// Register the captured chord (the old one is suspended). On failure the
    /// old chord stays suspended and capture continues; the eventual capture
    /// exit resumes it, so the app is never left keyless.
    fn commit(&self, new: HotKey) -> anyhow::Result<()> {
        self.manager.register(new)?;
        self.current.set(new);
        self.record_id.store(new.id(), Ordering::Relaxed);
        self.suspended.set(false);
        Ok(())
    }
}

/// App-scoped gpui state; windows reach shared services through it.
struct AppServices {
    dictation: Entity<Dictation>,
    hotkey: HotkeyRebind,
}

impl Global for AppServices {}

fn main() -> Result<()> {
    // Modes that must not touch (or auto-spawn) the daemon come first.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--transcribe-file") {
        return bench::transcribe_file(&args[1..]);
    }
    if args.first().is_some_and(|a| a == "--autostart") {
        return autostart::run(args.get(1).map(String::as_str).unwrap_or("status"));
    }
    if args.first().is_some_and(|a| a == "--gen-mark") {
        return Ok(mark::write_assets()?);
    }
    if args.first().is_some_and(|a| a == "--stats") {
        return stats::report();
    }
    if args.first().is_some_and(|a| a == "--test-sound") {
        return test_sound();
    }

    let (phase_tx, phase_rx) = futures::channel::mpsc::unbounded::<PhaseEvent>();
    let daemon = DaemonClient::spawn(socket_path(), daemon_bin(), Some(phase_tx.clone()));

    if let Some(text) = args.first() {
        let text = text.clone();
        daemon.chunk_tx.send(Msg::Flush)?;
        daemon.finish().context("daemon roundtrip failed")?;
        println!("Pasting in 3s, focus a text field...");
        thread::sleep(std::time::Duration::from_secs(3));
        // main() is the main thread here, as the TIS scan requires.
        return paste::insert(&text, keymap::v_keycode());
    }

    let levels: capture::LevelBars = Default::default();
    let recorder = Recorder::new(ensure_vad_model()?, levels.clone())?;
    println!("Mic: {}", recorder.describe());

    let session_settings = Arc::new(std::sync::Mutex::new(config::SessionSettings::load()));

    let manager =
        std::rc::Rc::new(GlobalHotKeyManager::new().context("registering global hotkey manager")?);
    let record_key = session_settings.lock().unwrap().hotkey();
    // Registered only while a session is live, so Escape works normally
    // otherwise; see the phase observer below.
    let escape_key = HotKey::new(None, Code::Escape);
    manager.register(record_key)?;

    // Freeze the pill in one phase for visual inspection:
    // `DIKTAFON_PILL_HOLD=transcribing diktafon`.
    if let Ok(hold) = std::env::var("DIKTAFON_PILL_HOLD") {
        let demo = phase_tx.clone();
        thread::spawn(move || {
            thread::sleep(std::time::Duration::from_secs(2));
            let _ = demo.unbounded_send(PhaseEvent::RecordingArmed);
            let event = match hold.as_str() {
                "arming" => None,
                "recording" => Some(PhaseEvent::RecordingStarted),
                "polishing" => Some(PhaseEvent::PolishingStarted),
                _ => Some(PhaseEvent::RecordingStopped),
            };
            let _ = demo.unbounded_send(PhaseEvent::RecordingStarted);
            let _ = demo.unbounded_send(PhaseEvent::Partial("penguin enterprises flagship".into()));
            if let Some(event) = event {
                let _ = demo.unbounded_send(event);
            }
        });
    }

    // Scripted phase walk for developing the pill without dictating:
    // `DIKTAFON_PILL_DEMO=1 diktafon`.
    if std::env::var_os("DIKTAFON_PILL_DEMO").is_some() {
        let demo = phase_tx.clone();
        thread::spawn(move || {
            let step = |event, secs| {
                let _ = demo.unbounded_send(event);
                thread::sleep(std::time::Duration::from_secs(secs));
            };
            thread::sleep(std::time::Duration::from_secs(2));
            step(PhaseEvent::RecordingArmed, 1);
            step(PhaseEvent::RecordingStarted, 2);
            step(
                PhaseEvent::Partial("penguin enterprises flagship".into()),
                2,
            );
            step(
                PhaseEvent::Partial("utilizes a proprietary polymer blend".into()),
                2,
            );
            step(PhaseEvent::RecordingStopped, 3);
            step(PhaseEvent::PolishingStarted, 3);
            step(
                PhaseEvent::SessionEnded {
                    error: None,
                    cancelled: false,
                },
                0,
            );
        });
    }

    let (event_tx, event_rx) = mpsc::channel::<GlobalHotKeyEvent>();
    let record_id = Arc::new(AtomicU32::new(record_key.id()));
    let hotkeys = Hotkeys {
        record: record_id.clone(),
        escape: escape_key.id(),
    };
    // Resolved on the main thread (TIS requirement) and refreshed at each
    // session start by the phase observer; the control loop only reads it.
    let v_keycode = Arc::new(AtomicU32::new(keymap::ANSI_V.into()));
    let paste_keycode = v_keycode.clone();
    let loop_settings = session_settings.clone();
    thread::spawn(move || {
        control_loop(
            recorder,
            daemon,
            event_rx,
            phase_tx,
            hotkeys,
            paste_keycode,
            loop_settings,
        )
    });

    let receiver = GlobalHotKeyEvent::receiver();
    thread::spawn(move || {
        while let Ok(event) = receiver.recv() {
            let _ = event_tx.send(event);
        }
    });

    // global-hotkey installs its Carbon handler on the application event
    // target, which GPUI's NSApp run loop dispatches (a bare CFRunLoop would
    // not). Explicit quit mode keeps the windowless app alive.
    gpui_platform::application()
        .with_assets(assets::Assets)
        .with_quit_mode(gpui::QuitMode::Explicit)
        .run(move |cx| {
            hide_from_dock();
            theme::install_fonts(cx);
            gpui_component::init(cx);
            theme::apply_settings_theme(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.bind_keys([
                gpui::KeyBinding::new("cmd-q", Quit, None),
                gpui::KeyBinding::new("cmd-w", CloseWindow, None),
            ]);
            permissions::check_at_launch();
            let dictation = Dictation::spawn(cx, phase_rx);
            let escape_manager = manager.clone();
            // The Carbon hotkey manager lives on this thread; register Escape
            // only while a session could still be cancelled.
            let mut escape_registered = false;
            cx.observe(&dictation, move |dictation, cx| {
                let phase = dictation.read(cx).phase;
                println!("[phase] {phase:?}");
                if phase == dictation::Phase::Arming {
                    // Refresh per session so layout switches are picked up;
                    // must happen on this (main) thread.
                    v_keycode.store(keymap::v_keycode().into(), Ordering::Relaxed);
                }
                let cancellable = matches!(
                    phase,
                    dictation::Phase::Arming | dictation::Phase::Recording
                );
                if cancellable != escape_registered {
                    let result = if cancellable {
                        escape_manager.register(escape_key)
                    } else {
                        escape_manager.unregister(escape_key)
                    };
                    match result {
                        Ok(()) => escape_registered = cancellable,
                        Err(e) => eprintln!("escape hotkey change failed: {e}"),
                    }
                }
            })
            .detach();
            pill::manage(cx, dictation.clone(), levels);
            statusbar::install(cx, &dictation, session_settings.clone());
            cx.set_global(AppServices {
                dictation,
                hotkey: HotkeyRebind {
                    manager,
                    current: std::cell::Cell::new(record_key),
                    record_id,
                    suspended: std::cell::Cell::new(false),
                },
            });
            println!(
                "Ready. Hold {} to dictate, release to paste.",
                session_settings.lock().unwrap().hotkey
            );
        });
    Ok(())
}

/// GPUI forces the Regular activation policy on launch, which gives this
/// windowless app a Dock icon and a Cmd+Tab entry; demote it to a background
/// app. Must run on the main thread after GPUI finished launching.
fn hide_from_dock() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let mtm = MainThreadMarker::new().expect("not on the main thread");
    NSApplication::sharedApplication(mtm)
        .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}

/// How long a mic may take to deliver its first samples; Bluetooth devices
/// can need hundreds of milliseconds.
const MIC_READY_TIMEOUT: Duration = Duration::from_millis(1500);

/// Hotkey ids as seen in `GlobalHotKeyEvent`s.
struct Hotkeys {
    /// Repointed by [`HotkeyRebind`] when the user records a new chord.
    record: Arc<AtomicU32>,
    escape: u32,
}

fn control_loop(
    mut recorder: Recorder,
    daemon: DaemonClient,
    events: mpsc::Receiver<GlobalHotKeyEvent>,
    phases: futures::channel::mpsc::UnboundedSender<PhaseEvent>,
    hotkeys: Hotkeys,
    v_keycode: Arc<AtomicU32>,
    settings: Arc<std::sync::Mutex<config::SessionSettings>>,
) {
    // Created on this thread: the output stream is not Send.
    let sounds = match sounds::Sounds::new() {
        Ok(sounds) => Some(sounds),
        Err(e) => {
            eprintln!("feedback sounds disabled: {e:#}");
            None
        }
    };
    let play = |cue| {
        if settings.lock().unwrap().sound_cues
            && let Some(sounds) = &sounds
        {
            sounds.play(cue);
        }
    };
    let mut session: Option<(Session, SessionTiming)> = None;
    for event in events {
        if event.id == hotkeys.escape {
            if event.state == HotKeyState::Pressed
                && let Some((s, _)) = session.take()
            {
                s.cancel();
                play(sounds::Cue::Cancel);
                println!("cancelled");
                let _ = phases.unbounded_send(PhaseEvent::SessionEnded {
                    error: None,
                    cancelled: true,
                });
            }
            continue;
        }
        if event.id != hotkeys.record.load(Ordering::Relaxed) {
            continue;
        }
        match event.state {
            HotKeyState::Pressed => {
                if session.is_none() {
                    let pressed_at = Instant::now();
                    let _ = daemon
                        .chunk_tx
                        .send(Msg::Start(settings.lock().unwrap().session()));
                    match recorder.start(daemon.chunk_tx.clone()) {
                        Ok(s) => {
                            let _ = phases.unbounded_send(PhaseEvent::RecordingArmed);
                            // Stream::play() returning does not mean samples
                            // flow yet; wait so slow mics don't eat first
                            // words. A queued Released is handled right after.
                            if s.wait_until_live(MIC_READY_TIMEOUT) {
                                play(sounds::Cue::Start);
                                println!("recording...");
                                let timing = SessionTiming {
                                    pressed_at,
                                    mic_ready_ms: pressed_at.elapsed().as_millis() as u64,
                                };
                                session = Some((s, timing));
                                let _ = phases.unbounded_send(PhaseEvent::RecordingStarted);
                            } else {
                                let error = "Microphone unavailable";
                                eprintln!(
                                    "microphone produced no samples; is another app holding it?"
                                );
                                // stop() flushes the (empty) session to the
                                // daemon; consume its result so it cannot be
                                // misdelivered to the next session.
                                s.stop();
                                play(sounds::Cue::Error);
                                recorder.mark_stream_failed();
                                let _ = daemon.finish();
                                let _ = phases.unbounded_send(PhaseEvent::SessionEnded {
                                    error: Some(error.into()),
                                    cancelled: false,
                                });
                            }
                        }
                        Err(e) => eprintln!("failed to start recording: {e}"),
                    }
                }
            }
            HotKeyState::Released => {
                if let Some((s, timing)) = session.take() {
                    let stopped_at = Instant::now();
                    s.stop();
                    let _ = phases.unbounded_send(PhaseEvent::RecordingStopped);
                    // `cancelled` also covers "nothing to paste": the pill
                    // plays its quiet ending, keeping the success bloom to
                    // mean words actually landed.
                    let (error, cancelled) = match daemon.finish() {
                        Ok(text) if text.is_empty() => {
                            println!("(no speech)");
                            (None, true)
                        }
                        Ok(text) => {
                            println!(">>> {text}");
                            let keycode = v_keycode.load(Ordering::Relaxed) as u16;
                            let error = paste::insert(&text, keycode).err().map(|e| {
                                eprintln!("paste failed (Accessibility permission?): {e:#}");
                                "Paste needs Accessibility".to_string()
                            });
                            println!("stop-to-paste: {:.2?}", stopped_at.elapsed());
                            (error, false)
                        }
                        Err(e) => {
                            play(sounds::Cue::Error);
                            eprintln!("inference error: {e:#}");
                            (Some("Transcription failed".to_string()), false)
                        }
                    };
                    let outcome = match (&error, cancelled) {
                        (Some(_), _) => "error",
                        (None, true) => "empty",
                        (None, false) => "pasted",
                    };
                    // Consume the spawn marker either way so an old spawn can
                    // never label a later session cold.
                    let cold_start = daemon
                        .spawned_at
                        .lock()
                        .unwrap()
                        .take()
                        .is_some_and(|at| at >= timing.pressed_at);
                    stats::append(&stats::Timing {
                        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                        mic_ready_ms: timing.mic_ready_ms,
                        recording_secs: ((stopped_at - timing.pressed_at).as_secs_f32()
                            - timing.mic_ready_ms as f32 / 1000.0)
                            .max(0.0),
                        stop_to_paste_ms: stopped_at.elapsed().as_millis() as u64,
                        cold_start,
                        outcome: outcome.into(),
                    });
                    let _ = phases.unbounded_send(PhaseEvent::SessionEnded { error, cancelled });
                }
            }
        }
    }
}

/// `diktafon --test-sound`: play the start cue with the microphone closed,
/// then again while it is open. Opening an input can force a Bluetooth
/// headset into call mode, which degrades playback, so hearing the two cases
/// apart says whether a broken cue is the file, the device, or the mic.
fn test_sound() -> Result<()> {
    // Queried at each stage: a Bluetooth headset switching to call mode
    // changes the output device's own format, which is the mechanism that
    // would wreck a cue played at that moment.
    fn output_format() -> String {
        use cpal::traits::{DeviceTrait, HostTrait};
        let Some(device) = cpal::default_host().default_output_device() else {
            return "no default output".into();
        };
        let name = device.name().unwrap_or_else(|_| "?".into());
        match device.default_output_config() {
            Ok(config) => format!(
                "{name} ({} Hz, {} ch)",
                config.sample_rate().0,
                config.channels()
            ),
            Err(e) => format!("{name} (config unavailable: {e})"),
        }
    }

    let sounds = sounds::Sounds::new().context("opening audio output")?;
    println!("sink opened as: {}", sounds.describe());
    println!("output now:     {}", output_format());

    let pause = || thread::sleep(Duration::from_millis(1500));
    println!("1/3 microphone closed");
    sounds.play(sounds::Cue::Start);
    pause();
    pause();

    let levels: capture::LevelBars = Default::default();
    let mut recorder = Recorder::new(ensure_vad_model()?, levels)?;
    println!("    mic: {}", recorder.describe());
    let (chunk_tx, chunk_rx) = mpsc::channel();
    // The capture thread would otherwise block once nothing drains it.
    thread::spawn(move || while chunk_rx.recv().is_ok() {});
    let session = recorder.start(chunk_tx)?;
    let mic_open = Instant::now();
    session.wait_until_live(MIC_READY_TIMEOUT);
    println!("    mic live after {:?}", mic_open.elapsed());
    // Exactly the dictation path: the cue follows the microphone going live,
    // which is when a shared headset is still switching to call mode.
    println!("2/3 microphone just opened (the real dictation path)");
    let cue_at = Instant::now();
    sounds.play(sounds::Cue::Start);
    println!(
        "    cue played {:?} after mic live, output {}",
        cue_at.elapsed(),
        output_format()
    );
    pause();
    pause();
    session.cancel();

    println!("3/3 microphone closed again");
    pause();
    println!("    output now: {}", output_format());
    sounds.play(sounds::Cue::Start);
    pause();
    pause();
    println!("Which of the three sounded wrong?");
    Ok(())
}

/// Per-session instants for the timings record; lives beside the session so a
/// cancel discards both.
struct SessionTiming {
    pressed_at: Instant,
    mic_ready_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vad_model_materializes_and_repairs() {
        let dir = std::env::temp_dir().join(format!("dkt-vad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("models/silero_vad_v4.onnx");

        materialize_vad_model(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), SILERO_VAD);

        // A wrong-size file (corruption, older version) is replaced.
        std::fs::write(&path, b"junk").unwrap();
        materialize_vad_model(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap().len(), SILERO_VAD.len());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
