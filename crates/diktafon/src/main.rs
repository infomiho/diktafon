mod capture;
mod paste;
mod transport;

use anyhow::{Context, Result};
use capture::{Recorder, Session};
use diktafon_protocol::{socket_path, Msg, SessionConfig};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;
use transport::DaemonClient;

fn vad_model_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME not set"))
        .join("Library/Application Support/diktafon/models/silero_vad_v4.onnx")
}

fn main() -> Result<()> {
    let daemon = DaemonClient::spawn(socket_path());

    if let Some(text) = std::env::args().nth(1) {
        daemon.chunk_tx.send(Msg::Flush)?;
        daemon.finish().context("daemon roundtrip failed")?;
        println!("Pasting in 3s, focus a text field...");
        thread::sleep(std::time::Duration::from_secs(3));
        return paste::insert(&text);
    }

    let recorder = Recorder::new(vad_model_path())?;
    println!("Mic: {}", recorder.describe());

    let manager = GlobalHotKeyManager::new().context("registering global hotkey manager")?;
    manager.register(HotKey::new(Some(Modifiers::ALT), Code::Space))?;

    let (event_tx, event_rx) = mpsc::channel::<HotKeyState>();
    thread::spawn(move || control_loop(recorder, daemon, event_rx));

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
        .run(|_cx| {
            hide_from_dock();
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

fn control_loop(recorder: Recorder, daemon: DaemonClient, events: mpsc::Receiver<HotKeyState>) {
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
                        }
                        Err(e) => eprintln!("failed to start recording: {e}"),
                    }
                }
            }
            HotKeyState::Released => {
                if let Some(s) = session.take() {
                    let stopped_at = Instant::now();
                    s.stop();
                    match daemon.finish() {
                        Ok(text) if text.is_empty() => println!("(no speech)"),
                        Ok(text) => {
                            println!(">>> {text}");
                            if let Err(e) = paste::insert(&text) {
                                eprintln!("paste failed (Accessibility permission?): {e}");
                            }
                            println!("stop-to-paste: {:.2?}", stopped_at.elapsed());
                        }
                        Err(e) => eprintln!("inference error: {e}"),
                    }
                }
            }
        }
    }
}
