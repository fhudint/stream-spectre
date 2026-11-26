use std::time::{Duration, Instant};

use super::blacklist::{Blacklist, BlacklistHit};
use super::cooldown::Cooldowns;
use super::item::TiktokDetails;
use crate::chat::{Role, Sender};
use crate::config::{ChatSourceKind, ModerationConfig};
use crate::i18n::I18n;
use crate::youtube::{Rejection, VideoDetails, VideoPolicy, check_video};

/// Why a request was turned down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    /// The sender's role is below the configured minimum.
    RoleTooLow {
        min: Role,
        /// The chat the sender wrote in, which has its own minimum.
        source: ChatSourceKind,
    },
    /// The gifter or fan club level is below the configured minimum.
    LevelTooLow,
    QueueFull,
    /// The chat the sender wrote in already has its share of the queue.
    SourceQueueFull {
        source: ChatSourceKind,
    },
    /// The sender is on the list of those who may not request anything.
    BlockedSender,
    /// A `youtube.com/shorts/` link, and Shorts are not allowed.
    ShortsNotAllowed,
    BlacklistedVideo,
    BlacklistedChannel,
    BlacklistedWord,
    /// The viewer's message has a banned word.
    BlacklistedMessage,
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
            Self::SourceQueueFull { .. } => "reject.source_queue_full",
            Self::BlockedSender => "reject.blocked_sender",
            Self::ShortsNotAllowed => "reject.shorts_not_allowed",
            Self::BlacklistedVideo => "reject.blacklisted_video",
            Self::BlacklistedChannel => "reject.blacklisted_channel",
            Self::BlacklistedWord => "reject.blacklisted_word",
            Self::BlacklistedMessage => "reject.blacklisted_message",
            Self::Cooldown { .. } => "reject.cooldown",
            Self::Video(rejection) => rejection.i18n_key(),
        }
    }

    /// Denials that are everyday noise rather than news for the moderators:
    /// viewers who may not submit (or are blocked), and viewers who submit too
    /// often.
    pub fn is_silent(&self) -> bool {
        matches!(
            self,
            Self::RoleTooLow { .. }
                | Self::LevelTooLow
                | Self::BlockedSender
                | Self::Cooldown { .. }
        )
    }

    /// Message for the sender or moderators, in the language of `i18n`.
    pub fn message(&self, i18n: &I18n) -> String {
        match self {
            Self::RoleTooLow { min, source } => {
                // A YouTube chat has its own words for the roles.
                let role_key = match (source, min) {
                    (_, Role::Everyone) => "role.everyone",
                    (ChatSourceKind::Youtube, Role::Follower) => "role.youtube.follower",
                    (ChatSourceKind::Youtube, Role::Subscriber) => "role.youtube.subscriber",
                    (ChatSourceKind::Youtube, Role::Moderator) => "role.youtube.moderator",
                    (_, Role::Follower) => "role.follower",
                    (_, Role::Subscriber) => "role.subscriber",
                    (_, Role::Moderator) => "role.moderator",
                };
                i18n.t_with("reject.role_too_low", &[("role", i18n.t(role_key))])
            }
            Self::LevelTooLow => i18n.t("reject.level_too_low").to_owned(),
            Self::QueueFull => i18n.t("reject.queue_full").to_owned(),
            Self::SourceQueueFull { source } => {
                let chat = match source {
                    ChatSourceKind::Youtube => i18n.t("dashboard.chat.source.youtube"),
                    ChatSourceKind::Tiktok => i18n.t("dashboard.chat.source.tiktok"),
                };
                i18n.t_with("reject.source_queue_full", &[("chat", chat)])
            }
            Self::BlockedSender => i18n.t("reject.blocked_sender").to_owned(),
            Self::ShortsNotAllowed => i18n.t("reject.shorts_not_allowed").to_owned(),
            Self::BlacklistedVideo => i18n.t("reject.blacklisted_video").to_owned(),
            Self::BlacklistedChannel => i18n.t("reject.blacklisted_channel").to_owned(),
            Self::BlacklistedWord => i18n.t("reject.blacklisted_word").to_owned(),
            Self::BlacklistedMessage => i18n.t("reject.blacklisted_message").to_owned(),
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

    /// Whether the sender may submit at all: not blocked, and with the role
    /// and levels asked of their chat.
    pub fn check_sender(&self, sender: &Sender) -> Result<(), Denial> {
        if self.blacklist.check_sender(sender) {
            return Err(Denial::BlockedSender);
        }
        check_sender(&self.settings, sender)
    }

    /// Whether the chat the sender wrote in still has room in the queue:
    /// `waiting_from_chat` of its requests are waiting or queued.
    pub fn check_share(&self, sender: &Sender, waiting_from_chat: usize) -> Result<(), Denial> {
        let most = self.settings.max_waiting_for(sender.source);
        if most > 0 && waiting_from_chat >= most {
            return Err(Denial::SourceQueueFull {
                source: sender.source,
            });
        }
        Ok(())
    }

    /// Whether requests with a `shorts` link may be made.
    pub fn check_shorts(&self, short: bool) -> Result<(), Denial> {
        if short && !self.settings.allow_shorts {
            return Err(Denial::ShortsNotAllowed);
        }
        Ok(())
    }

    /// Stage 1, before the YouTube lookup.
    ///
    /// Checks run in this order and the first failure wins; the cooldown is
    /// only started once everything else has passed.
    pub fn admit(
        &mut self,
        sender: &Sender,
        video: &(impl AsRef<str> + ?Sized),
        waiting_in_queue: usize,
        now: Instant,
    ) -> Result<(), Denial> {
        self.check_sender(sender)?;
        if waiting_in_queue >= self.settings.max_queue_len {
            return Err(Denial::QueueFull);
        }
        if let Some(hit) = self.blacklist.check_video_id(video) {
            return Err(hit.into());
        }
        self.cooldowns
            .try_acquire(&sender.id, self.settings.cooldown_for(sender.source), now)
            .map_err(|remaining| Denial::Cooldown { remaining })
    }

    /// `admit` for a request that comes with a message: the sender's rights
    /// are looked at first, then the words in the message, and only then the
    /// rest, so a refused message does not start the cooldown.
    pub fn admit_with_message(
        &mut self,
        sender: &Sender,
        video: &(impl AsRef<str> + ?Sized),
        message: &str,
        waiting_in_queue: usize,
        now: Instant,
    ) -> Result<(), Denial> {
        self.check_sender(sender)?;
        if self.blacklist.check_message(message) {
            return Err(Denial::BlacklistedMessage);
        }
        self.admit(sender, video, waiting_in_queue, now)
    }

    /// Gives the sender their cooldown back.
    pub fn refund(&mut self, sender: &Sender) {
        self.cooldowns.refund(&sender.id);
    }

    /// Stage 2 for a TikTok video: the blacklist is all there is to check,
    /// because TikTok tells nothing about length, age or embedding.
    pub fn vet_tiktok(&self, video: &TiktokDetails) -> Result<(), Denial> {
        match self.blacklist.check_tiktok(video) {
            Some(hit) => Err(hit.into()),
            None => Ok(()),
        }
    }

    /// Stage 2, with the looked-up video.
    pub fn vet(&self, video: &VideoDetails) -> Result<(), Denial> {
        self.vet_by(
            video,
            self.settings.max_duration_secs,
            &self.settings.allowed_categories,
        )
    }

    /// Stage 2 for a song: its own longest length and categories.
    pub fn vet_song(&self, video: &VideoDetails) -> Result<(), Denial> {
        self.vet_by(
            video,
            self.settings.song_max_duration_secs,
            &self.settings.song_allowed_categories,
        )
    }

    fn vet_by(
        &self,
        video: &VideoDetails,
        max_duration_secs: u64,
        categories: &[crate::config::VideoCategory],
    ) -> Result<(), Denial> {
        let policy = VideoPolicy {
            max_duration_secs,
            allow_live: self.settings.allow_live,
            allowed_categories: categories.iter().map(|category| category.id()).collect(),
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
/// The role must reach the minimum of the chat the sender wrote in
/// (`min_role` for TikTok, `youtube_min_role` for YouTube). On TikTok the
/// gifter and fan club levels must each reach their minimum (0 disables a
/// check); a YouTube chat has no such levels, so they are not asked of it. All
/// configured requirements apply together. Moderators are exempt from the
/// level requirements.
fn check_sender(settings: &ModerationConfig, sender: &Sender) -> Result<(), Denial> {
    let min = match sender.source {
        ChatSourceKind::Tiktok => settings.tiktok_min_role,
        ChatSourceKind::Youtube => settings.youtube_min_role.role(),
    };
    // Paying for a Super Chat is a way in, when the streamer says so.
    let paid_in = sender.paid && settings.youtube_superchat_bypass;
    if !paid_in && !min.allows(sender.role) {
        return Err(Denial::RoleTooLow {
            min,
            source: sender.source,
        });
    }
    if sender.role >= Role::Moderator || sender.source != ChatSourceKind::Tiktok {
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
    use crate::config::{BlacklistConfig, Language, YoutubeRole};
    use crate::youtube::{LiveStatus, VideoId};

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
            category_id: 10,
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
                min: Role::Follower,
                source: ChatSourceKind::Tiktok,
            })
        );
    }

    #[test]
    fn min_role_can_be_raised_or_dropped() {
        let now = Instant::now();
        let viewer = Sender::new("v", "Vic", Role::Everyone);
        let sub = Sender::new("s", "Sue", Role::Subscriber);

        let mut open = gatekeeper(ModerationConfig {
            tiktok_min_role: Role::Everyone,
            ..settings()
        });
        assert_eq!(open.admit(&viewer, &id(), 0, now), Ok(()));

        let mut subs_only = gatekeeper(ModerationConfig {
            tiktok_min_role: Role::Subscriber,
            ..settings()
        });
        assert_eq!(
            subs_only.admit(&follower(), &id(), 0, now),
            Err(Denial::RoleTooLow {
                min: Role::Subscriber,
                source: ChatSourceKind::Tiktok,
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
            tiktok_min_role: Role::Everyone,
            ..settings()
        });
        assert_eq!(g.settings().tiktok_min_role, Role::Everyone);
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
            senders: vec![],
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

    #[test]
    fn each_chat_has_its_own_minimum_role() {
        let now = Instant::now();
        let mut g = gatekeeper(ModerationConfig {
            tiktok_min_role: Role::Follower,
            youtube_min_role: YoutubeRole::Member,
            cooldown_secs: 0,
            ..settings()
        });
        let tiktok_follower = Sender::new("t", "Tia", Role::Follower);
        let youtube_viewer = Sender::new("y", "Yan", Role::Follower).on(ChatSourceKind::Youtube);
        let youtube_member = Sender::new("m", "Mia", Role::Subscriber).on(ChatSourceKind::Youtube);
        assert_eq!(g.admit(&tiktok_follower, &id(), 0, now), Ok(()));
        assert_eq!(
            g.admit(&youtube_viewer, &id(), 0, now),
            Err(Denial::RoleTooLow {
                min: Role::Subscriber,
                source: ChatSourceKind::Youtube,
            })
        );
        assert_eq!(g.admit(&youtube_member, &id(), 0, now), Ok(()));

        // And the other way round: TikTok can ask more than YouTube does.
        let mut g = gatekeeper(ModerationConfig {
            tiktok_min_role: Role::Subscriber,
            youtube_min_role: YoutubeRole::Viewer,
            cooldown_secs: 0,
            ..settings()
        });
        assert!(g.admit(&tiktok_follower, &id(), 0, now).is_err());
        assert_eq!(g.admit(&youtube_viewer, &id(), 0, now), Ok(()));
    }

    #[test]
    fn the_tiktok_levels_are_not_asked_of_a_youtube_viewer() {
        let now = Instant::now();
        let mut g = gatekeeper(ModerationConfig {
            min_gifter_level: 5,
            min_fan_club_level: 2,
            cooldown_secs: 0,
            ..settings()
        });
        let youtube_viewer = Sender::new("y", "Yan", Role::Follower).on(ChatSourceKind::Youtube);
        assert_eq!(g.admit(&youtube_viewer, &id(), 0, now), Ok(()));
        let tiktok_viewer = Sender::new("t", "Tia", Role::Follower);
        assert_eq!(
            g.admit(&tiktok_viewer, &id(), 0, now),
            Err(Denial::LevelTooLow)
        );
    }

    #[test]
    fn a_blocked_sender_is_refused_by_name_or_by_account_and_never_gets_a_cooldown() {
        let now = Instant::now();
        let blacklist = Blacklist::from_config(&BlacklistConfig {
            senders: vec!["Bad  Viewer!".to_owned(), "youtube:UCbad".to_owned()],
            ..BlacklistConfig::default()
        })
        .unwrap();
        let mut g = Gatekeeper::new(settings(), blacklist);
        let by_name = Sender::new("t-1", "bad viewer", Role::Follower);
        let by_id = Sender::new("youtube:UCbad", "Someone Else", Role::Subscriber)
            .on(ChatSourceKind::Youtube);
        let fine = Sender::new("t-2", "Good Viewer", Role::Follower);
        assert_eq!(g.admit(&by_name, &id(), 0, now), Err(Denial::BlockedSender));
        assert_eq!(g.admit(&by_id, &id(), 0, now), Err(Denial::BlockedSender));
        assert_eq!(g.check_sender(&by_name), Err(Denial::BlockedSender));
        assert_eq!(g.admit(&fine, &id(), 0, now), Ok(()));
        // Not even a moderator gets past the list.
        let moderator = Sender::new("t-3", "Bad Viewer", Role::Moderator);
        assert_eq!(
            g.admit(&moderator, &id(), 0, now),
            Err(Denial::BlockedSender)
        );
    }

    #[test]
    fn a_chat_cannot_take_more_than_its_share_of_the_queue() {
        let g = gatekeeper(ModerationConfig {
            youtube_max_waiting: 2,
            ..settings()
        });
        let youtube = Sender::new("y", "Yan", Role::Follower).on(ChatSourceKind::Youtube);
        assert_eq!(g.check_share(&youtube, 1), Ok(()));
        assert_eq!(
            g.check_share(&youtube, 2),
            Err(Denial::SourceQueueFull {
                source: ChatSourceKind::Youtube
            })
        );
        // The other chat has no limit of its own, and 0 is no limit.
        assert_eq!(g.check_share(&follower(), 500), Ok(()));
    }

    #[test]
    fn each_chat_can_have_its_own_cooldown() {
        let now = Instant::now();
        let mut g = gatekeeper(ModerationConfig {
            cooldown_secs: 60,
            youtube_cooldown_secs: Some(0),
            tiktok_cooldown_secs: None,
            ..settings()
        });
        let youtube = Sender::new("youtube:UC1", "Yan", Role::Follower).on(ChatSourceKind::Youtube);
        assert_eq!(g.admit(&youtube, &id(), 0, now), Ok(()));
        assert_eq!(g.admit(&youtube, &id(), 0, now), Ok(()), "YouTube has none");
        let tiktok = follower();
        assert_eq!(g.admit(&tiktok, &id(), 0, now), Ok(()));
        assert!(
            matches!(
                g.admit(&tiktok, &id(), 0, now),
                Err(Denial::Cooldown { .. })
            ),
            "TikTok keeps the general one"
        );
        let own = ModerationConfig {
            cooldown_secs: 0,
            tiktok_cooldown_secs: Some(30),
            ..settings()
        };
        assert_eq!(
            own.cooldown_for(ChatSourceKind::Tiktok),
            Duration::from_secs(30)
        );
        assert_eq!(own.cooldown_for(ChatSourceKind::Youtube), Duration::ZERO);
    }

    #[test]
    fn a_super_chat_gets_past_the_role_only_when_the_streamer_says_so() {
        let now = Instant::now();
        let viewer = Sender::new("y", "Yan", Role::Follower).on(ChatSourceKind::Youtube);
        let rich = viewer.clone().paid();
        let members_only = ModerationConfig {
            youtube_min_role: YoutubeRole::Member,
            cooldown_secs: 0,
            ..settings()
        };
        let mut closed = gatekeeper(members_only.clone());
        assert!(
            closed.admit(&rich, &id(), 0, now).is_err(),
            "off by default"
        );
        let mut open = gatekeeper(ModerationConfig {
            youtube_superchat_bypass: true,
            ..members_only
        });
        assert_eq!(open.admit(&rich, &id(), 0, now), Ok(()));
        assert!(
            open.admit(&viewer, &id(), 0, now).is_err(),
            "an ordinary chat still is not"
        );
    }

    #[test]
    fn shorts_links_are_refused_only_when_shorts_are_not_allowed() {
        let allowed = gatekeeper(settings());
        assert_eq!(allowed.check_shorts(true), Ok(()));
        let refused = gatekeeper(ModerationConfig {
            allow_shorts: false,
            ..settings()
        });
        assert_eq!(refused.check_shorts(true), Err(Denial::ShortsNotAllowed));
        assert_eq!(refused.check_shorts(false), Ok(()));
    }

    #[test]
    fn vet_applies_the_live_and_category_rules_of_the_settings() {
        let mut live = details();
        live.live = LiveStatus::Live;
        live.duration_secs = 0;
        assert_eq!(
            gatekeeper(settings()).vet(&live),
            Err(Denial::Video(Rejection::LiveStream))
        );
        let g = gatekeeper(ModerationConfig {
            allow_live: true,
            ..settings()
        });
        assert_eq!(g.vet(&live), Ok(()));

        let mut music = details();
        music.category_id = 10;
        let g = gatekeeper(ModerationConfig {
            allowed_categories: vec![crate::config::VideoCategory::Gaming],
            ..settings()
        });
        assert_eq!(
            g.vet(&music),
            Err(Denial::Video(Rejection::CategoryNotAllowed))
        );
        music.category_id = 20;
        assert_eq!(g.vet(&music), Ok(()));
    }

    #[test]
    fn the_role_message_uses_the_words_of_the_chat() {
        let i18n = I18n::load(Language::En).unwrap();
        let youtube = Denial::RoleTooLow {
            min: Role::Subscriber,
            source: ChatSourceKind::Youtube,
        };
        assert_eq!(
            youtube.message(&i18n),
            "Only Member and above can submit videos"
        );
        let tiktok = Denial::RoleTooLow {
            min: Role::Subscriber,
            source: ChatSourceKind::Tiktok,
        };
        assert_eq!(
            tiktok.message(&i18n),
            "Only Subscriber and above can submit videos"
        );
    }

    // --- messages ---

    #[test]
    fn every_denial_has_a_filled_in_message_in_both_languages() {
        let denials = [
            Denial::RoleTooLow {
                min: Role::Subscriber,
                source: ChatSourceKind::Tiktok,
            },
            Denial::LevelTooLow,
            Denial::QueueFull,
            Denial::SourceQueueFull {
                source: ChatSourceKind::Youtube,
            },
            Denial::SourceQueueFull {
                source: ChatSourceKind::Tiktok,
            },
            Denial::BlockedSender,
            Denial::ShortsNotAllowed,
            Denial::Video(Rejection::CategoryNotAllowed),
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
                    source: ChatSourceKind::Tiktok,
                },
                true,
            ),
            (Denial::LevelTooLow, true),
            (Denial::BlockedSender, true),
            (
                Denial::SourceQueueFull {
                    source: ChatSourceKind::Youtube,
                },
                false,
            ),
            (Denial::ShortsNotAllowed, false),
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
                min: Role::Follower,
                source: ChatSourceKind::Tiktok,
            })
        );
        assert_eq!(g.check_sender(&follower()), Ok(()));
    }

    #[test]
    fn messages_include_the_translated_role_and_rounded_up_seconds() {
        let en = I18n::load(Language::En).unwrap();
        assert_eq!(
            Denial::RoleTooLow {
                min: Role::Subscriber,
                source: ChatSourceKind::Tiktok,
            }
            .message(&en),
            "Only Subscriber and above can submit videos"
        );
        let id = I18n::load(Language::Id).unwrap();
        assert_eq!(
            Denial::RoleTooLow {
                min: Role::Everyone,
                source: ChatSourceKind::Tiktok,
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
