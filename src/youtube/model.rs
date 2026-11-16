use super::video_id::VideoId;

/// Whether a video is a live broadcast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveStatus {
    /// An ordinary uploaded video.
    None,
    /// Streaming right now.
    Live,
    /// A scheduled stream or premiere that has not started.
    Upcoming,
}

/// What we need to know about a video to decide whether to play it.
///
/// `title` and `channel` come from YouTube and are untrusted text: they have
/// been stripped of control characters and truncated, but they are not
/// HTML-escaped. Show them only as plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoDetails {
    pub id: VideoId,
    pub title: String,
    pub channel: String,
    pub duration_secs: u64,
    pub embeddable: bool,
    pub age_restricted: bool,
    pub live: LiveStatus,
    /// The video is private and cannot be played by others.
    pub private: bool,
}

/// Longest title or channel name kept, in characters. YouTube titles are at
/// most 100 characters; this only bounds hostile or unexpected data.
pub const MAX_TEXT_CHARS: usize = 200;

/// Replaces control characters (including newlines) with spaces, trims, and
/// truncates to `MAX_TEXT_CHARS` characters.
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
    fn sanitize_keeps_markup_as_plain_text() {
        assert_eq!(
            sanitize_text("<script>alert(1)</script>"),
            "<script>alert(1)</script>"
        );
    }
}
