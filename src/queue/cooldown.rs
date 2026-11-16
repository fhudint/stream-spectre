use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Most users tracked at once. Old entries are dropped well before this in
/// practice; the cap only bounds memory against floods of fresh accounts.
const MAX_TRACKED_USERS: usize = 10_000;

/// Per-user cooldown between submissions.
///
/// Time is passed in by the caller, which keeps this deterministic to test.
#[derive(Debug, Default)]
pub struct Cooldowns {
    last_attempt: HashMap<String, Instant>,
}

impl Cooldowns {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a cooldown for `user` if none is running.
    ///
    /// Returns `Err(remaining)` while the previous cooldown is still running.
    /// A zero `cooldown` disables the feature and records nothing.
    pub fn try_acquire(
        &mut self,
        user: &str,
        cooldown: Duration,
        now: Instant,
    ) -> Result<(), Duration> {
        if cooldown.is_zero() {
            return Ok(());
        }
        if let Some(last) = self.last_attempt.get(user) {
            let ready_at = *last + cooldown;
            if now < ready_at {
                return Err(ready_at - now);
            }
        }
        if self.last_attempt.len() >= MAX_TRACKED_USERS {
            self.make_room(cooldown, now);
        }
        self.last_attempt.insert(user.to_owned(), now);
        Ok(())
    }

    /// Cancels the cooldown of `user`, for attempts that failed through no
    /// fault of theirs (for example when YouTube was unreachable).
    pub fn refund(&mut self, user: &str) {
        self.last_attempt.remove(user);
    }

    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.last_attempt.len()
    }

    /// Drops expired entries; if that is not enough, drops the oldest one.
    fn make_room(&mut self, cooldown: Duration, now: Instant) {
        self.last_attempt.retain(|_, last| now < *last + cooldown);
        if self.last_attempt.len() >= MAX_TRACKED_USERS {
            let oldest = self
                .last_attempt
                .iter()
                .min_by_key(|(_, last)| **last)
                .map(|(user, _)| user.clone());
            if let Some(user) = oldest {
                self.last_attempt.remove(&user);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COOLDOWN: Duration = Duration::from_secs(60);

    #[test]
    fn first_attempt_passes_and_second_is_blocked_with_remaining_time() {
        let mut c = Cooldowns::new();
        let t0 = Instant::now();
        assert_eq!(c.try_acquire("u1", COOLDOWN, t0), Ok(()));
        assert_eq!(
            c.try_acquire("u1", COOLDOWN, t0 + Duration::from_secs(20)),
            Err(Duration::from_secs(40))
        );
    }

    #[test]
    fn the_blocked_attempt_does_not_extend_the_cooldown() {
        let mut c = Cooldowns::new();
        let t0 = Instant::now();
        c.try_acquire("u1", COOLDOWN, t0).unwrap();
        let _ = c.try_acquire("u1", COOLDOWN, t0 + Duration::from_secs(59));
        assert_eq!(
            c.try_acquire("u1", COOLDOWN, t0 + Duration::from_secs(60)),
            Ok(())
        );
    }

    #[test]
    fn passes_again_after_the_cooldown() {
        let mut c = Cooldowns::new();
        let t0 = Instant::now();
        c.try_acquire("u1", COOLDOWN, t0).unwrap();
        assert!(
            c.try_acquire("u1", COOLDOWN, t0 + Duration::from_secs(59))
                .is_err()
        );
        assert_eq!(
            c.try_acquire("u1", COOLDOWN, t0 + Duration::from_secs(61)),
            Ok(())
        );
    }

    #[test]
    fn users_are_independent() {
        let mut c = Cooldowns::new();
        let t0 = Instant::now();
        c.try_acquire("u1", COOLDOWN, t0).unwrap();
        assert_eq!(c.try_acquire("u2", COOLDOWN, t0), Ok(()));
        assert!(c.try_acquire("u1", COOLDOWN, t0).is_err());
    }

    #[test]
    fn zero_cooldown_disables_the_feature() {
        let mut c = Cooldowns::new();
        let t0 = Instant::now();
        for _ in 0..5 {
            assert_eq!(c.try_acquire("u1", Duration::ZERO, t0), Ok(()));
        }
        assert_eq!(c.tracked(), 0);
    }

    #[test]
    fn refund_lets_the_user_try_again_immediately() {
        let mut c = Cooldowns::new();
        let t0 = Instant::now();
        c.try_acquire("u1", COOLDOWN, t0).unwrap();
        c.refund("u1");
        assert_eq!(c.try_acquire("u1", COOLDOWN, t0), Ok(()));
        c.refund("never-seen");
    }

    #[test]
    fn memory_stays_bounded_and_prefers_dropping_expired_entries() {
        let mut c = Cooldowns::new();
        let t0 = Instant::now();
        for i in 0..MAX_TRACKED_USERS {
            c.try_acquire(&format!("old-{i}"), COOLDOWN, t0).unwrap();
        }
        assert_eq!(c.tracked(), MAX_TRACKED_USERS);

        // Much later, all old entries have expired and are swept.
        let later = t0 + Duration::from_secs(3_600);
        c.try_acquire("fresh", COOLDOWN, later).unwrap();
        assert_eq!(c.tracked(), 1);
    }

    #[test]
    fn when_nothing_has_expired_the_oldest_entry_is_evicted() {
        let mut c = Cooldowns::new();
        let t0 = Instant::now();
        for i in 0..MAX_TRACKED_USERS {
            let at = t0 + Duration::from_millis(i as u64);
            c.try_acquire(&format!("u-{i}"), COOLDOWN, at).unwrap();
        }
        let now = t0 + Duration::from_secs(1);
        c.try_acquire("newcomer", COOLDOWN, now).unwrap();
        assert_eq!(c.tracked(), MAX_TRACKED_USERS);
        // u-0 was the oldest and lost its cooldown; u-1 still has one.
        assert_eq!(c.try_acquire("u-0", COOLDOWN, now), Ok(()));
        assert!(c.try_acquire("u-5000", COOLDOWN, now).is_err());
    }
}
