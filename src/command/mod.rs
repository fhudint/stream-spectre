//! Parsing of chat messages into commands.
//!
//! Chat text is untrusted. The parser only ever hands on validated values (a
//! `VideoId`, or a `TiktokRef` made of a TikTok video ID or short code); the
//! raw text is dropped.

use crate::config::{CommandConfig, CommandEntry, CommandSource};
use crate::text::sanitize_message;
use crate::tiktok::{TiktokRef, extract_tiktok_ref};
use crate::youtube::{VideoId, extract_video};

/// Why a message that is clearly a command was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidReason {
    /// The command was sent without an argument.
    MissingArgument,
    /// The argument is not a recognised video ID or link of the command's
    /// source.
    InvalidLink,
    /// The argument is a link of another source, which has its own command.
    WrongSource(CommandSource),
}

/// Result of looking at one chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// Ordinary chat; ignore it.
    NotACommand,
    /// A video command with an unusable argument; worth telling the sender.
    /// `source` is the one the command is for.
    Invalid {
        source: CommandSource,
        reason: InvalidReason,
    },
    /// A valid YouTube request. `message` is what the viewer wrote after the
    /// link, cleaned up and cut short; empty when they wrote nothing. `short`
    /// says the link was a `youtube.com/shorts/` one.
    Youtube {
        id: VideoId,
        message: String,
        short: bool,
    },
    /// A valid TikTok request. A short code still has to be looked up.
    Tiktok { target: TiktokRef, message: String },
}

/// Recognises `<prefix><name> <id or link> [message]`, for example
/// `!v dQw4w9WgXcQ nice song`, for any of the configured names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandParser {
    prefix: String,
    commands: Vec<CommandEntry>,
}

impl CommandParser {
    pub fn new(config: &CommandConfig) -> Self {
        Self {
            prefix: config.prefix.clone(),
            commands: config.commands.clone(),
        }
    }

    /// What a viewer types to ask for a video of `source`, such as `!vt`: the
    /// first command configured for it, or `None` when there is none.
    pub fn command_for(&self, source: CommandSource) -> Option<String> {
        self.commands
            .iter()
            .find(|command| command.source == source)
            .map(|command| format!("{}{}", self.prefix, command.name))
    }

    /// Classifies a chat message. Never panics, whatever the input.
    ///
    /// - The prefix must match exactly; the command name is case-insensitive.
    /// - The name must be a whole word, so `!vid` is not `!v`, and `!vt` is
    ///   not `!v`.
    /// - The first word after the name is the video; what follows it is the
    ///   viewer's message (plain text, see `sanitize_message`).
    /// - A command accepts only its own source. A link of the other source
    ///   is reported as such, so the sender can be pointed to the right
    ///   command; a bare ID is never taken for the other source.
    pub fn parse(&self, message: &str) -> Parsed {
        let Some(after_prefix) = message.trim().strip_prefix(self.prefix.as_str()) else {
            return Parsed::NotACommand;
        };

        let (word, rest) = after_prefix
            .split_once(char::is_whitespace)
            .unwrap_or((after_prefix, ""));
        let Some(command) = self
            .commands
            .iter()
            .find(|command| word.eq_ignore_ascii_case(&command.name))
        else {
            return Parsed::NotACommand;
        };

        let source = command.source;
        let invalid = |reason| Parsed::Invalid { source, reason };
        let (argument, message) = rest
            .trim_start()
            .split_once(char::is_whitespace)
            .unwrap_or((rest.trim_start(), ""));
        if argument.is_empty() {
            return invalid(InvalidReason::MissingArgument);
        }
        let message = sanitize_message(message);
        match source {
            CommandSource::Youtube => match extract_video(argument) {
                Some((id, short)) => Parsed::Youtube { id, message, short },
                None if is_link(argument) && extract_tiktok_ref(argument).is_some() => {
                    invalid(InvalidReason::WrongSource(CommandSource::Tiktok))
                }
                None => invalid(InvalidReason::InvalidLink),
            },
            CommandSource::Tiktok => match extract_tiktok_ref(argument) {
                Some(target) => Parsed::Tiktok { target, message },
                None if is_link(argument) && extract_video(argument).is_some() => {
                    invalid(InvalidReason::WrongSource(CommandSource::Youtube))
                }
                None => invalid(InvalidReason::InvalidLink),
            },
        }
    }
}

/// Whether the argument is written as a link rather than a bare ID or code.
fn is_link(argument: &str) -> bool {
    argument.contains('/')
}

#[cfg(test)]
mod tests {
    use regex::Regex;

    use super::*;

    const ID: &str = "dQw4w9WgXcQ";

    fn parser(prefix: &str, names: &[&str]) -> CommandParser {
        CommandParser::new(&CommandConfig {
            prefix: prefix.to_owned(),
            commands: names
                .iter()
                .map(|name| CommandEntry::youtube(*name))
                .collect(),
        })
    }

    fn invalid(reason: InvalidReason) -> Parsed {
        Parsed::Invalid {
            source: CommandSource::Youtube,
            reason,
        }
    }

    fn default_parser() -> CommandParser {
        parser("!", &["v"])
    }

    fn video(id: &str) -> Parsed {
        video_with(id, "")
    }

    fn video_with(id: &str, message: &str) -> Parsed {
        Parsed::Youtube {
            id: VideoId::parse(id).unwrap(),
            message: message.to_owned(),
            short: false,
        }
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
            Parsed::Youtube {
                id: VideoId::parse(ID).unwrap(),
                message: String::new(),
                short: true,
            },
            "a Shorts link says so"
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
    fn the_first_word_after_the_name_is_the_video_and_the_rest_is_the_message() {
        let p = default_parser();
        assert_eq!(
            p.parse("!v dQw4w9WgXcQ please play this"),
            video_with(ID, "please play this")
        );
        assert_eq!(
            p.parse("!v please dQw4w9WgXcQ"),
            invalid(InvalidReason::InvalidLink)
        );
        // Spaces and line breaks in it are tidied; markup stays plain text.
        assert_eq!(
            p.parse("!v  dQw4w9WgXcQ \t halo   ka\n<b>x</b> "),
            video_with(ID, "halo   ka <b>x</b>")
        );
        assert_eq!(p.parse("!v dQw4w9WgXcQ   "), video(ID));
    }

    #[test]
    fn a_long_message_is_cut_and_the_video_still_counts() {
        let p = default_parser();
        let long = "a".repeat(500);
        match p.parse(&format!("!v {ID} {long}")) {
            Parsed::Youtube { message, .. } => {
                assert_eq!(message.chars().count(), crate::text::MAX_MESSAGE_CHARS);
            }
            other => panic!("{other:?}"),
        }
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
                invalid(InvalidReason::MissingArgument),
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
                invalid(InvalidReason::InvalidLink),
                "{text:?}"
            );
        }
    }

    #[test]
    fn prefix_and_name_come_from_configuration() {
        let p = parser("#", &["play"]);
        assert_eq!(p.parse("#play dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("#PLAY dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("!v dQw4w9WgXcQ"), Parsed::NotACommand);
        assert_eq!(p.parse("#playlist dQw4w9WgXcQ"), Parsed::NotACommand);

        let p = parser("?!", &["song_1"]);
        assert_eq!(p.parse("?!song_1 dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("?song_1 dQw4w9WgXcQ"), Parsed::NotACommand);
    }

    #[test]
    fn every_configured_name_is_a_command_and_the_others_are_not() {
        let p = parser("!", &["v", "yt", "Clip"]);
        assert_eq!(p.parse("!v dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("!yt dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("!YT dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("!clip dQw4w9WgXcQ"), video(ID));
        // A name that only starts like a configured one is not a command.
        for text in ["!vt dQw4w9WgXcQ", "!y dQw4w9WgXcQ", "!ytt dQw4w9WgXcQ"] {
            assert_eq!(p.parse(text), Parsed::NotACommand, "{text:?}");
        }
        // Each name reports its own problems.
        assert_eq!(p.parse("!yt"), invalid(InvalidReason::MissingArgument));
        assert_eq!(p.parse("!clip nope"), invalid(InvalidReason::InvalidLink));
    }

    #[test]
    fn a_name_that_starts_like_another_is_told_apart() {
        let p = parser("!", &["v", "vt"]);
        assert_eq!(p.parse("!v dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("!vt dQw4w9WgXcQ"), video(ID));
        assert_eq!(p.parse("!vtt dQw4w9WgXcQ"), Parsed::NotACommand);
    }

    fn mixed() -> CommandParser {
        CommandParser::new(&CommandConfig {
            prefix: "!".to_owned(),
            commands: vec![
                CommandEntry::youtube("v"),
                CommandEntry::tiktok("vt"),
                CommandEntry::tiktok("tt"),
            ],
        })
    }

    fn tiktok_invalid(reason: InvalidReason) -> Parsed {
        Parsed::Invalid {
            source: CommandSource::Tiktok,
            reason,
        }
    }

    const TT_ID: &str = "7700124541145599252";

    #[test]
    fn a_tiktok_command_takes_an_id_a_code_or_a_link() {
        let p = mixed();
        let id = |parsed: Parsed| match parsed {
            Parsed::Tiktok {
                target: TiktokRef::Video { id, account },
                ..
            } => Some((id.to_string(), account)),
            _ => None,
        };
        assert_eq!(
            id(p.parse(&format!("!vt {TT_ID}"))),
            Some((TT_ID.to_owned(), None))
        );
        assert_eq!(
            id(p.parse(&format!("!TT {TT_ID} please"))),
            Some((TT_ID.to_owned(), None))
        );
        assert_eq!(
            id(p.parse(&format!(
                "!vt https://www.tiktok.com/@cherlynn052/video/{TT_ID}?is_from_webapp=1"
            ))),
            Some((TT_ID.to_owned(), Some("cherlynn052".to_owned())))
        );
        match p.parse("!vt ZSpGELffb") {
            Parsed::Tiktok {
                target: TiktokRef::Short(code),
                ..
            } => assert_eq!(code.as_str(), "ZSpGELffb"),
            other => panic!("{other:?}"),
        }
        match p.parse("!vt https://vt.tiktok.com/ZSpGELffb/ halo ka") {
            Parsed::Tiktok {
                target: TiktokRef::Short(code),
                message,
            } => {
                assert_eq!(code.as_str(), "ZSpGELffb");
                assert_eq!(message, "halo ka");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn each_command_takes_only_its_own_source() {
        let p = mixed();
        // YouTube things after a TikTok word, and the other way round.
        assert_eq!(
            p.parse("!vt https://youtu.be/dQw4w9WgXcQ"),
            tiktok_invalid(InvalidReason::WrongSource(CommandSource::Youtube))
        );
        assert_eq!(
            p.parse("!v https://vt.tiktok.com/ZSpGELffb/"),
            invalid(InvalidReason::WrongSource(CommandSource::Tiktok))
        );
        assert_eq!(
            p.parse(&format!("!v https://www.tiktok.com/@a/video/{TT_ID}")),
            invalid(InvalidReason::WrongSource(CommandSource::Tiktok))
        );
        // A bare ID or code is never taken for the other source: after the
        // YouTube word it is simply not a YouTube ID.
        assert_eq!(
            p.parse(&format!("!v {TT_ID}")),
            invalid(InvalidReason::InvalidLink)
        );
        assert_eq!(p.parse("!v ZSpGELffb"), invalid(InvalidReason::InvalidLink));
        assert_eq!(p.parse("!v notanid"), invalid(InvalidReason::InvalidLink));
        // And after the TikTok word a YouTube ID with a dash is not a code.
        assert_eq!(
            p.parse("!vt a_b-C1d2E3f"),
            tiktok_invalid(InvalidReason::InvalidLink)
        );
        // Garbage and other hosts are just invalid.
        for text in [
            "!vt nope!",
            "!vt https://evil.com/x",
            "!vt javascript:alert(1)",
            "!vt 12",
        ] {
            assert_eq!(
                p.parse(text),
                tiktok_invalid(InvalidReason::InvalidLink),
                "{text:?}"
            );
        }
        assert_eq!(
            p.parse("!vt"),
            tiktok_invalid(InvalidReason::MissingArgument)
        );
        // The words stay apart: `!v` is YouTube, `!vt` and `!tt` are TikTok.
        assert!(matches!(
            p.parse(&format!("!v {ID}")),
            Parsed::Youtube { .. }
        ));
        assert!(matches!(p.parse("!tt ZSpGELffb"), Parsed::Tiktok { .. }));
        assert_eq!(p.parse("!vtt ZSpGELffb"), Parsed::NotACommand);
    }

    #[test]
    fn the_command_for_a_source_is_its_first_one_with_the_prefix() {
        let p = mixed();
        assert_eq!(p.command_for(CommandSource::Youtube).as_deref(), Some("!v"));
        assert_eq!(p.command_for(CommandSource::Tiktok).as_deref(), Some("!vt"));
        assert_eq!(default_parser().command_for(CommandSource::Tiktok), None);
        assert_eq!(
            parser("#", &["play"])
                .command_for(CommandSource::Youtube)
                .as_deref(),
            Some("#play")
        );
    }

    #[test]
    fn oversized_messages_do_not_panic() {
        let p = default_parser();
        let long_arg = format!("!v {}", "a".repeat(100_000));
        assert_eq!(p.parse(&long_arg), invalid(InvalidReason::InvalidLink));
        let many_words = format!("!v {ID} {}", "word ".repeat(50_000));
        match p.parse(&many_words) {
            Parsed::Youtube { id, message, .. } => {
                assert_eq!(id.as_str(), ID);
                assert!(message.starts_with("word word") && message.chars().count() <= 150);
            }
            other => panic!("{other:?}"),
        }
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
            if let Parsed::Youtube { id, .. } = p.parse(&message) {
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
