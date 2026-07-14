/// Strip ANSI escape sequences from a string.
/// Terraform and tofu emit ANSI colours even when piped on some configurations.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
                          // consume until we hit the final byte (an ASCII letter)
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passthrough() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn strips_ansi_sequences() {
        assert_eq!(strip_ansi("\x1b[1m\x1b[32mhello\x1b[0m"), "hello");
    }

    #[test]
    fn handles_unterminated_escape() {
        assert_eq!(strip_ansi("abc\x1b["), "abc");
    }
}
