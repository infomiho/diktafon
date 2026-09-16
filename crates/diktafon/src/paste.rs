use anyhow::Result;
use arboard::{Clipboard, ImageData};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc2::rc::autoreleasepool;
use std::cell::RefCell;
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
    insert_with(text, || press_cmd_v(v_keycode))
}

fn insert_with(text: &str, paste: impl FnOnce() -> Result<()>) -> Result<()> {
    // Clipboard image restoration creates autoreleased AppKit objects on the
    // long-lived dictation thread. Drain them after each paste, including errors.
    autoreleasepool(|_| {
        let mut clipboard = Clipboard::new()?;
        let previous = saved(&mut clipboard);
        clipboard.set_text(text)?;
        thread::sleep(Duration::from_millis(50));

        let pasted = paste();

        // Give the frontmost app time to read the clipboard before restoring.
        thread::sleep(Duration::from_millis(150));
        restore(&mut clipboard, previous);
        pasted
    })
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

thread_local! {
    static KEYBOARD_SOURCE: RefCell<Option<CGEventSource>> = const { RefCell::new(None) };
}

fn keyboard_source() -> Result<CGEventSource> {
    KEYBOARD_SOURCE.with(|slot| {
        let mut source = slot.borrow_mut();
        if source.is_none() {
            *source = Some(
                CGEventSource::new(CGEventSourceStateID::Private)
                    .map_err(|_| anyhow::anyhow!("creating keyboard event source failed"))?,
            );
        }
        Ok(source.as_ref().unwrap().clone())
    })
}

fn paste_events(v_keycode: u16) -> Result<[CGEvent; 4]> {
    let source = keyboard_source()?;
    let event = |keycode, down, command| -> Result<CGEvent> {
        let event = CGEvent::new_keyboard_event(source.clone(), keycode, down)
            .map_err(|_| anyhow::anyhow!("creating paste keyboard event failed"))?;
        event.set_flags(if command {
            CGEventFlags::CGEventFlagCommand
        } else {
            CGEventFlags::empty()
        });
        Ok(event)
    };
    Ok([
        event(55, true, true)?,
        event(v_keycode, true, true)?,
        event(v_keycode, false, true)?,
        event(55, false, false)?,
    ])
}

fn press_cmd_v(v_keycode: u16) -> Result<()> {
    anyhow::ensure!(
        crate::permissions::accessibility_granted(),
        "Accessibility permission missing"
    );
    let events = paste_events(v_keycode)?;
    for event in &events[..3] {
        event.post(CGEventTapLocation::HID);
    }
    thread::sleep(Duration::from_millis(100));
    events[3].post(CGEventTapLocation::HID);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardItem};
    use objc2_foundation::NSArray;

    struct ClipboardSnapshot(Vec<Retained<NSPasteboardItem>>);

    impl ClipboardSnapshot {
        fn capture() -> Self {
            autoreleasepool(|_| {
                let board = NSPasteboard::generalPasteboard();
                let mut copies = Vec::new();
                if let Some(items) = board.pasteboardItems() {
                    for item in items {
                        let copy = NSPasteboardItem::new();
                        for kind in item.types() {
                            let data = item.dataForType(&kind).expect("read clipboard format");
                            assert!(copy.setData_forType(&data, &kind));
                        }
                        copies.push(copy);
                    }
                }
                Self(copies)
            })
        }
    }

    impl Drop for ClipboardSnapshot {
        fn drop(&mut self) {
            autoreleasepool(|_| {
                let board = NSPasteboard::generalPasteboard();
                let objects: Vec<_> = self
                    .0
                    .iter()
                    .cloned()
                    .map(ProtocolObject::from_retained)
                    .collect();
                board.clearContents();
                if !objects.is_empty() {
                    assert!(board.writeObjects(&NSArray::from_retained_slice(&objects)));
                }
            });
        }
    }

    fn live_heap_bytes() -> usize {
        // A null zone asks libmalloc to sum live allocations across all zones.
        let mut stats: libc::malloc_statistics_t = unsafe { std::mem::zeroed() };
        unsafe { libc::malloc_zone_statistics(std::ptr::null_mut(), &raw mut stats) };
        stats.size_in_use
    }

    fn footprint_bytes() -> usize {
        let output = std::process::Command::new("footprint")
            .args(["-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        let mut value = text.split("Footprint:").nth(1).unwrap().split_whitespace();
        let size: usize = value.next().unwrap().parse().unwrap();
        size * match value.next().unwrap() {
            "KB" => 1024,
            "MB" => 1024 * 1024,
            unit => panic!("unexpected footprint unit: {unit}"),
        }
    }

    #[test]
    #[ignore = "requires macOS WindowServer and footprint, run alone"]
    fn repeated_keyboard_sessions_have_bounded_memory() {
        thread::spawn(|| {
            for _ in 0..20 {
                autoreleasepool(|_| paste_events(9)).unwrap();
            }
            let before = footprint_bytes();
            for cycle in 0..1000 {
                let keycode = if cycle % 2 == 0 { 9 } else { 6 };
                let events = autoreleasepool(|_| paste_events(keycode)).unwrap();
                use core_graphics::event::{CGEventType, EventField};
                for (event, (key, down, command)) in events.iter().zip([
                    (55, true, true),
                    (keycode, true, true),
                    (keycode, false, true),
                    (55, false, false),
                ]) {
                    assert_eq!(
                        event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE),
                        i64::from(key)
                    );
                    assert_eq!(
                        event.get_type() as u32,
                        if key == 55 {
                            CGEventType::FlagsChanged as u32
                        } else if down {
                            CGEventType::KeyDown as u32
                        } else {
                            CGEventType::KeyUp as u32
                        }
                    );
                    assert_eq!(
                        event.get_flags().contains(CGEventFlags::CGEventFlagCommand),
                        command
                    );
                }
            }
            let after = footprint_bytes();
            eprintln!("keyboard footprint: {before} -> {after}");
            assert!(after.saturating_sub(before) < 4 * 1024 * 1024);
        })
        .join()
        .unwrap();
    }

    #[test]
    #[ignore = "temporarily uses the macOS clipboard, run alone with --ignored --exact"]
    fn repeated_image_pastes_preserve_clipboard_without_memory_growth() {
        thread::spawn(|| {
            autoreleasepool(|_| {
                let _original = ClipboardSnapshot::capture();
                let mut clipboard = Clipboard::new().unwrap();
                let pixels: Vec<u8> = (0..1024 * 1024)
                    .flat_map(|i| [(i % 251) as u8, (i % 127) as u8, (i % 61) as u8, 255])
                    .collect();
                autoreleasepool(|_| {
                    clipboard
                        .set_image(ImageData {
                            width: 1024,
                            height: 1024,
                            bytes: pixels.clone().into(),
                        })
                        .unwrap();
                });
                let paste = || {
                    insert_with("memory regression", || {
                        assert_eq!(Clipboard::new()?.get_text()?, "memory regression");
                        Ok(())
                    })
                    .unwrap();
                    autoreleasepool(|_| {
                        let restored = Clipboard::new().unwrap().get_image().unwrap();
                        assert_eq!((restored.width, restored.height), (1024, 1024));
                        assert!(restored.bytes.as_ref() == pixels, "clipboard image changed");
                    });
                };
                for _ in 0..3 {
                    paste();
                }
                let baseline = live_heap_bytes();
                for _ in 0..12 {
                    paste();
                }
                let after = live_heap_bytes();
                let growth = after.saturating_sub(baseline);
                eprintln!("clipboard live heap: {baseline} -> {after} bytes, growth {growth}");
                assert!(
                    growth < 16 * 1024 * 1024,
                    "repeated clipboard image restores retained {} MiB",
                    growth / (1024 * 1024)
                );
            })
        })
        .join()
        .unwrap();
    }
}
