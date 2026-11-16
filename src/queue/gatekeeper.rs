use std::time::{Duration, Instant};

use super::blacklist::{Blacklist, BlacklistHit};
use super::cooldown::Cooldowns;
use crate::chat::{Role, Sender};
use crate::config::ModerationConfig;
use crate::i18n::I18n;
use crate::youtube::{Rejection, VideoDetails, VideoId, VideoPolicy, check_video};

/// Why a request was turned down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    /// The sender's role is below the configured minimum.
    RoleTooLow {
        min: Role,
    },
    /// The gifter or fan club level is below the configured minimum.
    LevelTooLow,
    QueueFull,
    BlacklistedVideo,
    BlacklistedChannel,
    BlacklistedWord,
    /// The sender must wait before submitting again.
    Cooldown {
        remaining: Duration,
    },
    /// The video itself is not acceptable.
    Video(Rejection),
}

impl Denial {
    /// Key of this denial's message in the locale catalogs.
    pub fn i18n_key(&self) -> &'static str {
        match self {
            Self::RoleTooLow { .. } => "reject.role_too_low",
            Self::LevelTooLow => "reject.level_too_low",
            Self::QueueFull => "reject.queue_full",
            Self::BlacklistedVideo => "reject.blacklisted_video",
            Self::BlacklistedChannel => "reject.blacklisted_channel",
            Self::BlacklistedWord => "reject.blacklisted_word",
            Self::Cooldown { .. } => "reject.cooldown",
            Self::Video(rejection) => rejection.i18n_key(),
        }
    }

    /// Denials that are everyday noise rather than news for the moderators:
    /// viewers who may not submit, and viewers who submit too often.
    pub fn is_silent(&self) -> bool {
        matches!(
            self,
            Self::RoleTooLow { .. } | Self::LevelTooLow | Self::Cooldown { .. }
        )
    }

    /// Message for the sender or moderators, in the language of `i18n`.
    pub fn message(&self, i18n: &I18n) -> String {
        match self {
            Self::RoleTooLow { min } => {
                let role_key = match min {
                    Role::Everyone => "role.everyone",
                    Role::Follower => "role.follower",
                    Role::Subscriber => "role.subscriber",
                    Role::Moderator => "role.moderator",
                };
                i18n.t_with("reject.role_too_low", &[("role", i18n.t(role_key))])
            }
            Self::LevelTooLow => i18n.t("reject.level_too_low").to_owned(),
            Self::QueueFull => i18n.t("reject.queue_full").to_owned(),
            Self::BlacklistedVideo => i18n.t("reject.blacklisted_video").to_owned(),
            Self::BlacklistedChannel => i18n.t("reject.blacklisted_channel").to_owned(),
            Self::BlacklistedWord => i18n.t("reject.blacklisted_word").to_owned(),
            Self::Cooldown { remaining } => {
                // Round up so the viewer never retries too early.
                let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
                i18n.t_with("reject.cooldown", &[("seconds", &seconds.to_string())])
            }
            Self::Video(rejection) => {
                let args = rejection.i18n_args();
                let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
                i18n.t_with(rejection.i18n_key(), &args)
            }
        }
    }
}

impl From<BlacklistHit> for Denial {
    fn from(hit: BlacklistHit) -> Self {
        match hit {
            BlacklistHit::Video => Self::BlacklistedVideo,
            BlacklistHit::Channel => Self::BlacklistedChannel,
            BlacklistHit::TitleWord => Self::BlacklistedWord,
        }
    }
}

/// Applies the moderation rules to incoming requests.
///
/// A request passes through two stages around the YouTube lookup:
///
/// 1. `admit`: everything that can be decided without a lookup (sender,
///    queue space, blacklisted ID, cooldown), so abusers cannot spend API
///    quota.
/// 2. `vet`: rules that need the video's details (duration, embeddable,
///    age restriction, blacklisted channel or title word).
///
/// The cooldown starts at `admit`, so it is spent whether or not the video
/// turns out to be acceptable. Call `refund` when the lookup failed through
/// no fault of the sender.
#[derive(Debug)]
pub struct Gatekeeper {
    settings: ModerationConfig,
    blacklist: Blacklist,
    cooldowns: Cooldowns,
}

impl Gatekeeper {
    pub fn new(settings: ModerationConfig, blacklist: Blacklist) -> Self {
        Self {
            settings,
            blacklist,
            cooldowns: Cooldowns::new(),
        }
    }

    pub fn settings(&self) -> &ModerationConfig {
        &self.settings
    }

    /// Replaces the settings; cooldowns that are running keep running.
    pub fn set_settings(&mut self, settings: ModerationConfig) {
        self.settings = settings;
    }

    pub fn set_blacklist(&mut self, blacklist: Blacklist) {
        self.blacklist = blacklist;
    }

    /// Whether the sender's role and levels allow submitting at all.
    pub fn check_sender(&self, sender: &Sender) -> Result<(), Denial> {
        check_sender(&self.settings, sender)
    }

    /// Stage 1, before the YouTube lookup.
    ///
    /// Checks run in this order and the first failure wins; the cooldown is
    /// only started once everything else has passed.
    pub fn admit(
        &mut self,
        sender: &Sender,
        video: &VideoId,
        waiting_in_queue: usize,
        now: Instant,
    ) -> Result<(), Denial> {
        check_sender(&self.settings, sender)?;
        if waiting_in_queue >= self.settings.max_queue_len {
            return Err(Denial::QueueFull);
        }
        if let Some(hit) = self.blacklist.check_video_id(video) {
            return Err(hit.into());
        }
        self.cooldowns
            .try_acquire(&sender.id, self.settings.cooldown(), now)
            .map_err(|remaining| Denial::Cooldown { remaining })
    }

    /// Gives the sender their cooldown back.
    pub fn refund(&mut self, sender: &Sender) {
        self.cooldowns.refund(&sender.id);
    }

    /// Stage 2, with the looked-up video.
    pub fn vet(&self, video: &VideoDetails) -> Result<(), Denial> {
        let policy = VideoPolicy {
            max_duration_secs: self.settings.max_duration_secs,
        };
        check_video(video, &policy).map_err(Denial::Video)?;
        match self.blacklist.check_details(video) {
            Some(hit) => Err(hit.into()),
            None => Ok(()),
        }
    }
}

/// Role and level requirements.
///
/// The role must reach `min_role`. The gifter and fan club levels must each
/// reach their minimum (0 disables a check); all configured requirements
/// apply together. Moderators are exempt from the level requirements.
fn check_sender(settings: &ModerationConfig, sender: &Sender) -> Result<(), Denial> {
    if !settings.min_role.allows(sender.role) {
        return Err(Denial::RoleTooLow {
            min: settings.min_role,
        });
    }
    if sender.role >= Role::Moderator {
        return Ok(());
    }
    if sender.gifter_level < settings.min_gifter_level
        || sender.fan_club_level < settings.min_fan_club_level
    {
        return Err(Denial::LevelTooLow);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BlacklistConfig, Language};
    use crate::youtube::LiveStatus;

    const OK_ID: &str = "dQw4w9WgXcQ";

    fn id() -> VideoId {
        VideoId::parse(OK_ID).unwrap()
    }

    fn follower() -> Sender {
        Sender::new("u-follower", "Fanny", Role::Follower)
    }

    fn settings() -> ModerationConfig {
        ModerationConfig::default()
    }

    fn gatekeeper(settings: ModerationConfig) -> Gatekeeper {
        Gatekeeper::new(settings, Blacklist::default())
    }

    fn details() -> VideoDetails {
        VideoDetails {
            id: id(),
            title: "A nice song".to_owned(),
            channel: "Some Channel".to_owned(),
            duration_secs: 200,
            embeddable: true,
            age_restricted: false,
            live: LiveStatus::None,
            private: false,
        }
    }

    // --- sender requirements ---

    #[test]
    fn default_settings_admit_a_follower_but_not_a_plain_viewer() {
        let mut g = gatekeeper(settings());
        let now = Instant::now();
        assert_eq!(g.admit(&follower(), &id(), 0, now), Ok(()));

        let viewer = Sender::new("u-viewer", "Vic", Role::Everyone);
        assert_eq!(
            g.admit(&viewer, &id(), 0, now),
            Err(Denial::RoleTooLow {
                min: Role::Follower
            })
        );
    }

    #[test]
    fn min_role_can_be_raised_or_dropped() {
        let now = Instant::now();
        let viewer = Sender::new("v", "Vic", Role::Everyone);
        let sub = Sender::new("s", "Sue", Role::Subscriber);

        let mut open = gatekeeper(ModerationConfig {
            min_role: Role::Everyone,
            ..settings()
        });
        assert_eq!(open.admit(&viewer, &id(), 0, now), Ok(()));

        let mut subs_only = gatekeeper(ModerationConfig {
            min_role: Role::Subscriber,
            ..settings()
        });
        assert_eq!(
            subs_only.admit(&follower(), &id(), 0, now),
            Err(Denial::RoleTooLow {
                min: Role::Subscriber
            })
        );
        assert_eq!(subs_only.admit(&sub, &id(), 0, now), Ok(()));
    }

    #[test]
    fn gifter_and_fan_club_levels_must_both_be_met() {
        let now = Instant::now();
        let s = ModerationConfig {
            min_gifter_level: 5,
            min_fan_club_level: 2,
            cooldown_secs: 0,
            ..settings()
        };
        let mut g = gatekeeper(s);

        let mut user = follower();
        user.gifter_level = 5;
        user.fan_club_level = 2;
        assert_eq!(g.admit(&user, &id(), 0, now), Ok(()));

        user.gifter_level = 4;
        assert_eq!(g.admit(&user, &id(), 0, now), Err(Denial::LevelTooLow));
        user.gifter_level = 9;
        user.fan_club_level = 1;
        assert_eq!(g.admit(&user, &id(), 0, now), Err(Denial::LevelTooLow));
    }

    #[test]
    fn a_zero_level_minimum_turns_that_check_off() {
        let now = Instant::now();
        let mut g = gatekeeper(ModerationConfig {
            min_gifter_level: 0,
            min_fan_club_level: 3,
            cooldown_secs: 0,
            ..settings()
        });
        let mut user = follower();
        user.fan_club_level = 3;
        user.gifter_level = 0;
        assert_eq!(g.admit(&user, &id(), 0, now), Ok(()));
    }

    #[test]
    fn moderators_skip_the_level_requirements_but_not_the_role_check() {
        let now = Instant::now();
        let mut g = gatekeeper(ModerationConfig {
            min_gifter_level: 50,
            min_fan_club_level: 50,
            ..settings()
        });
        let moderator = Sender::new("m", "Mo", Role::Moderator);
        assert_eq!(g.admit(&moderator, &id(), 0, now), Ok(()));
    }

    // --- queue space, blacklist and cooldown ---

    #[test]
    fn a_full_queue_is_refused_before_the_cooldown_starts() {
        let now = Instant::now();
        let mut g = gatekeeper(ModerationConfig {
            max_queue_len: 3,
            ..settings()
        });
        assert_eq!(g.admit(&follower(), &id(), 3, now), Err(Denial::QueueFull));
        // No cooldown was spent, so the same sender can try again at once.
        assert_eq!(g.admit(&follower(), &id(), 2, now), Ok(()));
    }

    #[test]
    fn blacklisted_ids_are_refused_before_the_cooldown_starts() {
        let now = Instant::now();
        let blacklist = Blacklist::from_config(&BlacklistConfig {
            video_ids: vec![OK_ID.to_owned()],
            ..BlacklistConfig::default()
        })
        .unwrap();
        let mut g = Gatekeeper::new(settings(), blacklist);
        assert_eq!(
            g.admit(&follower(), &id(), 0, now),
            Err(Denial::BlacklistedVideo)
        );
        let other = VideoId::parse("aaaaaaaaaaa").unwrap();
        assert_eq!(g.admit(&follower(), &other, 0, now), Ok(()));
    }

    #[test]
    fn the_cooldown_starts_at_admit_and_reports_the_remaining_time() {
        let t0 = Instant::now();
        let mut g = gatekeeper(ModerationConfig {
            cooldown_secs: 60,
            ..settings()
        });
        assert_eq!(g.admit(&follower(), &id(), 0, t0), Ok(()));
        assert_eq!(
            g.admit(&follower(), &id(), 0, t0 + Duration::from_secs(10)),
            Err(Denial::Cooldown {
                remaining: Duration::from_secs(50)
            })
        );
        assert_eq!(
            g.admit(&follower(), &id(), 0, t0 + Duration::from_secs(60)),
            Ok(())
        );
    }

    #[test]
    fn cooldowns_are_per_user_id_not_per_display_name() {
        let t0 = Instant::now();
        let mut g = gatekeeper(settings());
        let a = Sender::new("id-1", "Same Name", Role::Follower);
        let b = Sender::new("id-2", "Same Name", Role::Follower);
        let a_renamed = Sender::new("id-1", "New Name", Role::Follower);
        assert_eq!(g.admit(&a, &id(), 0, t0), Ok(()));
        assert_eq!(g.admit(&b, &id(), 0, t0), Ok(()));
        assert!(g.admit(&a_renamed, &id(), 0, t0).is_err());
    }

    #[test]
    fn refund_gives_the_cooldown_back() {
        let t0 = Instant::now();
        let mut g = gatekeeper(settings());
        g.admit(&follower(), &id(), 0, t0).unwrap();
        g.refund(&follower());
        assert_eq!(g.admit(&follower(), &id(), 0, t0), Ok(()));
    }

    #[test]
    fn a_zero_cooldown_allows_back_to_back_requests() {
        let t0 = Instant::now();
        let mut g = gatekeeper(ModerationConfig {
            cooldown_secs: 0,
            ..settings()
        });
        for _ in 0..3 {
            assert_eq!(g.admit(&follower(), &id(), 0, t0), Ok(()));
        }
    }

    #[test]
    fn changed_settings_apply_to_later_requests() {
        let t0 = Instant::now();
        let mut g = gatekeeper(settings());
        let viewer = Sender::new("v", "Vic", Role::Everyone);
        assert!(g.admit(&viewer, &id(), 0, t0).is_err());
        g.set_settings(ModerationConfig {
            min_role: Role::Everyone,
            ..settings()
        });
        assert_eq!(g.settings().min_role, Role::Everyone);
        assert_eq!(g.admit(&viewer, &id(), 0, t0), Ok(()));
    }

    // --- vetting ---

    #[test]
    fn vet_accepts_a_normal_video() {
        assert_eq!(gatekeeper(settings()).vet(&details()), Ok(()));
    }

    #[test]
    fn vet_applies_the_video_policy_with_the_configured_duration_limit() {
        let g = gatekeeper(ModerationConfig {
            max_duration_secs: 120,
            ..settings()
        });
        assert_eq!(
            g.vet(&details()),
            Err(Denial::Video(Rejection::TooLong { max_secs: 120 }))
        );

        let mut not_embeddable = details();
        not_embeddable.embeddable = false;
        assert_eq!(
            gatekeeper(settings()).vet(&not_embeddable),
            Err(Denial::Video(Rejection::NotEmbeddable))
        );
    }

    #[test]
    fn vet_applies_channel_and_title_blacklists() {
        let blacklist = Blacklist::from_config(&BlacklistConfig {
            video_ids: vec![],
            channels: vec!["some channel".to_owned()],
            title_words: vec!["nasty".to_owned()],
        })
        .unwrap();
        let g = Gatekeeper::new(settings(), blacklist);
        assert_eq!(g.vet(&details()), Err(Denial::BlacklistedChannel));

        let mut other_channel = details();
        other_channel.channel = "Fine Channel".to_owned();
        assert_eq!(g.vet(&other_channel), Ok(()));
        other_channel.title = "Something NASTY inside".to_owned();
        assert_eq!(g.vet(&other_channel), Err(Denial::BlacklistedWord));
    }

    #[test]
    fn blacklist_updates_take_effect() {
        let mut g = gatekeeper(settings());
        assert_eq!(g.vet(&details()), Ok(()));
        g.set_blacklist(
            Blacklist::from_config(&BlacklistConfig {
                video_ids: vec![OK_ID.to_owned()],
                ..BlacklistConfig::default()
            })
            .unwrap(),
        );
        assert_eq!(g.vet(&details()), Err(Denial::BlacklistedVideo));
    }

    // --- messages ---

    #[test]
    fn every_denial_has_a_filled_in_message_in_both_languages() {
        let denials = [
            Denial::RoleTooLow {
                min: Role::Subscriber,
            },
            Denial::LevelTooLow,
            Denial::QueueFull,
            Denial::BlacklistedVideo,
            Denial::BlacklistedChannel,
            Denial::BlacklistedWord,
            Denial::Cooldown {
                remaining: Duration::from_millis(41_200),
            },
            Denial::Video(Rejection::NotFound),
            Denial::Video(Rejection::NotEmbeddable),
            Denial::Video(Rejection::AgeRestricted),
            Denial::Video(Rejection::LiveStream),
            Denial::Video(Rejection::TooLong { max_secs: 600 }),
        ];
        for language in [Language::Id, Language::En] {
            let i18n = I18n::load(language).unwrap();
            for denial in denials {
                let text = denial.message(&i18n);
                assert!(!text.is_empty(), "{denial:?}");
                assert!(!text.contains('{'), "unfilled placeholder: {text}");
                assert!(
                    !text.contains("reject.") && !text.contains("role."),
                    "missing catalog key for {denial:?} in {language:?}: {text}"
                );
            }
        }
    }

    #[test]
    fn keys_exist_in_both_catalogs_and_only_everyday_noise_is_silent() {
        let denials = [
            (
                Denial::RoleTooLow {
                    min: Role::Follower,
                },
                true,
            ),
            (Denial::LevelTooLow, true),
            (
                Denial::Cooldown {
                    remaining: Duration::from_secs(1),
                },
                true,
            ),
            (Denial::QueueFull, false),
            (Denial::BlacklistedVideo, false),
            (Denial::BlacklistedChannel, false),
            (Denial::BlacklistedWord, false),
            (Denial::Video(Rejection::NotEmbeddable), false),
            (Denial::Video(Rejection::TooLong { max_secs: 60 }), false),
        ];
        for language in [Language::Id, Language::En] {
            let i18n = I18n::load(language).unwrap();
            for (denial, silent) in denials {
                assert_ne!(
                    i18n.t(denial.i18n_key()),
                    denial.i18n_key(),
                    "{denial:?} {language:?}"
                );
                assert_eq!(denial.is_silent(), silent, "{denial:?}");
            }
        }
    }

    #[test]
    fn check_sender_is_available_without_starting_a_cooldown() {
        let g = gatekeeper(settings());
        let viewer = Sender::new("v", "Vic", Role::Everyone);
        assert_eq!(
            g.check_sender(&viewer),
            Err(Denial::RoleTooLow {
                min: Role::Follower
            })
        );
        assert_eq!(g.check_sender(&follower()), Ok(()));
    }

    #[test]
    fn messages_include_the_translated_role_and_rounded_up_seconds() {
        let en = I18n::load(Language::En).unwrap();
        assert_eq!(
            Denial::RoleTooLow {
                min: Role::Subscriber
            }
            .message(&en),
            "Only Subscriber and above can submit videos"
        );
        let id = I18n::load(Language::Id).unwrap();
        assert_eq!(
            Denial::RoleTooLow {
                min: Role::Everyone
            }
            .message(&id),
            "Hanya Semua orang ke atas yang boleh mengirim video"
        );
        assert_eq!(
            Denial::Cooldown {
                remaining: Duration::from_millis(41_200)
            }
            .message(&en),
            "Wait 42 seconds before submitting again"
        );
        assert_eq!(
            Denial::Cooldown {
                remaining: Duration::from_secs(5)
            }
            .message(&en),
            "Wait 5 seconds before submitting again"
        );
        assert_eq!(
            Denial::Video(Rejection::TooLong { max_secs: 600 }).message(&en),
            "Video is longer than the 10:00 limit"
        );
    }
}
