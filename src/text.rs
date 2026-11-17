//! Helpers for untrusted text that will be shown to people.

/// Longest display text kept, in characters. Titles, channel names and user
/// names are far shorter in practice; this bounds hostile or odd data.
pub const MAX_TEXT_CHARS: usize = 200;

/// Longest message a viewer may add to a request, in characters. It is shown
/// on the overlay, where a long one would not fit.
pub const MAX_MESSAGE_CHARS: usize = 150;

/// `sanitize_text`, cut to `MAX_MESSAGE_CHARS`.
pub fn sanitize_message(text: &str) -> String {
    sanitize_text(text)
        .chars()
        .take(MAX_MESSAGE_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// Replaces control characters (including newlines) with spaces, trims, and
/// truncates to `MAX_TEXT_CHARS` characters.
///
/// The result is plain text. It is not HTML-escaped; show it with
/// `textContent` or escape it for the context it ends up in.
pub fn sanitize_text(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned.trim().chars().take(MAX_TEXT_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_control_characters_and_trims() {
        assert_eq!(sanitize_text("  hello\nworld\t! "), "hello world !");
        assert_eq!(sanitize_text("a\0b\u{7}c"), "a b c");
        assert_eq!(sanitize_text(""), "");
        assert_eq!(sanitize_text("\n\n"), "");
    }

    #[test]
    fn sanitize_truncates_by_characters_not_bytes() {
        let long = "é".repeat(MAX_TEXT_CHARS + 50);
        let out = sanitize_text(&long);
        assert_eq!(out.chars().count(), MAX_TEXT_CHARS);
    }

    #[test]
    fn a_message_is_cut_shorter_and_never_ends_in_a_space() {
        let long = format!("{} tail", "x".repeat(MAX_MESSAGE_CHARS - 1));
        let out = sanitize_message(&long);
        assert_eq!(out.chars().count(), MAX_MESSAGE_CHARS - 1);
        assert_eq!(sanitize_message("  hi\nthere "), "hi there");
        assert_eq!(sanitize_message(""), "");
    }

    #[test]
    fn sanitize_keeps_markup_as_plain_text() {
        assert_eq!(
            sanitize_text("<script>alert(1)</script>"),
            "<script>alert(1)</script>"
        );
    }
}
