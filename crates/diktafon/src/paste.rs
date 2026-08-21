use anyhow::Result;
use arboard::{Clipboard, ImageData};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread;
use std::time::Duration;

/// What the clipboard held before we borrowed it.
enum Saved {
    Text(String),
    Image(ImageData<'static>),
    Empty,
}

/// Insert text into the frontmost app: stash the clipboard, set the text,
/// synthesize Cmd+V using the layout-resolved keycode, then restore the
/// clipboard (even when the keystroke failed). Requires the Accessibility
/// permission for the synthesized keystroke.
pub fn insert(text: &str, v_keycode: u16) -> Result<()> {
    let mut clipboard = Clipboard::new()?;
    let previous = saved(&mut clipboard);
    clipboard.set_text(text)?;
    thread::sleep(Duration::from_millis(50));

    let pasted = press_cmd_v(v_keycode);

    // Give the frontmost app time to read the clipboard before restoring.
    thread::sleep(Duration::from_millis(150));
    restore(&mut clipboard, previous);
    pasted
}

fn saved(clipboard: &mut Clipboard) -> Saved {
    if let Ok(text) = clipboard.get_text() {
        return Saved::Text(text);
    }
    // Probe images only when there is no text; the common case skips the copy.
    if let Ok(image) = clipboard.get_image() {
        return Saved::Image(image.to_owned_img());
    }
    Saved::Empty
}

fn restore(clipboard: &mut Clipboard, previous: Saved) {
    let result = match previous {
        Saved::Text(text) => clipboard.set_text(text),
        Saved::Image(image) => clipboard.set_image(image),
        // The clipboard read as empty; leaving our text would surprise a
        // later paste. Trade-off: arboard only sees text and images, so
        // copied files or custom-type content also read as empty and get
        // cleared here.
        Saved::Empty => clipboard.clear(),
    };
    if let Err(e) = result {
        eprintln!("restoring the clipboard failed: {e}");
    }
}

fn press_cmd_v(v_keycode: u16) -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(Key::Meta, Direction::Press)?;
    let click = enigo.key(Key::Other(v_keycode.into()), Direction::Click);
    // Hold Meta briefly: apps that poll modifier state drop too-fast chords.
    thread::sleep(Duration::from_millis(100));
    let release = enigo.key(Key::Meta, Direction::Release);
    click?;
    release?;
    Ok(())
}
