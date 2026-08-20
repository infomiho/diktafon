use anyhow::Result;
use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread;
use std::time::Duration;

/// Insert text into the frontmost app: stash the clipboard, set the text,
/// synthesize Cmd+V, then restore the clipboard. Requires the Accessibility
/// permission for the synthesized keystroke.
pub fn insert(text: &str) -> Result<()> {
    let mut clipboard = Clipboard::new()?;
    let previous = clipboard.get_text().ok();
    clipboard.set_text(text)?;
    thread::sleep(Duration::from_millis(50));

    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(Key::Meta, Direction::Press)?;
    enigo.key(Key::Unicode('v'), Direction::Click)?;
    enigo.key(Key::Meta, Direction::Release)?;

    thread::sleep(Duration::from_millis(150));
    if let Some(previous) = previous {
        clipboard.set_text(previous)?;
    }
    Ok(())
}
