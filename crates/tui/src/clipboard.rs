use arboard::Clipboard;

/// Copy plain text to the system clipboard.
/// Returns an error description string on failure.
pub fn copy_text(text: &str) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| format!("clipboard init failed: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("clipboard set failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_copy_text_does_not_panic() {
        // We cannot assert clipboard content in a headless environment,
        // but we can verify the function does not panic on valid input.
        let result = copy_text("hello clipboard");
        // In CI/headless env clipboard may fail; we just ensure it returns.
        let _ = result;
    }
}
