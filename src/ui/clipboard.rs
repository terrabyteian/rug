//! Clipboard integration: native clipboard via arboard, plus OSC 52 for tmux/SSH.

use std::io::{self, Write};

/// Cap on raw text bytes before base64 (terminals limit OSC 52 payloads).
const MAX_OSC52_BYTES: usize = 1_000_000;

/// RFC 4648 standard-alphabet base64 with padding (hand-rolled: not worth a dep).
fn b64(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::new();
    let mut i = 0;

    // Process 3-byte chunks.
    while i + 3 <= data.len() {
        let b1 = data[i];
        let b2 = data[i + 1];
        let b3 = data[i + 2];

        result.push(TABLE[(b1 >> 2) as usize] as char);
        result.push(TABLE[(((b1 & 0x03) << 4) | (b2 >> 4)) as usize] as char);
        result.push(TABLE[(((b2 & 0x0f) << 2) | (b3 >> 6)) as usize] as char);
        result.push(TABLE[(b3 & 0x3f) as usize] as char);

        i += 3;
    }

    // Process remaining bytes.
    let remaining = data.len() - i;
    if remaining == 1 {
        let b1 = data[i];
        result.push(TABLE[(b1 >> 2) as usize] as char);
        result.push(TABLE[((b1 & 0x03) << 4) as usize] as char);
        result.push_str("==");
    } else if remaining == 2 {
        let b1 = data[i];
        let b2 = data[i + 1];
        result.push(TABLE[(b1 >> 2) as usize] as char);
        result.push(TABLE[(((b1 & 0x03) << 4) | (b2 >> 4)) as usize] as char);
        result.push(TABLE[((b2 & 0x0f) << 2) as usize] as char);
        result.push('=');
    }

    result
}

/// Truncate `s` to at most `max` bytes without splitting a UTF-8 char.
/// Returns a slice at a char boundary.
fn truncate_on_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    // Walk backwards from `max` to find a valid UTF-8 boundary.
    for i in (0..=max).rev() {
        if s.is_char_boundary(i) {
            return &s[..i];
        }
    }
    // Should not happen if max > 0, but fallback to empty string.
    ""
}

/// The OSC 52 set-clipboard sequence for `text`. With `tmux_wrap`, the whole
/// sequence is wrapped in a tmux DCS passthrough (every ESC doubled) so it
/// reaches the outer terminal even without tmux's set-clipboard option.
pub fn osc52_sequence(text: &str, tmux_wrap: bool) -> String {
    let encoded = b64(text.as_bytes());
    let plain = format!("\x1b]52;c;{}\x07", encoded);

    if !tmux_wrap {
        return plain;
    }

    // Wrap for tmux: replace every ESC (\x1b) with ESC ESC (\x1b\x1b).
    let escaped = plain.replace('\x1b', "\x1b\x1b");
    format!("\x1bPtmux;{}\x1b\\", escaped)
}

/// Best-effort copy: arboard native clipboard (errors ignored — headless/SSH
/// setups lack one), then OSC 52 to stdout. Arboard receives the full text;
/// the OSC 52 payload is truncated to MAX_OSC52_BYTES at a char boundary.
pub fn copy_to_clipboard(text: &str) -> io::Result<()> {
    // Try native clipboard with the full text; ignore errors.
    let _ =
        arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text.to_string()));

    // Truncate to MAX_OSC52_BYTES at a char boundary for the OSC 52 payload only.
    let truncated = truncate_on_char_boundary(text, MAX_OSC52_BYTES);

    // Detect tmux.
    let tmux_wrap = std::env::var_os("TMUX").is_some();

    // Write OSC 52 sequence.
    let sequence = osc52_sequence(truncated, tmux_wrap);
    let mut stdout = io::stdout();
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_b64_empty() {
        assert_eq!(b64(b""), "");
    }

    #[test]
    fn test_b64_single() {
        assert_eq!(b64(b"f"), "Zg==");
    }

    #[test]
    fn test_b64_two() {
        assert_eq!(b64(b"fo"), "Zm8=");
    }

    #[test]
    fn test_b64_three() {
        assert_eq!(b64(b"foo"), "Zm9v");
    }

    #[test]
    fn test_b64_four() {
        assert_eq!(b64(b"foob"), "Zm9vYg==");
    }

    #[test]
    fn test_b64_five() {
        assert_eq!(b64(b"fooba"), "Zm9vYmE=");
    }

    #[test]
    fn test_b64_six() {
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_b64_null_and_ff() {
        assert_eq!(b64(&[0x00]), "AA==");
        assert_eq!(b64(&[0xff]), "/w==");
        assert_eq!(b64(&[0x00, 0xff]), "AP8=");
    }

    #[test]
    fn test_osc52_sequence_plain() {
        let result = osc52_sequence("test", false);
        assert_eq!(result, "\x1b]52;c;dGVzdA==\x07");
    }

    #[test]
    fn test_osc52_sequence_tmux() {
        let result = osc52_sequence("test", true);
        // The plain form is "\x1b]52;c;dGVzdA==\x07", so with tmux wrapping:
        // - ESC becomes ESC ESC: \x1b -> \x1b\x1b
        // - Wrap in DCS: \x1bPtmux;...\x1b\\
        let expected = "\x1bPtmux;\x1b\x1b]52;c;dGVzdA==\x07\x1b\\";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_truncate_on_char_boundary_within_bounds() {
        let s = "hello";
        assert_eq!(truncate_on_char_boundary(s, 10), "hello");
    }

    #[test]
    fn test_truncate_on_char_boundary_exact() {
        let s = "hello";
        assert_eq!(truncate_on_char_boundary(s, 5), "hello");
    }

    #[test]
    fn test_truncate_on_char_boundary_mid_char() {
        // "café" has 'é' as a multi-byte UTF-8 char (U+00E9 = 0xC3 0xA9).
        let s = "café";
        // Try to cut at byte 4 (would be in the middle of 'é').
        // Should return "caf" (3 bytes).
        assert_eq!(truncate_on_char_boundary(s, 4), "caf");
    }

    #[test]
    fn test_truncate_on_char_boundary_before_multibyte() {
        // "café" is "c" + "a" + "f" + "é" (2 bytes)
        // Truncating at byte 3 should give "caf".
        let s = "café";
        assert_eq!(truncate_on_char_boundary(s, 3), "caf");
    }

    #[test]
    fn test_truncate_on_char_boundary_include_multibyte() {
        // "café" is "c" (1) + "a" (1) + "f" (1) + "é" (2) = 5 bytes.
        // Truncating at byte 5 should include "café".
        let s = "café";
        assert_eq!(truncate_on_char_boundary(s, 5), "café");
    }

    #[test]
    fn test_truncate_on_char_boundary_zero_max() {
        let s = "hello";
        assert_eq!(truncate_on_char_boundary(s, 0), "");
    }

    #[test]
    fn test_osc52_with_special_chars() {
        // OSC 52 with text containing newlines and special chars.
        let text = "line1\nline2";
        let result = osc52_sequence(text, false);
        // Base64 of "line1\nline2" is "bGluZTEKbGluZTI="
        assert_eq!(result, "\x1b]52;c;bGluZTEKbGluZTI=\x07");
    }
}
