use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Length of a YouTube video ID.
pub const VIDEO_ID_LEN: usize = 11;

/// A validated YouTube video ID: exactly `^[A-Za-z0-9_-]{11}$`.
///
/// This is the only form in which a video reference leaves the command parser.
/// Because the characters are restricted, an ID is safe to put in a URL or in
/// HTML; raw chat text never is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VideoId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a valid YouTube video ID (expected 11 characters from A-Z, a-z, 0-9, '_' and '-')")]
pub struct InvalidVideoId;

impl VideoId {
    pub fn parse(candidate: &str) -> Result<Self, InvalidVideoId> {
        if is_valid(candidate) {
            Ok(Self(candidate.to_owned()))
        } else {
            Err(InvalidVideoId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_valid(candidate: &str) -> bool {
    candidate.len() == VIDEO_ID_LEN
        && candidate
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl fmt::Display for VideoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for VideoId {
    type Err = InvalidVideoId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for VideoId {
    type Error = InvalidVideoId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if is_valid(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidVideoId)
        }
    }
}

impl From<VideoId> for String {
    fn from(id: VideoId) -> Self {
        id.0
    }
}

#[cfg(test)]
mod tests {
    use regex::Regex;

    use super::*;

    #[test]
    fn accepts_valid_ids() {
        for ok in [
            "dQw4w9WgXcQ",
            "aaaaaaaaaaa",
            "00000000000",
            "___________",
            "-----------",
            "a_b-C1d2E3f",
        ] {
            let id = VideoId::parse(ok).unwrap();
            assert_eq!(id.as_str(), ok);
            assert_eq!(id.to_string(), ok);
        }
    }

    #[test]
    fn ids_are_case_sensitive_and_kept_as_is() {
        assert_ne!(
            VideoId::parse("dQw4w9WgXcQ").unwrap(),
            VideoId::parse("dqw4w9wgxcq").unwrap()
        );
    }

    #[test]
    fn rejects_invalid_ids() {
        for bad in [
            "",
            "dQw4w9WgXc",   // 10
            "dQw4w9WgXcQQ", // 12
            "dQw4w9WgXc!",  // bad char
            "dQw4w9WgXc ",  // trailing space
            " dQw4w9WgXcQ", // leading space
            "dQw4w9WgXc\n", // trailing newline (11 bytes)
            "dQw4w9WgXc\0", // NUL
            "abcdefghijé",  // 10 chars, 11 bytes
            "abcdefghi😀",  // 10 chars, 13 bytes
            "dQw4w9Wg/cQ",  // slash
            "dQw4w9Wg%41Q", // percent-encoding
            "https://youtu.be/dQw4w9WgXcQ",
        ] {
            assert_eq!(VideoId::parse(bad), Err(InvalidVideoId), "{bad:?}");
        }
    }

    /// Small deterministic generator, so the test needs no extra crate.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }

        fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
            let len = items.len() as u64;
            let index = (self.next() % len) as usize;
            &items[index]
        }
    }

    #[test]
    fn behaves_exactly_like_the_documented_regex() {
        let regex = Regex::new(r"^[A-Za-z0-9_-]{11}$").unwrap();
        let valid_chars: Vec<char> = ('a'..='z')
            .chain('A'..='Z')
            .chain('0'..='9')
            .chain(['_', '-'])
            .collect();
        let mixed: Vec<char> = [
            'a', 'Z', '0', '9', '_', '-', '!', ' ', 'é', '\n', '.', '/', '😀',
        ]
        .to_vec();

        let mut rng = Lcg(42);
        let mut valid_seen = 0;
        for round in 0..40_000 {
            let alphabet = if round % 2 == 0 { &valid_chars } else { &mixed };
            let len = 9 + (rng.next() % 5) as usize; // 9..=13
            let candidate: String = (0..len).map(|_| *rng.pick(alphabet)).collect();

            let expected = regex.is_match(&candidate);
            assert_eq!(
                VideoId::parse(&candidate).is_ok(),
                expected,
                "{candidate:?}"
            );
            valid_seen += usize::from(expected);
        }
        assert!(valid_seen > 1000, "generator should produce many valid IDs");
    }

    #[test]
    fn serde_round_trip_and_validation() {
        let id = VideoId::parse("dQw4w9WgXcQ").unwrap();
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"dQw4w9WgXcQ\"");
        let back: VideoId = serde_json::from_str("\"dQw4w9WgXcQ\"").unwrap();
        assert_eq!(back, id);
        assert!(serde_json::from_str::<VideoId>("\"too-short\"").is_err());
        assert!(serde_json::from_str::<VideoId>("42").is_err());
    }

    #[test]
    fn from_str_and_string_conversion() {
        let id: VideoId = "dQw4w9WgXcQ".parse().unwrap();
        assert_eq!(String::from(id), "dQw4w9WgXcQ");
        assert!("nope".parse::<VideoId>().is_err());
    }
}
