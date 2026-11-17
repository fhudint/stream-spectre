use std::collections::HashSet;

use super::item::TiktokDetails;
use crate::config::{BlacklistConfig, ConfigError};
use crate::tiktok::TiktokVideoId;
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
/// - Video IDs match exactly (they are case-sensitive). A YouTube ID and a
///   TikTok ID can share one list: they cannot be mistaken for each other.
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
    video_ids: HashSet<String>,
    channels: HashSet<String>,
    title_phrases: Vec<String>,
}

impl Blacklist {
    pub fn from_config(config: &BlacklistConfig) -> Result<Self, ConfigError> {
        let mut video_ids = HashSet::new();
        for (i, entry) in config.video_ids.iter().enumerate() {
            let id = entry.trim();
            if VideoId::parse(id).is_err() && TiktokVideoId::parse(id).is_err() {
                return Err(ConfigError::invalid(
                    format!("blacklist.video_ids[{i}]"),
                    "must be an 11-character YouTube video ID or a TikTok video ID (15 to 20 digits)",
                ));
            }
            video_ids.insert(id.to_owned());
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

    /// Cheap check that needs no lookup. Takes a YouTube or a TikTok ID.
    pub fn check_video_id(&self, id: &(impl AsRef<str> + ?Sized)) -> Option<BlacklistHit> {
        self.video_ids
            .contains(id.as_ref())
            .then_some(BlacklistHit::Video)
    }

    /// Checks a TikTok video: its ID, and its account against the channel
    /// names (an account is the channel of a TikTok video). There is no title
    /// to check.
    pub fn check_tiktok(&self, video: &TiktokDetails) -> Option<BlacklistHit> {
        if let Some(hit) = self.check_video_id(&video.id) {
            return Some(hit);
        }
        let account = video.account.as_deref()?;
        self.channels
            .contains(&normalize(account))
            .then_some(BlacklistHit::Channel)
    }

    /// Whether a viewer's message has one of the banned words, the same words
    /// as for titles and matched the same way.
    pub fn check_message(&self, message: &str) -> bool {
        let text = format!(" {} ", normalize(message));
        self.title_phrases
            .iter()
            .any(|phrase| text.contains(phrase.as_str()))
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
    fn a_message_is_checked_against_the_same_words() {
        let b = Blacklist::from_config(&config(&[], &[], &["bad word"])).unwrap();
        assert!(b.check_message("you Bad-Word!"));
        assert!(!b.check_message("badword"));
        assert!(!b.check_message(""));
        assert!(!Blacklist::default().check_message("anything"));
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

    fn tiktok(id: &str, account: Option<&str>) -> TiktokDetails {
        TiktokDetails {
            id: TiktokVideoId::parse(id).unwrap(),
            account: account.map(str::to_owned),
        }
    }

    #[test]
    fn tiktok_ids_share_the_list_with_youtube_ids() {
        let b = Blacklist::from_config(&config(&[OK_ID, "7700124541145599252"], &[], &[])).unwrap();
        assert_eq!(
            b.check_tiktok(&tiktok("7700124541145599252", None)),
            Some(BlacklistHit::Video)
        );
        assert_eq!(b.check_tiktok(&tiktok("7700124541145599253", None)), None);
        assert_eq!(
            b.check_video_id("7700124541145599252"),
            Some(BlacklistHit::Video)
        );
        // Spaces around an entry are ignored, as for YouTube.
        let b = Blacklist::from_config(&config(&[" 7700124541145599252 "], &[], &[])).unwrap();
        assert!(
            b.check_tiktok(&tiktok("7700124541145599252", None))
                .is_some()
        );
        // Neither kind of ID: refused, with the position.
        for bad in [
            "12345",
            "77001245411455992a2",
            "https://www.tiktok.com/@a/video/7700124541145599252",
        ] {
            let error = Blacklist::from_config(&config(&["dQw4w9WgXcQ", bad], &[], &[]))
                .unwrap_err()
                .to_string();
            assert!(error.contains("blacklist.video_ids[1]"), "{error}");
        }
    }

    #[test]
    fn a_tiktok_account_is_checked_like_a_channel_name() {
        let b = Blacklist::from_config(&config(&[], &["Bad.Account_1"], &["spoiler"])).unwrap();
        for account in ["bad.account_1", "BAD.ACCOUNT_1", "bad_account.1"] {
            assert_eq!(
                b.check_tiktok(&tiktok("7700124541145599252", Some(account))),
                Some(BlacklistHit::Channel),
                "{account}"
            );
        }
        assert_eq!(
            b.check_tiktok(&tiktok("7700124541145599252", Some("other"))),
            None
        );
        // No account known: nothing to match. Title words have nothing to match.
        assert_eq!(b.check_tiktok(&tiktok("7700124541145599252", None)), None);
        assert_eq!(
            b.check_tiktok(&tiktok("7700124541145599252", Some("spoiler"))),
            None
        );
    }
}
