use std::collections::HashSet;

use crate::config::{BlacklistConfig, ConfigError};
use crate::youtube::{VideoDetails, VideoId};

/// Which blacklist entry matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlacklistHit {
    Video,
    Channel,
    TitleWord,
}

/// Blocked videos, channels and title words.
///
/// - Video IDs match exactly (they are case-sensitive).
/// - Channel names match ignoring case, punctuation and extra spaces.
/// - Title words match as whole words or phrases, ignoring case and
///   punctuation: the entry `ass` does not block "Classic", and `bad word`
///   blocks "Bad-Word!". Scripts written without spaces (such as Chinese or
///   Japanese) have no word boundaries, so entries in them only match when
///   they stand apart in the title.
///
/// Look-alike characters from other alphabets are not folded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Blacklist {
    video_ids: HashSet<VideoId>,
    channels: HashSet<String>,
    title_phrases: Vec<String>,
}

impl Blacklist {
    pub fn from_config(config: &BlacklistConfig) -> Result<Self, ConfigError> {
        let mut video_ids = HashSet::new();
        for (i, entry) in config.video_ids.iter().enumerate() {
            let id = VideoId::parse(entry.trim()).map_err(|_| {
                ConfigError::invalid(
                    format!("blacklist.video_ids[{i}]"),
                    "must be an 11-character YouTube video ID",
                )
            })?;
            video_ids.insert(id);
        }

        let mut channels = HashSet::new();
        for (i, entry) in config.channels.iter().enumerate() {
            let name = normalize(entry);
            if name.is_empty() {
                return Err(ConfigError::invalid(
                    format!("blacklist.channels[{i}]"),
                    "must contain at least one letter or digit",
                ));
            }
            channels.insert(name);
        }

        let mut title_phrases = Vec::new();
        for (i, entry) in config.title_words.iter().enumerate() {
            let phrase = normalize(entry);
            if phrase.is_empty() {
                return Err(ConfigError::invalid(
                    format!("blacklist.title_words[{i}]"),
                    "must contain at least one letter or digit",
                ));
            }
            // Padded with spaces so matching respects word boundaries.
            title_phrases.push(format!(" {phrase} "));
        }

        Ok(Self {
            video_ids,
            channels,
            title_phrases,
        })
    }

    /// Cheap check that needs no YouTube lookup.
    pub fn check_video_id(&self, id: &VideoId) -> Option<BlacklistHit> {
        self.video_ids.contains(id).then_some(BlacklistHit::Video)
    }

    /// Checks the channel name and title of a looked-up video.
    pub fn check_details(&self, video: &VideoDetails) -> Option<BlacklistHit> {
        if let Some(hit) = self.check_video_id(&video.id) {
            return Some(hit);
        }
        if self.channels.contains(&normalize(&video.channel)) {
            return Some(BlacklistHit::Channel);
        }
        let title = format!(" {} ", normalize(&video.title));
        self.title_phrases
            .iter()
            .any(|phrase| title.contains(phrase.as_str()))
            .then_some(BlacklistHit::TitleWord)
    }
}

/// Lowercases, turns every run of non-alphanumeric characters into one space,
/// and trims.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(c.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::youtube::LiveStatus;

    fn config(ids: &[&str], channels: &[&str], words: &[&str]) -> BlacklistConfig {
        let own = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect();
        BlacklistConfig {
            video_ids: own(ids),
            channels: own(channels),
            title_words: own(words),
        }
    }

    fn video(id: &str, title: &str, channel: &str) -> VideoDetails {
        VideoDetails {
            id: VideoId::parse(id).unwrap(),
            title: title.to_owned(),
            channel: channel.to_owned(),
            duration_secs: 100,
            embeddable: true,
            age_restricted: false,
            live: LiveStatus::None,
            private: false,
        }
    }

    const OK_ID: &str = "dQw4w9WgXcQ";

    #[test]
    fn empty_blacklist_blocks_nothing() {
        let b = Blacklist::from_config(&BlacklistConfig::default()).unwrap();
        assert_eq!(b.check_details(&video(OK_ID, "anything", "anyone")), None);
        assert_eq!(b.check_video_id(&VideoId::parse(OK_ID).unwrap()), None);
    }

    #[test]
    fn video_ids_match_exactly_and_case_sensitively() {
        let b = Blacklist::from_config(&config(&["  dQw4w9WgXcQ "], &[], &[])).unwrap();
        let blocked = VideoId::parse("dQw4w9WgXcQ").unwrap();
        let other_case = VideoId::parse("DQW4W9WGXCQ").unwrap();
        assert_eq!(b.check_video_id(&blocked), Some(BlacklistHit::Video));
        assert_eq!(b.check_video_id(&other_case), None);
        assert_eq!(
            b.check_details(&video("dQw4w9WgXcQ", "t", "c")),
            Some(BlacklistHit::Video)
        );
    }

    #[test]
    fn channels_match_ignoring_case_punctuation_and_spacing() {
        let b = Blacklist::from_config(&config(&[], &["Bad Channel"], &[])).unwrap();
        for name in [
            "Bad Channel",
            "bad channel",
            "BAD   CHANNEL",
            "Bad-Channel!",
            " bad.channel ",
        ] {
            assert_eq!(
                b.check_details(&video(OK_ID, "t", name)),
                Some(BlacklistHit::Channel),
                "{name}"
            );
        }
        for name in ["Bad Channels", "Not Bad Channel", "Bad", ""] {
            assert_eq!(b.check_details(&video(OK_ID, "t", name)), None, "{name}");
        }
    }

    #[test]
    fn title_words_match_whole_words_only() {
        let b = Blacklist::from_config(&config(&[], &[], &["ass"])).unwrap();
        assert_eq!(
            b.check_details(&video(OK_ID, "Kick ass moments", "c")),
            Some(BlacklistHit::TitleWord)
        );
        for title in [
            "Classic songs",
            "Assassin's Creed",
            "bass boosted",
            "passing",
        ] {
            assert_eq!(b.check_details(&video(OK_ID, title, "c")), None, "{title}");
        }
    }

    #[test]
    fn title_words_ignore_case_and_punctuation_and_match_phrases() {
        let b = Blacklist::from_config(&config(&[], &[], &["bad word", "x-rated"])).unwrap();
        for title in [
            "BAD WORD",
            "a Bad-Word!",
            "so (bad word) here",
            "bad   word",
            "X Rated edit",
            "x-rated",
        ] {
            assert_eq!(
                b.check_details(&video(OK_ID, title, "c")),
                Some(BlacklistHit::TitleWord),
                "{title}"
            );
        }
        for title in ["bad", "word bad", "badword", "bad words"] {
            assert_eq!(b.check_details(&video(OK_ID, title, "c")), None, "{title}");
        }
    }

    #[test]
    fn unicode_words_are_case_folded() {
        let b = Blacklist::from_config(&config(&[], &[], &["Éclair", "ПЛОХО"])).unwrap();
        assert_eq!(
            b.check_details(&video(OK_ID, "mon éCLAIR au chocolat", "c")),
            Some(BlacklistHit::TitleWord)
        );
        assert_eq!(
            b.check_details(&video(OK_ID, "это плохо", "c")),
            Some(BlacklistHit::TitleWord)
        );
    }

    #[test]
    fn title_checks_an_empty_or_symbol_only_title_without_matching() {
        let b = Blacklist::from_config(&config(&[], &[], &["spam"])).unwrap();
        assert_eq!(b.check_details(&video(OK_ID, "", "c")), None);
        assert_eq!(b.check_details(&video(OK_ID, "!!! ???", "c")), None);
    }

    #[test]
    fn video_id_check_takes_priority_over_the_other_checks() {
        let b = Blacklist::from_config(&config(&[OK_ID], &["Chan"], &["word"])).unwrap();
        assert_eq!(
            b.check_details(&video(OK_ID, "word", "Chan")),
            Some(BlacklistHit::Video)
        );
        assert_eq!(
            b.check_details(&video("aaaaaaaaaaa", "word", "Chan")),
            Some(BlacklistHit::Channel)
        );
    }

    #[test]
    fn invalid_entries_are_rejected_with_their_position() {
        let field = |c: BlacklistConfig| match Blacklist::from_config(&c).unwrap_err() {
            ConfigError::Invalid { field, .. } => field,
            other => panic!("{other}"),
        };
        assert_eq!(
            field(config(&[OK_ID, "short"], &[], &[])),
            "blacklist.video_ids[1]"
        );
        assert_eq!(
            field(config(&[], &["ok", "!!!"], &[])),
            "blacklist.channels[1]"
        );
        assert_eq!(
            field(config(&[], &[], &["ok", " - "])),
            "blacklist.title_words[1]"
        );
        assert_eq!(field(config(&[], &[], &[""])), "blacklist.title_words[0]");
    }

    #[test]
    fn normalize_examples() {
        assert_eq!(normalize("  Hello,   World!! "), "hello world");
        assert_eq!(normalize("A-B_C.D"), "a b c d");
        assert_eq!(normalize("!!!"), "");
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("ÉCOLE"), "école");
    }
}
