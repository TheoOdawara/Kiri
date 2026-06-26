use std::io;

use arboard::Clipboard;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::modules::tui::domain::view_state::ImageAttachment;
use crate::shared::kernel::error::AgentError;

/// What the OS clipboard held when read: an image (encoded as a PNG data URL ready for the provider's
/// multimodal content), plain text, or nothing usable.
pub enum ClipboardContent {
    Image(ImageAttachment),
    Text(String),
    Empty,
}

/// Read the OS clipboard, preferring an image over text. Every failure collapses to `Empty`: clipboard
/// access is best-effort device I/O and must never crash the TUI.
pub fn read() -> ClipboardContent {
    let Ok(mut clipboard) = Clipboard::new() else {
        return ClipboardContent::Empty;
    };
    if let Ok(image) = clipboard.get_image()
        && let Some(attachment) = encode_png_data_url(&image)
    {
        return ClipboardContent::Image(attachment);
    }
    match clipboard.get_text() {
        Ok(text) if !text.is_empty() => ClipboardContent::Text(text),
        _ => ClipboardContent::Empty,
    }
}

/// Copy text to the OS clipboard. Copy is a direct user intent (Ctrl+C, mouse-release), so a failure is
/// surfaced by the caller as a transcript notice rather than swallowed. Empty text is a no-op that
/// returns `Ok` WITHOUT writing — a selection over blank cells must never clobber the user's clipboard.
pub fn copy_text(text: &str) -> Result<(), AgentError> {
    if text.is_empty() {
        return Ok(());
    }
    let mut clipboard = Clipboard::new()
        .map_err(|e| AgentError::Io(io::Error::other(format!("clipboard unavailable: {e}"))))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| AgentError::Io(io::Error::other(format!("clipboard write failed: {e}"))))?;
    Ok(())
}

/// Encode arboard's RGBA8 image (row-major, 4 bytes/pixel) as a `data:image/png;base64,...` URL.
fn encode_png_data_url(image: &arboard::ImageData<'_>) -> Option<ImageAttachment> {
    let (width, height) = (image.width, image.height);
    if width == 0 || height == 0 {
        return None;
    }
    let mut png: Vec<u8> = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(image.bytes.as_ref()).ok()?;
    }
    Some(ImageAttachment {
        data_url: format!("data:image/png;base64,{}", STANDARD.encode(&png)),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_text_empty_is_ok_and_does_not_touch_the_clipboard() {
        // Empty short-circuits before `Clipboard::new()`, so it is Ok in any (even headless) environment
        // and — by never reaching `set_text` — cannot overwrite the user's existing clipboard contents.
        assert!(copy_text("").is_ok());
    }
}
