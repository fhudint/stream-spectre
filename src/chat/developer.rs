//! The accounts of the author of the program. Their chat lines carry a `dev`
//! badge, and `@dev text` from them shows a short notice on the mediashare
//! overlay (which a streamer can switch off with `overlay.mediashare.developer_notice`).
//!
//! These are public identifiers, not secrets: a viewer cannot take them over,
//! because the chat platforms send the real account, not the display name.
//! A fork changes them here.

/// TikTok `unique_id` (the username without `@`); TikTok keeps these unique.
const TIKTOK_USERNAME: &str = "fhudint";
/// YouTube channel ID, which unlike the name cannot be copied by another channel.
const YOUTUBE_CHANNEL: &str = "UCEezyhkNVyvreFHALgKT7LQ";

pub fn is_tiktok(unique_id: &str) -> bool {
    unique_id.eq_ignore_ascii_case(TIKTOK_USERNAME)
}

pub fn is_youtube(channel: &str) -> bool {
    channel == YOUTUBE_CHANNEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_account_not_a_lookalike() {
        assert!(is_tiktok("fhudint") && is_tiktok("FHUDINT"));
        assert!(!is_tiktok("fhudint2") && !is_tiktok(""));
        assert!(is_youtube(YOUTUBE_CHANNEL));
        assert!(!is_youtube("UCEezyhkNVyvreFHALgKT7LR") && !is_youtube(""));
    }
}
