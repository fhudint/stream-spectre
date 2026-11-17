use super::error::ConfigError;
use super::model::{
    ChatSourceKind, CommandConfig, Config, DashboardConfig, MAX_CHAT_HISTORY, ModerationConfig,
    TiktokConfig,
};
use super::secrets::Secrets;
use crate::youtube::VideoId;

const MAX_TIKTOK_USERNAME_LEN: usize = 64;
const MAX_PREFIX_LEN: usize = 4;
const MAX_COMMAND_NAME_LEN: usize = 32;
/// More words than this is a typo or a mistake, not a setup.
const MAX_COMMANDS: usize = 8;

impl Config {
    /// Checks every value that serde cannot check by type alone.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.server.port == 0 {
            return Err(ConfigError::invalid(
                "server.port",
                "must be between 1 and 65535",
            ));
        }

        match self.chat.source {
            ChatSourceKind::Tiktok => self.chat.tiktok.validate()?,
        }

        self.command.validate()?;

        let y = &self.youtube;
        require_positive("youtube.cache_ttl_secs", y.cache_ttl_secs)?;
        require_positive("youtube.cache_max_entries", y.cache_max_entries as u64)?;
        require_positive("youtube.request_timeout_secs", y.request_timeout_secs)?;

        self.moderation.validate()?;
        self.dashboard.validate()?;

        let b = &self.blacklist;
        for (field, entries) in [
            ("blacklist.video_ids", &b.video_ids),
            ("blacklist.channels", &b.channels),
            ("blacklist.title_words", &b.title_words),
        ] {
            for (i, entry) in entries.iter().enumerate() {
                if entry.trim().is_empty() {
                    // An empty title word would match every title.
                    return Err(ConfigError::invalid(
                        format!("{field}[{i}]"),
                        "must not be empty",
                    ));
                }
            }
        }
        for (i, entry) in b.video_ids.iter().enumerate() {
            if VideoId::parse(entry.trim()).is_err() {
                return Err(ConfigError::invalid(
                    format!("blacklist.video_ids[{i}]"),
                    "must be an 11-character YouTube video ID, not a link",
                ));
            }
        }
        // Title words are matched as whole words; punctuation-only entries
        // contain no word and could never match.
        for (i, entry) in b.title_words.iter().enumerate() {
            if !entry.chars().any(char::is_alphanumeric) {
                return Err(ConfigError::invalid(
                    format!("blacklist.title_words[{i}]"),
                    "must contain at least one letter or digit",
                ));
            }
        }

        Ok(())
    }

    /// Checks combinations of config and secrets.
    pub fn validate_with_secrets(&self, secrets: &Secrets) -> Result<(), ConfigError> {
        if !self.server.bind.is_loopback() && secrets.dashboard_token.is_none() {
            return Err(ConfigError::invalid(
                "server.bind",
                "binding beyond loopback requires the DASHBOARD_TOKEN secret",
            ));
        }
        Ok(())
    }
}

impl ModerationConfig {
    /// Checks the moderation settings on their own, so they can also be
    /// validated when changed at runtime (for example from the dashboard).
    pub fn validate(&self) -> Result<(), ConfigError> {
        require_positive("moderation.max_duration_secs", self.max_duration_secs)?;
        require_positive("moderation.max_queue_len", self.max_queue_len as u64)?;
        if self.max_volume > 100 {
            return Err(ConfigError::invalid(
                "moderation.max_volume",
                "must be between 0 and 100",
            ));
        }
        if self.volume > self.max_volume {
            return Err(ConfigError::invalid(
                "moderation.volume",
                "must not exceed moderation.max_volume",
            ));
        }
        Ok(())
    }
}

impl DashboardConfig {
    /// Checks the dashboard settings on their own, so they can also be
    /// validated when changed at runtime.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.chat_history > MAX_CHAT_HISTORY {
            return Err(ConfigError::invalid(
                "dashboard.chat_history",
                format!("must be at most {MAX_CHAT_HISTORY}"),
            ));
        }
        Ok(())
    }
}

fn require_positive(field: &str, value: u64) -> Result<(), ConfigError> {
    if value == 0 {
        return Err(ConfigError::invalid(field, "must be greater than 0"));
    }
    Ok(())
}

impl TiktokConfig {
    /// Checks the username on its own, so it can also be validated when
    /// changed at runtime (from the dashboard). Empty is fine: it means no
    /// chat is read yet. Anything else must be a plausible TikTok username,
    /// which also keeps it safe to put in a URL.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let field = "chat.tiktok.username";
        let name = self.username();
        if name.is_empty() {
            // Only `@` (or spaces and `@`) typed: that is a mistake, not "none".
            if self.username.trim().is_empty() {
                return Ok(());
            }
            return Err(ConfigError::invalid(
                field,
                "must be a username, not just '@'",
            ));
        }
        if name.chars().count() > MAX_TIKTOK_USERNAME_LEN {
            return Err(ConfigError::invalid(
                field,
                format!("must be at most {MAX_TIKTOK_USERNAME_LEN} characters"),
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        {
            return Err(ConfigError::invalid(
                field,
                "may only contain letters, digits, '_' and '.'",
            ));
        }
        Ok(())
    }
}

impl CommandConfig {
    /// The prefix is short and has no whitespace; there are 1 to
    /// `MAX_COMMANDS` commands with valid names, no two the same when case is
    /// ignored (chat matches names that way).
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_prefix(&self.prefix)?;
        if self.commands.is_empty() || self.commands.len() > MAX_COMMANDS {
            return Err(ConfigError::invalid(
                "command.commands",
                format!("must have 1 to {MAX_COMMANDS} commands"),
            ));
        }
        for (i, entry) in self.commands.iter().enumerate() {
            validate_command_name(&format!("command.commands[{i}].name"), &entry.name)?;
            let repeats = self.commands[..i]
                .iter()
                .any(|earlier| earlier.name.eq_ignore_ascii_case(&entry.name));
            if repeats {
                return Err(ConfigError::invalid(
                    format!("command.commands[{i}].name"),
                    "is used twice (capital letters do not make a difference)",
                ));
            }
        }
        Ok(())
    }
}

fn validate_prefix(prefix: &str) -> Result<(), ConfigError> {
    let field = "command.prefix";
    if prefix.is_empty() || prefix.chars().count() > MAX_PREFIX_LEN {
        return Err(ConfigError::invalid(
            field,
            format!("must be 1 to {MAX_PREFIX_LEN} characters"),
        ));
    }
    if prefix.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ConfigError::invalid(field, "must not contain whitespace"));
    }
    Ok(())
}

fn validate_command_name(field: &str, name: &str) -> Result<(), ConfigError> {
    if name.is_empty() || name.chars().count() > MAX_COMMAND_NAME_LEN {
        return Err(ConfigError::invalid(
            field,
            format!("must be 1 to {MAX_COMMAND_NAME_LEN} characters"),
        ));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(ConfigError::invalid(
            field,
            "may only contain ASCII letters, digits and '_'",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;
    use crate::config::CommandEntry;

    fn valid() -> Config {
        let mut c = Config::default();
        c.chat.tiktok.username = "someone".to_owned();
        c
    }

    fn field_of(err: ConfigError) -> String {
        match err {
            ConfigError::Invalid { field, .. } => field,
            other => panic!("expected Invalid, got {other}"),
        }
    }

    #[test]
    fn default_config_with_username_is_valid() {
        valid().validate().unwrap();
    }

    #[test]
    fn no_username_is_allowed_and_means_no_chat_yet() {
        Config::default().validate().unwrap();
        for blank in ["", "   "] {
            let mut c = valid();
            c.chat.tiktok.username = blank.to_owned();
            c.validate().unwrap();
            assert_eq!(c.chat.tiktok.username(), "");
        }
    }

    #[test]
    fn username_is_normalized() {
        let mut c = valid();
        c.chat.tiktok.username = "  @some.one_1 ".to_owned();
        c.validate().unwrap();
        assert_eq!(c.chat.tiktok.username(), "some.one_1");
    }

    #[test]
    fn username_rejects_url_unsafe_characters() {
        for bad in [
            "a/b",
            "a b",
            "a?x=1",
            "a#b",
            "héllo",
            "@",
            " @ ",
            &"x".repeat(65),
        ] {
            let mut c = valid();
            c.chat.tiktok.username = bad.to_owned();
            assert_eq!(
                field_of(c.validate().unwrap_err()),
                "chat.tiktok.username",
                "{bad}"
            );
        }
    }

    #[test]
    fn chat_history_is_limited() {
        let mut c = valid();
        c.dashboard.chat_history = 0;
        c.validate().unwrap();
        c.dashboard.chat_history = MAX_CHAT_HISTORY;
        c.validate().unwrap();
        c.dashboard.chat_history = MAX_CHAT_HISTORY + 1;
        assert_eq!(
            field_of(c.validate().unwrap_err()),
            "dashboard.chat_history"
        );
    }

    #[test]
    fn port_zero_is_rejected() {
        let mut c = valid();
        c.server.port = 0;
        assert_eq!(field_of(c.validate().unwrap_err()), "server.port");
    }

    #[test]
    fn command_prefix_and_name_are_checked() {
        for bad in ["", "!!!!!", "! ", "\t"] {
            let mut c = valid();
            c.command.prefix = bad.to_owned();
            assert_eq!(
                field_of(c.validate().unwrap_err()),
                "command.prefix",
                "{bad:?}"
            );
        }
        for bad in ["", "two words", "v!", "vídeo", &"x".repeat(33)] {
            let mut c = valid();
            c.command.commands = vec![CommandEntry::youtube("ok"), CommandEntry::youtube(bad)];
            assert_eq!(
                field_of(c.validate().unwrap_err()),
                "command.commands[1].name",
                "{bad:?}"
            );
        }
        let mut c = valid();
        c.command.prefix = "#".to_owned();
        c.command.commands = vec![CommandEntry::youtube("play_video2")];
        c.validate().unwrap();
    }

    #[test]
    fn there_are_one_to_eight_commands_and_no_name_twice() {
        let mut c = valid();
        c.command.commands = vec![];
        assert_eq!(field_of(c.validate().unwrap_err()), "command.commands");
        c.command.commands = (0..9)
            .map(|n| CommandEntry::youtube(format!("c{n}")))
            .collect();
        assert_eq!(field_of(c.validate().unwrap_err()), "command.commands");
        c.command.commands.truncate(8);
        c.validate().unwrap();

        // Chat ignores capitals in the name, so these would be one command.
        c.command.commands = vec![CommandEntry::youtube("yt"), CommandEntry::youtube("YT")];
        assert_eq!(
            field_of(c.validate().unwrap_err()),
            "command.commands[1].name"
        );
        // A name that only starts like another is fine: names match whole.
        c.command.commands = vec![CommandEntry::youtube("v"), CommandEntry::youtube("vt")];
        c.validate().unwrap();
    }

    #[test]
    fn zero_limits_are_rejected() {
        type Mutate = fn(&mut Config);
        let cases: [(&str, Mutate); 5] = [
            ("youtube.cache_ttl_secs", |c| c.youtube.cache_ttl_secs = 0),
            ("youtube.cache_max_entries", |c| {
                c.youtube.cache_max_entries = 0
            }),
            ("youtube.request_timeout_secs", |c| {
                c.youtube.request_timeout_secs = 0
            }),
            ("moderation.max_duration_secs", |c| {
                c.moderation.max_duration_secs = 0
            }),
            ("moderation.max_queue_len", |c| {
                c.moderation.max_queue_len = 0
            }),
        ];
        for (field, mutate) in cases {
            let mut c = valid();
            mutate(&mut c);
            assert_eq!(field_of(c.validate().unwrap_err()), field);
        }
    }

    #[test]
    fn cooldown_and_levels_may_be_zero() {
        let mut c = valid();
        c.moderation.cooldown_secs = 0;
        c.moderation.min_gifter_level = 0;
        c.moderation.min_fan_club_level = 0;
        c.validate().unwrap();
    }

    #[test]
    fn volume_bounds_are_checked() {
        let mut c = valid();
        c.moderation.max_volume = 101;
        assert_eq!(field_of(c.validate().unwrap_err()), "moderation.max_volume");

        let mut c = valid();
        c.moderation.max_volume = 40;
        c.moderation.volume = 41;
        assert_eq!(field_of(c.validate().unwrap_err()), "moderation.volume");

        let mut c = valid();
        c.moderation.max_volume = 100;
        c.moderation.volume = 100;
        c.validate().unwrap();
    }

    #[test]
    fn title_words_need_a_letter_or_digit() {
        for bad in ["!!!", "-", "  ?  ", "…"] {
            let mut c = valid();
            c.blacklist.title_words = vec!["ok".to_owned(), bad.to_owned()];
            assert_eq!(
                field_of(c.validate().unwrap_err()),
                "blacklist.title_words[1]",
                "{bad}"
            );
        }
        let mut c = valid();
        c.blacklist.title_words = vec![
            "bad word".to_owned(),
            "x-rated".to_owned(),
            "日本".to_owned(),
        ];
        c.validate().unwrap();
    }

    #[test]
    fn moderation_settings_validate_on_their_own() {
        let mut m = ModerationConfig::default();
        m.validate().unwrap();
        m.volume = 90;
        assert_eq!(
            match m.validate().unwrap_err() {
                ConfigError::Invalid { field, .. } => field,
                other => panic!("{other}"),
            },
            "moderation.volume"
        );
    }

    #[test]
    fn blacklisted_video_ids_must_be_valid_ids() {
        for bad in ["short", "https://youtu.be/dQw4w9WgXcQ", "dQw4w9WgXc!"] {
            let mut c = valid();
            c.blacklist.video_ids = vec!["dQw4w9WgXcQ".to_owned(), bad.to_owned()];
            assert_eq!(
                field_of(c.validate().unwrap_err()),
                "blacklist.video_ids[1]",
                "{bad}"
            );
        }

        let mut c = valid();
        c.blacklist.video_ids = vec!["dQw4w9WgXcQ".to_owned(), "  a_b-C1d2E3f ".to_owned()];
        c.validate().unwrap();
    }

    #[test]
    fn blank_blacklist_entries_are_rejected() {
        let mut c = valid();
        c.blacklist.title_words = vec!["ok".to_owned(), "  ".to_owned()];
        assert_eq!(
            field_of(c.validate().unwrap_err()),
            "blacklist.title_words[1]"
        );

        let mut c = valid();
        c.blacklist.video_ids = vec![String::new()];
        assert_eq!(
            field_of(c.validate().unwrap_err()),
            "blacklist.video_ids[0]"
        );

        let mut c = valid();
        c.blacklist.channels = vec!["Some Channel".to_owned()];
        c.validate().unwrap();
    }

    fn secrets(token: Option<&str>) -> Secrets {
        Secrets::from_lookup(|k| match k {
            "YOUTUBE_API_KEY" => Some("key".to_owned()),
            "DASHBOARD_TOKEN" => token.map(str::to_owned),
            _ => None,
        })
        .unwrap()
    }

    #[test]
    fn non_loopback_bind_requires_dashboard_token() {
        let mut c = valid();
        c.validate_with_secrets(&secrets(None)).unwrap();

        c.server.bind = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        let err = c.validate_with_secrets(&secrets(None)).unwrap_err();
        assert_eq!(field_of(err), "server.bind");
        c.validate_with_secrets(&secrets(Some("0123456789abcdef")))
            .unwrap();

        c.server.bind = "::1".parse().unwrap();
        c.validate_with_secrets(&secrets(None)).unwrap();
    }
}
