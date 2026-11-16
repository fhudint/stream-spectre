use super::error::ConfigError;
use super::model::{ChatSourceKind, Config};
use super::secrets::Secrets;

const MAX_TIKTOK_USERNAME_LEN: usize = 64;
const MAX_PREFIX_LEN: usize = 4;
const MAX_COMMAND_NAME_LEN: usize = 32;

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
            ChatSourceKind::Tiktok => validate_tiktok_username(self.chat.tiktok.username())?,
        }

        validate_prefix(&self.command.prefix)?;
        validate_command_name(&self.command.name)?;

        let y = &self.youtube;
        require_positive("youtube.cache_ttl_secs", y.cache_ttl_secs)?;
        require_positive("youtube.cache_max_entries", y.cache_max_entries as u64)?;
        require_positive("youtube.request_timeout_secs", y.request_timeout_secs)?;

        let m = &self.moderation;
        require_positive("moderation.max_duration_secs", m.max_duration_secs)?;
        require_positive("moderation.max_queue_len", m.max_queue_len as u64)?;
        if m.max_volume > 100 {
            return Err(ConfigError::invalid(
                "moderation.max_volume",
                "must be between 0 and 100",
            ));
        }
        if m.volume > m.max_volume {
            return Err(ConfigError::invalid(
                "moderation.volume",
                "must not exceed moderation.max_volume",
            ));
        }

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

fn require_positive(field: &str, value: u64) -> Result<(), ConfigError> {
    if value == 0 {
        return Err(ConfigError::invalid(field, "must be greater than 0"));
    }
    Ok(())
}

fn validate_tiktok_username(name: &str) -> Result<(), ConfigError> {
    let field = "chat.tiktok.username";
    if name.is_empty() {
        return Err(ConfigError::invalid(
            field,
            "must be set when chat.source = \"tiktok\"",
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

fn validate_command_name(name: &str) -> Result<(), ConfigError> {
    let field = "command.name";
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
    fn username_is_required_and_normalized() {
        assert_eq!(
            field_of(Config::default().validate().unwrap_err()),
            "chat.tiktok.username"
        );

        let mut c = valid();
        c.chat.tiktok.username = "  @some.one_1 ".to_owned();
        c.validate().unwrap();
        assert_eq!(c.chat.tiktok.username(), "some.one_1");
    }

    #[test]
    fn username_rejects_url_unsafe_characters() {
        for bad in ["a/b", "a b", "a?x=1", "a#b", "héllo", "@", &"x".repeat(65)] {
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
            c.command.name = bad.to_owned();
            assert_eq!(
                field_of(c.validate().unwrap_err()),
                "command.name",
                "{bad:?}"
            );
        }
        let mut c = valid();
        c.command.prefix = "#".to_owned();
        c.command.name = "play_video2".to_owned();
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
