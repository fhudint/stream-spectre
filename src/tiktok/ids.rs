use std::fmt;

use serde::{Deserialize, Serialize};

/// TikTok video IDs are 64-bit numbers, 19 digits today. The bounds leave room
/// either way without accepting a short number someone typed by accident.
pub const VIDEO_ID_MIN_LEN: usize = 15;
pub const VIDEO_ID_MAX_LEN: usize = 20;

const SHORT_CODE_MIN_LEN: usize = 6;
const SHORT_CODE_MAX_LEN: usize = 12;

/// A validated TikTok video ID: only ASCII digits, `VIDEO_ID_MIN_LEN` to
/// `VIDEO_ID_MAX_LEN` of them. Safe to put in a URL or in HTML.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TiktokVideoId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a valid TikTok video ID (expected 15 to 20 digits)")]
pub struct InvalidTiktokVideoId;

impl TiktokVideoId {
    pub fn parse(candidate: &str) -> Result<Self, InvalidTiktokVideoId> {
        let len_ok = (VIDEO_ID_MIN_LEN..=VIDEO_ID_MAX_LEN).contains(&candidate.len());
        if len_ok && candidate.bytes().all(|b| b.is_ascii_digit()) {
            Ok(Self(candidate.to_owned()))
        } else {
            Err(InvalidTiktokVideoId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TiktokVideoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for TiktokVideoId {
    type Error = InvalidTiktokVideoId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<TiktokVideoId> for String {
    fn from(id: TiktokVideoId) -> Self {
        id.0
    }
}

/// The code of a TikTok short link (`ZSpGELffb` in `vt.tiktok.com/ZSpGELffb/`):
/// ASCII letters and digits, 6 to 12 of them. Capitals matter: the code is
/// case-sensitive on TikTok's side, so it is kept exactly as typed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ShortCode(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a valid TikTok short link code (expected 6 to 12 letters or digits)")]
pub struct InvalidShortCode;

impl ShortCode {
    pub fn parse(candidate: &str) -> Result<Self, InvalidShortCode> {
        let len_ok = (SHORT_CODE_MIN_LEN..=SHORT_CODE_MAX_LEN).contains(&candidate.len());
        if len_ok && candidate.bytes().all(|b| b.is_ascii_alphanumeric()) {
            Ok(Self(candidate.to_owned()))
        } else {
            Err(InvalidShortCode)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ShortCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_ids_are_15_to_20_digits() {
        for good in [
            "7700124541145599252",
            "7219950637247728923",
            &"1".repeat(15),
            &"9".repeat(20),
        ] {
            assert_eq!(TiktokVideoId::parse(good).unwrap().as_str(), good);
        }
        for bad in [
            "",
            "77001245411455", // 14 digits
            &"1".repeat(21),
            "77001245411455992a2",
            "-7700124541145599252",
            " 7700124541145599252",
            "7700124541145599252 ",
            "７７００１２４５４１１４５５９９２５２", // full-width digits
            "7700124541145599252/",
            "dQw4w9WgXcQ",
        ] {
            assert!(TiktokVideoId::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn video_ids_round_trip_through_json_and_refuse_bad_ones() {
        let id = TiktokVideoId::parse("7700124541145599252").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"7700124541145599252\"");
        assert_eq!(serde_json::from_str::<TiktokVideoId>(&json).unwrap(), id);
        assert!(serde_json::from_str::<TiktokVideoId>("\"12\"").is_err());
        assert!(serde_json::from_str::<TiktokVideoId>("7700124541145599252").is_err());
    }

    #[test]
    fn short_codes_are_6_to_12_letters_or_digits_and_keep_their_case() {
        for good in ["ZSpGELffb", "ZSpGEL", "ZSpGELffb123", "123456", "abcdef"] {
            assert_eq!(ShortCode::parse(good).unwrap().as_str(), good);
        }
        for bad in [
            "",
            "ZSpGE",
            "ZSpGELffb1234",
            "ZSp GELffb",
            "ZSp-GELffb",
            "ZSp_GELffb",
            "ZSpGELf/b",
            "ZSpGELfñb",
            "ZSpGELffb\n",
            "../..",
        ] {
            assert!(ShortCode::parse(bad).is_err(), "{bad:?}");
        }
        assert_ne!(ShortCode::parse("ZSpGELffb"), ShortCode::parse("zspgelffb"));
    }
}
