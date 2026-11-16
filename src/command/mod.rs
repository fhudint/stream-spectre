//! Parsing of chat messages into commands.
//!
//! Chat text is untrusted. The parser only ever hands on a validated
//! `VideoId`; the raw text is dropped.

use crate::youtube::{VideoId, extract_video_id};

/// Why a message that is clearly a command was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidReason {
    /// The command was sent without an argument.
    MissingArgument,
    /// The argument is not a recognised video ID or YouTube link.
    InvalidLink,
}

/// Result of looking at one chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// Ordinary chat; ignore it.
    NotACommand,
    /// The video command with an unusable argument; worth telling the sender.
    Invalid(InvalidReason),
    /// A valid video request.
    Video(VideoId),
}

/// Recognises `<prefix><name> <id or link>`, for example `!v dQw4w9WgXcQ`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandParser {
    prefix: String,
    name: String,
}

impl CommandParser {
    pub fn new(prefix: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            name: name.into(),
        }
    }

    /// Classifies a chat message. Never panics, whatever the input.
    ///
    /// - The prefix must match exactly; the command name is case-insensitive.
    /// - The name must be a whole word, so `!vid` is not `!v`.
    /// - Only the first word after the name is used; the rest is ignored.
    pub fn parse(&self, message: &str) -> Parsed {
        let Some(after_prefix) = message.trim().strip_prefix(self.prefix.as_str()) else {
            return Parsed::NotACommand;
        };

        let (word, rest) = after_prefix
            .split_once(char::is_whitespace)
            .unwrap_or((after_prefix, ""));
        if !word.eq_ignore_ascii_case(&self.name) {
            return Parsed::NotACommand;
        }

        let Some(argument) = rest.split_whitespace().next() else {
            return Parsed::Invalid(InvalidReason::MissingArgument);
        };
        match extract_video_id(argument) {
            Some(id) => Parsed::Video(id),
            None => Parsed::Invalid(InvalidReason::InvalidLink),
        }
    }
}

#[cfg(test)]
mod tests {
    use regex::Regex;

    use super::*;

    const ID: &str = "dQw4w9WgXcQ";

    fn default_parser() -> CommandParser {
        CommandParser::new("!", "v")
    }

    fn video(id: &str) -> Parsed {
        Parsed::Video(VideoId::parse(id).unwrap())
    }

    #[test]
    fn accepts_bare_id_and_the_three_link_forms() {
        let p = default_parser();
        assert_eq!(p.parse("!v dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("!v https://youtu.be/dQw4w9WgXcQ"), video(ID));
        assert_eq!(
            p.parse("!v https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            video(ID)
        );
        assert_eq!(
            p.parse("!v https://www.youtube.com/shorts/dQw4w9WgXcQ"),
            video(ID)
        );
    }

    #[test]
    fn command_name_is_case_insensitive_but_the_id_keeps_its_case() {
        let p = default_parser();
        assert_eq!(p.parse("!V dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("!v DQW4W9WGXCQ"), video("DQW4W9WGXCQ"));
    }

    #[test]
    fn tolerates_surrounding_and_repeated_whitespace() {
        let p = default_parser();
        assert_eq!(p.parse("  !v   dQw4w9WgXcQ  "), video(ID));
        assert_eq!(p.parse("!v\tdQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("!v\u{a0}dQw4w9WgXcQ"), video(ID)); // no-break space
        assert_eq!(p.parse("!v\r\ndQw4w9WgXcQ\r\n"), video(ID));
    }

    #[test]
    fn only_the_first_word_after_the_name_counts() {
        let p = default_parser();
        assert_eq!(p.parse("!v dQw4w9WgXcQ please play this"), video(ID));
        assert_eq!(
            p.parse("!v please dQw4w9WgXcQ"),
            Parsed::Invalid(InvalidReason::InvalidLink)
        );
    }

    #[test]
    fn ordinary_chat_is_not_a_command() {
        let p = default_parser();
        for text in [
            "",
            "   ",
            "hello",
            "v dQw4w9WgXcQ",    // no prefix
            "! v dQw4w9WgXcQ",  // space after prefix
            "!vid dQw4w9WgXcQ", // name must be a whole word
            "!vv dQw4w9WgXcQ",
            "!play dQw4w9WgXcQ",
            "hey !v dQw4w9WgXcQ", // prefix must start the message
            "!",
            "!!v dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ",
            "😀 !v dQw4w9WgXcQ",
        ] {
            assert_eq!(p.parse(text), Parsed::NotACommand, "{text:?}");
        }
    }

    #[test]
    fn command_without_argument_is_reported() {
        let p = default_parser();
        for text in ["!v", "!v ", "!V   ", "  !v\t"] {
            assert_eq!(
                p.parse(text),
                Parsed::Invalid(InvalidReason::MissingArgument),
                "{text:?}"
            );
        }
    }

    #[test]
    fn unusable_arguments_are_reported_as_invalid_links() {
        let p = default_parser();
        for text in [
            "!v notanid",
            "!v dQw4w9WgXc",
            "!v dQw4w9WgXcQQ",
            "!v https://evil.com/watch?v=dQw4w9WgXcQ",
            "!v javascript:alert(1)",
            "!v <script>alert(1)</script>",
            "!v 🎵",
            "!v https://www.youtube.com/playlist?list=PLabc",
        ] {
            assert_eq!(
                p.parse(text),
                Parsed::Invalid(InvalidReason::InvalidLink),
                "{text:?}"
            );
        }
    }

    #[test]
    fn prefix_and_name_come_from_configuration() {
        let p = CommandParser::new("#", "play");
        assert_eq!(p.parse("#play dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("#PLAY dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("!v dQw4w9WgXcQ"), Parsed::NotACommand);
        assert_eq!(p.parse("#playlist dQw4w9WgXcQ"), Parsed::NotACommand);

        let p = CommandParser::new("?!", "song_1");
        assert_eq!(p.parse("?!song_1 dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("?song_1 dQw4w9WgXcQ"), Parsed::NotACommand);
    }

    #[test]
    fn oversized_messages_do_not_panic() {
        let p = default_parser();
        let long_arg = format!("!v {}", "a".repeat(100_000));
        assert_eq!(
            p.parse(&long_arg),
            Parsed::Invalid(InvalidReason::InvalidLink)
        );
        let many_words = format!("!v {ID} {}", "word ".repeat(50_000));
        assert_eq!(p.parse(&many_words), video(ID));
        assert_eq!(p.parse(&"!".repeat(100_000)), Parsed::NotACommand);
    }

    /// Same tiny generator as in the video ID tests.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
    }

    #[test]
    fn random_messages_never_panic_and_only_yield_valid_ids() {
        const PIECES: &[&str] = &[
            "!v",
            "!V",
            "!",
            "v",
            " ",
            "\t",
            "\n",
            "\u{a0}",
            "\u{200b}",
            ID,
            "https://",
            "http://",
            "youtu.be/",
            "youtube.com/watch?v=",
            "www.youtube.com/shorts/",
            "&",
            "?",
            "=",
            "😀",
            "é",
            "%41",
            "%",
            "..",
            "@",
            ":",
            "/",
            "{",
            "}",
            "<",
            ">",
            "javascript:",
            "ftp://",
            "\0",
            "\\",
            "https://youtu.be/dQw4w9WgXcQ",
            "youtu.be/dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        ];
        let valid = Regex::new(r"^[A-Za-z0-9_-]{11}$").unwrap();
        let p = default_parser();
        let mut rng = Lcg(7);
        let mut videos = 0;

        for _ in 0..50_000 {
            // Half of the messages start like a real command so the valid
            // path is exercised as well as the rejection paths.
            let start = if rng.next() % 2 == 0 { "!v " } else { "" };
            let count = (rng.next() % 12) as usize;
            let body: String = (0..count)
                .map(|_| PIECES[(rng.next() % PIECES.len() as u64) as usize])
                .collect();
            let message = format!("{start}{body}");
            if let Parsed::Video(id) = p.parse(&message) {
                assert!(valid.is_match(id.as_str()), "{message:?} -> {id}");
                videos += 1;
            }
        }
        assert!(
            videos > 100,
            "generator should hit valid commands too ({videos})"
        );
    }
}
