mod capture;
mod dictation;
mod paste;
mod pill;
mod transport;

use anyhow::{Context, Result};
use capture::{Recorder, Session};
use dictation::{Dictation, PhaseEvent};
use diktafon_protocol::{socket_path, Msg, SessionConfig};
use gpui::{Entity, Global};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;
use transport::DaemonClient;

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
fn daemon_bin() -> Option<PathBuf> {
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

/// App-scoped GPUI state (cadence's AppServices pattern); keeps the entities
/// alive for windows to consume later.
struct AppServices {
    #[expect(dead_code, reason = "keeps the entity alive; windows receive their own clones")]
    dictation: Entity<Dictation>,
}

impl Global for AppServices {}

fn main() -> Result<()> {
    let (phase_tx, phase_rx) = futures::channel::mpsc::unbounded::<PhaseEvent>();
    let daemon = DaemonClient::spawn(socket_path(), daemon_bin(), Some(phase_tx.clone()));

    if let Some(text) = std::env::args().nth(1) {
        daemon.chunk_tx.send(Msg::Flush)?;
        daemon.finish().context("daemon roundtrip failed")?;
        println!("Pasting in 3s, focus a text field...");
        thread::sleep(std::time::Duration::from_secs(3));
        return paste::insert(&text);
    }

    let levels: capture::LevelBars = Default::default();
    let recorder = Recorder::new(ensure_vad_model()?, levels.clone())?;
    println!("Mic: {}", recorder.describe());

    let manager = GlobalHotKeyManager::new().context("registering global hotkey manager")?;
    manager.register(HotKey::new(Some(Modifiers::ALT), Code::Space))?;

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
            step(PhaseEvent::RecordingStarted, 4);
            step(PhaseEvent::RecordingStopped, 3);
            step(PhaseEvent::PolishingStarted, 3);
            step(PhaseEvent::SessionEnded { error: None }, 0);
        });
    }

    let (event_tx, event_rx) = mpsc::channel::<HotKeyState>();
    thread::spawn(move || control_loop(recorder, daemon, event_rx, phase_tx));

    let receiver = GlobalHotKeyEvent::receiver();
    thread::spawn(move || {
        while let Ok(event) = receiver.recv() {
            let _ = event_tx.send(event.state);
        }
    });

    // global-hotkey installs its Carbon handler on the application event
    // target, which GPUI's NSApp run loop dispatches (a bare CFRunLoop would
    // not). Explicit quit mode keeps the windowless app alive.
    gpui_platform::application()
        .with_quit_mode(gpui::QuitMode::Explicit)
        .run(move |cx| {
            hide_from_dock();
            let dictation = Dictation::spawn(cx, phase_rx);
            cx.observe(&dictation, |dictation, cx| {
                println!("[phase] {:?}", dictation.read(cx).phase);
            })
            .detach();
            pill::manage(cx, dictation.clone(), levels);
            cx.set_global(AppServices { dictation });
            println!("Ready. Hold Option+Space to dictate, release to paste.");
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

fn control_loop(
    recorder: Recorder,
    daemon: DaemonClient,
    events: mpsc::Receiver<HotKeyState>,
    phases: futures::channel::mpsc::UnboundedSender<PhaseEvent>,
) {
    let mut session: Option<Session> = None;
    for state in events {
        match state {
            HotKeyState::Pressed => {
                if session.is_none() {
                    let _ = daemon.chunk_tx.send(Msg::Start(SessionConfig::default()));
                    match recorder.start(daemon.chunk_tx.clone()) {
                        Ok(s) => {
                            println!("recording...");
                            session = Some(s);
                            let _ = phases.unbounded_send(PhaseEvent::RecordingStarted);
                        }
                        Err(e) => eprintln!("failed to start recording: {e}"),
                    }
                }
            }
            HotKeyState::Released => {
                if let Some(s) = session.take() {
                    let stopped_at = Instant::now();
                    s.stop();
                    let _ = phases.unbounded_send(PhaseEvent::RecordingStopped);
                    let error = match daemon.finish() {
                        Ok(text) if text.is_empty() => {
                            println!("(no speech)");
                            None
                        }
                        Ok(text) => {
                            println!(">>> {text}");
                            if let Err(e) = paste::insert(&text) {
                                eprintln!("paste failed (Accessibility permission?): {e}");
                            }
                            println!("stop-to-paste: {:.2?}", stopped_at.elapsed());
                            None
                        }
                        Err(e) => {
                            eprintln!("inference error: {e}");
                            Some(format!("{e:#}"))
                        }
                    };
                    let _ = phases.unbounded_send(PhaseEvent::SessionEnded { error });
                }
            }
        }
    }
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
