use super::role::Role;

/// Who sent a chat message, as far as moderation cares.
///
/// Each chat source fills this from its own data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sender {
    /// Stable unique ID of the account (not the display name, which can
    /// change). Cooldowns are tracked per `id`.
    pub id: String,
    /// Name shown to the streamer and on the overlay. Untrusted text.
    pub name: String,
    pub role: Role,
    /// TikTok gifter level; 0 when unknown or not applicable.
    pub gifter_level: u32,
    /// TikTok fan club level; 0 when unknown or not applicable.
    pub fan_club_level: u32,
}

impl Sender {
    /// A sender with no levels, mostly useful in tests.
    pub fn new(id: impl Into<String>, name: impl Into<String>, role: Role) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            role,
            gifter_level: 0,
            fan_club_level: 0,
        }
    }
}
