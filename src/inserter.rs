//! Types text into the focused app: put it on the clipboard, press ⌘V / Ctrl+V,
//! then put the previous clipboard (text or image) back.

use std::thread::sleep;
use std::time::Duration;

use anyhow::Result;
use arboard::{Clipboard, ImageData};

enum Saved {
    Text(String),
    Image(ImageData<'static>),
    Nothing,
}

pub fn paste_text(text: &str, restore: bool) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let mut cb = Clipboard::new()?;
    let saved = if !restore {
        Saved::Nothing
    } else if let Ok(t) = cb.get_text() {
        Saved::Text(t)
    } else if let Ok(img) = cb.get_image() {
        Saved::Image(img.to_owned_img())
    } else {
        Saved::Nothing
    };
    cb.set_text(text)?;
    sleep(Duration::from_millis(30));
    crate::platform::send_paste();
    if restore {
        sleep(Duration::from_millis(400)); // let the target app read the clipboard first
        match saved {
            Saved::Text(t) => cb.set_text(t)?,
            Saved::Image(i) => cb.set_image(i)?,
            Saved::Nothing => {}
        }
    }
    Ok(())
}
