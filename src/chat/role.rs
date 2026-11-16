use serde::{Deserialize, Serialize};

/// Standing of a chat user, ordered from least to most privileged.
///
/// Each platform adapter maps its own badges onto this scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Everyone,
    Follower,
    Subscriber,
    Moderator,
}

impl Role {
    /// Whether a user with role `user` meets this minimum role.
    pub fn allows(self, user: Role) -> bool {
        user >= self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered() {
        assert!(Role::Everyone < Role::Follower);
        assert!(Role::Follower < Role::Subscriber);
        assert!(Role::Subscriber < Role::Moderator);
    }

    #[test]
    fn allows_checks_minimum() {
        assert!(Role::Follower.allows(Role::Follower));
        assert!(Role::Follower.allows(Role::Moderator));
        assert!(!Role::Follower.allows(Role::Everyone));
        assert!(Role::Everyone.allows(Role::Everyone));
        assert!(!Role::Moderator.allows(Role::Subscriber));
    }

    #[test]
    fn serde_uses_lowercase_names() {
        #[derive(Deserialize)]
        struct Wrap {
            role: Role,
        }
        let w: Wrap = toml::from_str("role = \"subscriber\"").unwrap();
        assert_eq!(w.role, Role::Subscriber);
        assert!(toml::from_str::<Wrap>("role = \"vip\"").is_err());
    }
}
