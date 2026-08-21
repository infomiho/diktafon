mod capture;
mod paste;

use anyhow::{Context, Result};
use capture::{Recorder, Session};
use diktafon_protocol::Msg;
use diktafond::Inference;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

// global-hotkey installs its Carbon handler on the application event target,
// which only an application event loop dispatches; a bare CFRunLoop does not.
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn RunApplicationEventLoop();
}

fn models_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME not set"))
        .join("Library/Application Support/diktafon/models")
}

fn main() -> Result<()> {
    println!("Loading models...");
    let load_start = Instant::now();
    let inference = Inference::spawn(&models_dir())?;
    println!("Models loaded in {:.2?}", load_start.elapsed());

    if let Some(text) = std::env::args().nth(1) {
        println!("Polishing and pasting in 3s, focus a text field...");
        thread::sleep(std::time::Duration::from_secs(3));
        inference.chunk_tx.send(Msg::Flush)?;
        inference.finish()?;
        return paste::insert(&text);
    }

    let recorder = Recorder::new()?;
    println!("Mic: {}", recorder.describe());

    let manager = GlobalHotKeyManager::new().context("registering global hotkey manager")?;
    manager.register(HotKey::new(Some(Modifiers::ALT), Code::Space))?;

    let (event_tx, event_rx) = mpsc::channel::<HotKeyState>();
    thread::spawn(move || control_loop(recorder, inference, event_rx));

    let receiver = GlobalHotKeyEvent::receiver();
    thread::spawn(move || {
        while let Ok(event) = receiver.recv() {
            let _ = event_tx.send(event.state);
        }
    });

    println!("Ready. Hold Option+Space to dictate, release to paste.");
    unsafe { RunApplicationEventLoop() };
    Ok(())
}

fn control_loop(recorder: Recorder, inference: Inference, events: mpsc::Receiver<HotKeyState>) {
    let mut session: Option<Session> = None;
    for state in events {
        match state {
            HotKeyState::Pressed => {
                if session.is_none() {
                    match recorder.start(inference.chunk_tx.clone()) {
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
                    match inference.finish() {
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
