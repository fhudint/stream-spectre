use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use moka::Expiry;
use moka::future::Cache;

use super::client::VideoSource;
use super::error::YoutubeError;
use super::model::VideoDetails;
use super::video_id::VideoId;

/// How long "no such video" is remembered, so a repeated bad ID does not
/// spend quota every time.
pub const NEGATIVE_TTL: Duration = Duration::from_secs(60);

/// After the quota is reported exhausted, wait this long before trying again.
/// The quota resets at midnight Pacific Time; probing every 15 minutes avoids
/// needing time-zone rules.
pub const QUOTA_BACKOFF: Duration = Duration::from_secs(15 * 60);

/// After a rate-limit response, wait this long before trying again.
pub const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(30);

struct TtlPolicy {
    found: Duration,
    missing: Duration,
}

impl Expiry<VideoId, Option<VideoDetails>> for TtlPolicy {
    fn expire_after_create(
        &self,
        _key: &VideoId,
        value: &Option<VideoDetails>,
        _created_at: Instant,
    ) -> Option<Duration> {
        Some(if value.is_some() {
            self.found
        } else {
            self.missing
        })
    }
}

/// Stops calling the API for a while after it reports quota or rate limits.
#[derive(Default)]
struct Breaker {
    blocked: Option<(Instant, YoutubeError)>,
}

impl Breaker {
    fn check(&mut self, now: Instant) -> Result<(), YoutubeError> {
        match &self.blocked {
            Some((until, error)) if now < *until => Err(error.clone()),
            Some(_) => {
                self.blocked = None;
                Ok(())
            }
            None => Ok(()),
        }
    }

    fn record(&mut self, error: &YoutubeError, now: Instant) {
        let backoff = match error {
            YoutubeError::QuotaExceeded => QUOTA_BACKOFF,
            YoutubeError::RateLimited => RATE_LIMIT_BACKOFF,
            _ => return,
        };
        self.blocked = Some((now + backoff, error.clone()));
    }
}

/// Cached, quota-aware video lookups on top of a `VideoSource`.
///
/// - Found videos are cached for `ttl`, "no such video" for `NEGATIVE_TTL`.
/// - Concurrent lookups of the same ID share one API call.
/// - Failures are never cached, except that a quota or rate-limit failure
///   pauses API calls for a while (cached videos are still served).
pub struct VideoLookup<S> {
    source: S,
    cache: Cache<VideoId, Option<VideoDetails>>,
    breaker: Mutex<Breaker>,
}

impl<S: VideoSource> VideoLookup<S> {
    pub fn new(source: S, ttl: Duration, max_entries: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_entries)
            .expire_after(TtlPolicy {
                found: ttl,
                missing: ttl.min(NEGATIVE_TTL),
            })
            .build();
        Self {
            source,
            cache,
            breaker: Mutex::new(Breaker::default()),
        }
    }

    pub fn source(&self) -> &S {
        &self.source
    }

    /// Forgets every cached answer and lifts any pause after a quota or rate
    /// limit. Used when the API key changes: what was learned with the old
    /// key says nothing about the new one.
    pub fn reset(&self) {
        self.cache.invalidate_all();
        *self.breaker() = Breaker::default();
    }

    /// `Ok(None)` means YouTube has no such (visible) video.
    pub async fn lookup(&self, id: &VideoId) -> Result<Option<VideoDetails>, YoutubeError> {
        self.lookup_at(id, Instant::now()).await
    }

    async fn lookup_at(
        &self,
        id: &VideoId,
        now: Instant,
    ) -> Result<Option<VideoDetails>, YoutubeError> {
        if let Some(hit) = self.cache.get(id).await {
            return Ok(hit);
        }
        self.breaker().check(now)?;

        match self
            .cache
            .try_get_with(id.clone(), self.source.fetch(id))
            .await
        {
            Ok(value) => Ok(value),
            Err(shared) => {
                let error = YoutubeError::clone(&shared);
                self.breaker().record(&error, now);
                Err(error)
            }
        }
    }

    fn breaker(&self) -> MutexGuard<'_, Breaker> {
        self.breaker.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::youtube::model::LiveStatus;

    type Reply = Result<Option<VideoDetails>, YoutubeError>;

    fn id(text: &str) -> VideoId {
        VideoId::parse(text).unwrap()
    }

    fn details(id: &VideoId) -> VideoDetails {
        VideoDetails {
            id: id.clone(),
            title: "t".to_owned(),
            channel: "c".to_owned(),
            duration_secs: 100,
            embeddable: true,
            age_restricted: false,
            live: LiveStatus::None,
            private: false,
        }
    }

    /// Replays scripted replies, then answers "found" for everything.
    #[derive(Default)]
    struct FakeSource {
        calls: AtomicUsize,
        script: Mutex<VecDeque<Reply>>,
        delay: Duration,
    }

    impl FakeSource {
        fn with_script(replies: Vec<Reply>) -> Self {
            Self {
                script: Mutex::new(replies.into()),
                ..Self::default()
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl VideoSource for FakeSource {
        async fn fetch(&self, id: &VideoId) -> Reply {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            let next = self.script.lock().unwrap().pop_front();
            next.unwrap_or_else(|| Ok(Some(details(id))))
        }
    }

    fn lookup(source: FakeSource) -> VideoLookup<FakeSource> {
        VideoLookup::new(source, Duration::from_secs(3_600), 100)
    }

    #[tokio::test]
    async fn reset_forgets_cached_answers_and_lifts_a_quota_pause() {
        let service = lookup(FakeSource::with_script(vec![
            Ok(Some(details(&id("ccccccccccc")))),
            Err(YoutubeError::QuotaExceeded),
        ]));
        let cached = id("ccccccccccc");
        let other = id("ddddddddddd");
        service.lookup(&cached).await.unwrap();
        assert_eq!(
            service.lookup(&other).await,
            Err(YoutubeError::QuotaExceeded)
        );
        // Paused now: this does not even reach the source.
        assert_eq!(service.source().calls(), 2);
        assert_eq!(
            service.lookup(&other).await,
            Err(YoutubeError::QuotaExceeded)
        );
        assert_eq!(service.source().calls(), 2);

        service.reset();
        // The pause is gone, and the cached video is asked for again.
        assert!(service.lookup(&other).await.unwrap().is_some());
        assert!(service.lookup(&cached).await.unwrap().is_some());
        assert_eq!(service.source().calls(), 4);
    }

    #[tokio::test]
    async fn found_videos_are_cached() {
        let service = lookup(FakeSource::default());
        let a = id("aaaaaaaaaaa");
        let first = service.lookup(&a).await.unwrap().unwrap();
        let second = service.lookup(&a).await.unwrap().unwrap();
        assert_eq!(first, second);
        assert_eq!(service.source().calls(), 1);

        service.lookup(&id("bbbbbbbbbbb")).await.unwrap();
        assert_eq!(service.source().calls(), 2);
    }

    #[tokio::test]
    async fn missing_videos_are_cached_too() {
        let service = lookup(FakeSource::with_script(vec![Ok(None)]));
        let a = id("aaaaaaaaaaa");
        assert_eq!(service.lookup(&a).await, Ok(None));
        assert_eq!(service.lookup(&a).await, Ok(None));
        assert_eq!(service.source().calls(), 1);
    }

    #[tokio::test]
    async fn ordinary_failures_are_not_cached_and_do_not_pause_the_api() {
        let service = lookup(FakeSource::with_script(vec![
            Err(YoutubeError::Network("timeout")),
            Err(YoutubeError::Unavailable { status: 503 }),
        ]));
        let a = id("aaaaaaaaaaa");
        assert_eq!(
            service.lookup(&a).await,
            Err(YoutubeError::Network("timeout"))
        );
        assert_eq!(
            service.lookup(&a).await,
            Err(YoutubeError::Unavailable { status: 503 })
        );
        // The third call reaches the source again and succeeds.
        assert!(service.lookup(&a).await.unwrap().is_some());
        assert_eq!(service.source().calls(), 3);
    }

    #[tokio::test]
    async fn quota_errors_pause_the_api_but_cached_videos_still_work() {
        let service = lookup(FakeSource::with_script(vec![
            Ok(Some(details(&id("ccccccccccc")))),
            Err(YoutubeError::QuotaExceeded),
        ]));
        let cached = id("ccccccccccc");
        let other = id("ddddddddddd");
        let start = Instant::now();

        service.lookup_at(&cached, start).await.unwrap();
        assert_eq!(
            service.lookup_at(&other, start).await,
            Err(YoutubeError::QuotaExceeded)
        );
        assert_eq!(service.source().calls(), 2);

        // Paused: no further API calls, same error.
        let during = start + Duration::from_secs(60);
        assert_eq!(
            service.lookup_at(&other, during).await,
            Err(YoutubeError::QuotaExceeded)
        );
        assert_eq!(service.source().calls(), 2);
        // Cached videos are still served.
        assert!(service.lookup_at(&cached, during).await.unwrap().is_some());

        // After the backoff the API is tried again.
        let after = start + QUOTA_BACKOFF + Duration::from_secs(1);
        assert!(service.lookup_at(&other, after).await.unwrap().is_some());
        assert_eq!(service.source().calls(), 3);
    }

    #[tokio::test]
    async fn rate_limits_pause_the_api_briefly() {
        let service = lookup(FakeSource::with_script(vec![Err(
            YoutubeError::RateLimited,
        )]));
        let a = id("aaaaaaaaaaa");
        let start = Instant::now();

        assert_eq!(
            service.lookup_at(&a, start).await,
            Err(YoutubeError::RateLimited)
        );
        assert_eq!(
            service
                .lookup_at(&a, start + RATE_LIMIT_BACKOFF - Duration::from_secs(1))
                .await,
            Err(YoutubeError::RateLimited)
        );
        assert_eq!(service.source().calls(), 1);

        let after = start + RATE_LIMIT_BACKOFF + Duration::from_secs(1);
        assert!(service.lookup_at(&a, after).await.unwrap().is_some());
        assert_eq!(service.source().calls(), 2);
    }

    #[tokio::test]
    async fn concurrent_lookups_of_one_id_share_a_single_api_call() {
        let source = FakeSource {
            delay: Duration::from_millis(100),
            ..FakeSource::default()
        };
        let service = lookup(source);
        let a = id("aaaaaaaaaaa");

        let (r1, r2, r3, r4) = tokio::join!(
            service.lookup(&a),
            service.lookup(&a),
            service.lookup(&a),
            service.lookup(&a)
        );
        for result in [r1, r2, r3, r4] {
            assert!(result.unwrap().is_some());
        }
        assert_eq!(service.source().calls(), 1);
    }

    #[tokio::test]
    async fn concurrent_failures_are_shared_but_not_cached() {
        let source = FakeSource {
            delay: Duration::from_millis(50),
            script: Mutex::new(vec![Err(YoutubeError::Network("timeout"))].into()),
            ..FakeSource::default()
        };
        let service = lookup(source);
        let a = id("aaaaaaaaaaa");

        let (r1, r2) = tokio::join!(service.lookup(&a), service.lookup(&a));
        assert_eq!(r1, Err(YoutubeError::Network("timeout")));
        assert_eq!(r2, Err(YoutubeError::Network("timeout")));
        assert_eq!(service.source().calls(), 1);

        assert!(service.lookup(&a).await.unwrap().is_some());
    }

    #[test]
    fn ttl_policy_uses_a_short_ttl_for_missing_videos() {
        let policy = TtlPolicy {
            found: Duration::from_secs(3_600),
            missing: Duration::from_secs(60),
        };
        let key = id("aaaaaaaaaaa");
        let now = Instant::now();
        assert_eq!(
            policy.expire_after_create(&key, &Some(details(&key)), now),
            Some(Duration::from_secs(3_600))
        );
        assert_eq!(
            policy.expire_after_create(&key, &None, now),
            Some(Duration::from_secs(60))
        );
    }
}
