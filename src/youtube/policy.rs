use super::duration::format_duration;
use super::model::{LiveStatus, VideoDetails};

/// Rules a video must satisfy to be accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPolicy {
    pub max_duration_secs: u64,
    /// Live streams and scheduled premieres may be played.
    pub allow_live: bool,
    /// The category numbers a video may have; any when empty.
    pub allowed_categories: Vec<u32>,
}

/// Why a video was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// Private, unplayable, or missing.
    NotFound,
    NotEmbeddable,
    AgeRestricted,
    /// Live streams and scheduled premieres, when they are not allowed.
    LiveStream,
    /// The video's category is not one of those allowed.
    CategoryNotAllowed,
    TooLong {
        max_secs: u64,
    },
}

impl Rejection {
    /// Key in the locale catalogs.
    pub fn i18n_key(&self) -> &'static str {
        match self {
            Self::NotFound => "reject.not_found",
            Self::NotEmbeddable => "reject.not_embeddable",
            Self::AgeRestricted => "reject.age_restricted",
            Self::LiveStream => "reject.live_stream",
            Self::CategoryNotAllowed => "reject.category_not_allowed",
            Self::TooLong { .. } => "reject.too_long",
        }
    }

    /// Placeholder values for the message at `i18n_key`.
    pub fn i18n_args(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::TooLong { max_secs } => vec![("max", format_duration(*max_secs))],
            _ => Vec::new(),
        }
    }
}

/// Checks a looked-up video against the policy. The first failing rule wins,
/// in a fixed order: unplayable, live, age-restricted, not embeddable, category,
/// too long.
pub fn check(video: &VideoDetails, policy: &VideoPolicy) -> Result<(), Rejection> {
    if video.private || (video.duration_secs == 0 && video.live == LiveStatus::None) {
        return Err(Rejection::NotFound);
    }
    if video.live != LiveStatus::None && !policy.allow_live {
        return Err(Rejection::LiveStream);
    }
    if video.age_restricted {
        return Err(Rejection::AgeRestricted);
    }
    if !video.embeddable {
        return Err(Rejection::NotEmbeddable);
    }
    if !policy.allowed_categories.is_empty()
        && !policy.allowed_categories.contains(&video.category_id)
    {
        return Err(Rejection::CategoryNotAllowed);
    }
    if video.duration_secs > policy.max_duration_secs {
        return Err(Rejection::TooLong {
            max_secs: policy.max_duration_secs,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::youtube::VideoId;

    const POLICY: VideoPolicy = VideoPolicy {
        max_duration_secs: 600,
        allow_live: false,
        allowed_categories: Vec::new(),
    };

    fn video() -> VideoDetails {
        VideoDetails {
            id: VideoId::parse("dQw4w9WgXcQ").unwrap(),
            title: "t".to_owned(),
            channel: "c".to_owned(),
            duration_secs: 213,
            embeddable: true,
            age_restricted: false,
            live: LiveStatus::None,
            private: false,
            category_id: 10,
        }
    }

    #[test]
    fn live_streams_pass_when_they_are_allowed() {
        let allowed = VideoPolicy {
            allow_live: true,
            ..POLICY
        };
        for live in [LiveStatus::Live, LiveStatus::Upcoming] {
            let mut v = video();
            v.live = live;
            v.duration_secs = 0;
            assert_eq!(check(&v, &allowed), Ok(()), "{live:?}");
        }
        // A video that is gone is still gone.
        let mut v = video();
        v.private = true;
        assert_eq!(check(&v, &allowed), Err(Rejection::NotFound));
    }

    #[test]
    fn only_the_allowed_categories_pass_when_some_are_named() {
        let music_and_games = VideoPolicy {
            allowed_categories: vec![10, 20],
            ..POLICY
        };
        let mut v = video();
        assert_eq!(check(&v, &music_and_games), Ok(()));
        v.category_id = 20;
        assert_eq!(check(&v, &music_and_games), Ok(()));
        v.category_id = 24;
        assert_eq!(
            check(&v, &music_and_games),
            Err(Rejection::CategoryNotAllowed)
        );
        v.category_id = 0;
        assert_eq!(
            check(&v, &music_and_games),
            Err(Rejection::CategoryNotAllowed),
            "a video that does not say is not allowed either"
        );
        assert_eq!(check(&v, &POLICY), Ok(()), "none named: all pass");
    }

    #[test]
    fn a_normal_video_passes() {
        assert_eq!(check(&video(), &POLICY), Ok(()));
    }

    #[test]
    fn duration_limit_is_inclusive() {
        let mut v = video();
        v.duration_secs = 600;
        assert_eq!(check(&v, &POLICY), Ok(()));
        v.duration_secs = 601;
        assert_eq!(
            check(&v, &POLICY),
            Err(Rejection::TooLong { max_secs: 600 })
        );
    }

    #[test]
    fn each_rule_rejects() {
        let mut v = video();
        v.embeddable = false;
        assert_eq!(check(&v, &POLICY), Err(Rejection::NotEmbeddable));

        let mut v = video();
        v.age_restricted = true;
        assert_eq!(check(&v, &POLICY), Err(Rejection::AgeRestricted));

        for live in [LiveStatus::Live, LiveStatus::Upcoming] {
            let mut v = video();
            v.live = live;
            v.duration_secs = 0;
            assert_eq!(check(&v, &POLICY), Err(Rejection::LiveStream), "{live:?}");
        }

        let mut v = video();
        v.private = true;
        assert_eq!(check(&v, &POLICY), Err(Rejection::NotFound));

        let mut v = video();
        v.duration_secs = 0;
        assert_eq!(check(&v, &POLICY), Err(Rejection::NotFound));
    }

    #[test]
    fn rules_apply_in_a_fixed_order() {
        let mut v = video();
        v.private = true;
        v.live = LiveStatus::Live;
        v.age_restricted = true;
        v.embeddable = false;
        v.duration_secs = 99_999;
        assert_eq!(check(&v, &POLICY), Err(Rejection::NotFound));
        v.private = false;
        assert_eq!(check(&v, &POLICY), Err(Rejection::LiveStream));
        v.live = LiveStatus::None;
        assert_eq!(check(&v, &POLICY), Err(Rejection::AgeRestricted));
        v.age_restricted = false;
        assert_eq!(check(&v, &POLICY), Err(Rejection::NotEmbeddable));
        v.embeddable = true;
        assert_eq!(
            check(&v, &POLICY),
            Err(Rejection::TooLong { max_secs: 600 })
        );
    }

    #[test]
    fn rejection_messages_exist_in_both_locales_with_matching_placeholders() {
        use crate::config::Language;
        use crate::i18n::I18n;

        let all = [
            Rejection::NotFound,
            Rejection::NotEmbeddable,
            Rejection::AgeRestricted,
            Rejection::LiveStream,
            Rejection::TooLong { max_secs: 185 },
        ];
        for language in [Language::Id, Language::En] {
            let i18n = I18n::load(language).unwrap();
            for rejection in all {
                let key = rejection.i18n_key();
                let args = rejection.i18n_args();
                let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
                let text = i18n.t_with(key, &args);
                assert_ne!(text, key, "missing {key} in {language:?}");
                assert!(!text.contains('{'), "unfilled placeholder in {text:?}");
            }
        }
        let text = I18n::load(Language::En)
            .unwrap()
            .t_with("reject.too_long", &[("max", "3:05")]);
        assert_eq!(text, "Video is longer than the 3:05 limit");
    }
}
