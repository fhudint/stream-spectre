//! Keeps a chat connection alive: connects, reports status, and reconnects
//! with a growing delay. Independent of any particular platform.

use std::future::Future;
use std::mem;
use std::time::Duration;

use tokio::time::Instant;

use super::source::{ChatEvent, ChatMessage, ChatSource, ChatStatus, GiftEvent};

/// Delay before the first retry; doubles on each consecutive failure.
pub const BASE_DELAY: Duration = Duration::from_secs(2);
/// Longest delay between retries.
pub const MAX_DELAY: Duration = Duration::from_secs(60);
/// How often to check whether a streamer has gone live.
pub const NOT_LIVE_DELAY: Duration = Duration::from_secs(30);
/// A connection that lasted this long counts as working and resets the delay.
pub const HEALTHY_SESSION: Duration = Duration::from_secs(30);

/// Why connecting failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectFailure {
    /// The streamer is not live.
    NotLive,
    /// The platform refused us (rate limit, IP or device block, login needed).
    Blocked,
    /// Network or protocol trouble.
    Network,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectError {
    pub kind: ConnectFailure,
    /// Short description for the logs.
    pub detail: String,
}

/// One thing that happened on an established connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedItem {
    Message(ChatMessage),
    Gift(GiftEvent),
    Status(ChatStatus),
    /// The connection is over (dropped, or the stream ended).
    Ended,
}

/// An established connection.
pub trait ChatFeed: Send {
    fn next(&mut self) -> impl Future<Output = FeedItem> + Send;

    /// A name for the channel that is nicer to read than its ID, when the
    /// connection learned one (for YouTube, the channel and the live's title).
    fn label(&self) -> Option<String> {
        None
    }
}

/// Knows how to open connections to one channel.
pub trait ChatConnector: Send {
    type Feed: ChatFeed;

    fn channel(&self) -> &str;

    fn connect(&mut self) -> impl Future<Output = Result<Self::Feed, ConnectError>> + Send;
}

/// Delays between attempts: exponential, with a fixed slow poll while the
/// streamer is offline.
#[derive(Debug, Default)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    /// Records a failed attempt and returns how long to wait.
    pub fn after_failure(&mut self, kind: ConnectFailure) -> Duration {
        if kind == ConnectFailure::NotLive {
            // Waiting for a stream to start is not an error.
            self.failures = 0;
            return NOT_LIVE_DELAY;
        }
        self.failures = self.failures.saturating_add(1);
        self.delay()
    }

    /// Records the end of a connection that lasted `lived` and returns how
    /// long to wait before reconnecting.
    pub fn after_session(&mut self, lived: Duration) -> Duration {
        if lived >= HEALTHY_SESSION {
            self.failures = 0;
            return BASE_DELAY;
        }
        self.failures = self.failures.saturating_add(1);
        self.delay()
    }

    fn delay(&self) -> Duration {
        let doublings = self.failures.saturating_sub(1).min(16);
        BASE_DELAY
            .checked_mul(1 << doublings)
            .map_or(MAX_DELAY, |delay| delay.min(MAX_DELAY))
    }
}

enum State<F> {
    Idle,
    Sleeping(Duration),
    Connected { feed: F, since: Instant },
}

/// A `ChatSource` that connects through a `ChatConnector` and reconnects
/// forever. Repeated identical status reports are suppressed, so the
/// dashboard hears about changes only.
pub struct ReconnectingSource<C: ChatConnector> {
    connector: C,
    state: State<C::Feed>,
    backoff: Backoff,
    last_status: Option<mem::Discriminant<ChatStatus>>,
}

impl<C: ChatConnector> ReconnectingSource<C> {
    pub fn new(connector: C) -> Self {
        Self {
            connector,
            state: State::Idle,
            backoff: Backoff::default(),
            last_status: None,
        }
    }

    /// Wraps a status in an event unless it repeats the previous one.
    fn announce(&mut self, status: ChatStatus) -> Option<ChatEvent> {
        let kind = mem::discriminant(&status);
        if self.last_status == Some(kind) {
            return None;
        }
        self.last_status = Some(kind);
        Some(ChatEvent::Status(status))
    }
}

impl<C: ChatConnector> ChatSource for ReconnectingSource<C> {
    async fn next_event(&mut self) -> Option<ChatEvent> {
        loop {
            match &mut self.state {
                State::Sleeping(delay) => {
                    let delay = *delay;
                    tokio::time::sleep(delay).await;
                    self.state = State::Idle;
                }
                State::Idle => match self.connector.connect().await {
                    Ok(feed) => {
                        let channel = feed
                            .label()
                            .unwrap_or_else(|| self.connector.channel().to_owned());
                        self.state = State::Connected {
                            feed,
                            since: Instant::now(),
                        };
                        tracing::info!(channel = self.connector.channel(), "chat connected");
                        if let Some(event) = self.announce(ChatStatus::Connected { channel }) {
                            return Some(event);
                        }
                    }
                    Err(error) => {
                        let delay = self.backoff.after_failure(error.kind);
                        tracing::warn!(
                            channel = self.connector.channel(),
                            kind = ?error.kind,
                            detail = %error.detail,
                            retry_in_secs = delay.as_secs(),
                            "chat connection failed"
                        );
                        self.state = State::Sleeping(delay);
                        let status = if error.kind == ConnectFailure::NotLive {
                            ChatStatus::WaitingForLive {
                                channel: self.connector.channel().to_owned(),
                            }
                        } else {
                            ChatStatus::Reconnecting { retry_in: delay }
                        };
                        if let Some(event) = self.announce(status) {
                            return Some(event);
                        }
                    }
                },
                State::Connected { feed, since } => match feed.next().await {
                    FeedItem::Message(message) => return Some(ChatEvent::Message(message)),
                    FeedItem::Gift(gift) => return Some(ChatEvent::Gift(gift)),
                    FeedItem::Status(status) => {
                        if let Some(event) = self.announce(status) {
                            return Some(event);
                        }
                    }
                    FeedItem::Ended => {
                        let lived = since.elapsed();
                        let delay = self.backoff.after_session(lived);
                        tracing::info!(
                            channel = self.connector.channel(),
                            lived_secs = lived.as_secs(),
                            retry_in_secs = delay.as_secs(),
                            "chat connection ended"
                        );
                        self.state = State::Sleeping(delay);
                        if let Some(event) = self.announce(ChatStatus::Disconnected) {
                            return Some(event);
                        }
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::chat::{Role, Sender};

    fn message(text: &str) -> ChatMessage {
        ChatMessage {
            sender: Sender::new("u1", "User", Role::Follower),
            text: text.to_owned(),
        }
    }

    fn not_live() -> ConnectError {
        ConnectError {
            kind: ConnectFailure::NotLive,
            detail: "offline".to_owned(),
        }
    }

    fn network() -> ConnectError {
        ConnectError {
            kind: ConnectFailure::Network,
            detail: "boom".to_owned(),
        }
    }

    // --- Backoff ---

    #[test]
    fn failures_back_off_exponentially_up_to_the_cap() {
        let mut b = Backoff::default();
        let delays: Vec<u64> = (0..8)
            .map(|_| b.after_failure(ConnectFailure::Network).as_secs())
            .collect();
        assert_eq!(delays, vec![2, 4, 8, 16, 32, 60, 60, 60]);
    }

    #[test]
    fn a_long_failure_streak_never_overflows() {
        let mut b = Backoff::default();
        for _ in 0..100_000 {
            assert!(b.after_failure(ConnectFailure::Other) <= MAX_DELAY);
        }
    }

    #[test]
    fn waiting_for_a_stream_polls_slowly_and_resets_the_backoff() {
        let mut b = Backoff::default();
        b.after_failure(ConnectFailure::Network);
        b.after_failure(ConnectFailure::Network);
        assert_eq!(b.after_failure(ConnectFailure::NotLive), NOT_LIVE_DELAY);
        assert_eq!(b.after_failure(ConnectFailure::Network), BASE_DELAY);
    }

    #[test]
    fn healthy_sessions_reset_and_short_ones_escalate() {
        let mut b = Backoff::default();
        assert_eq!(b.after_session(Duration::from_secs(1)), BASE_DELAY);
        assert_eq!(
            b.after_session(Duration::from_secs(1)),
            Duration::from_secs(4)
        );
        assert_eq!(
            b.after_session(Duration::from_secs(1)),
            Duration::from_secs(8)
        );
        assert_eq!(b.after_session(HEALTHY_SESSION), BASE_DELAY);
        assert_eq!(b.after_session(Duration::from_secs(1)), BASE_DELAY);
    }

    // --- ReconnectingSource with a scripted connector ---

    enum Step {
        Item(FeedItem),
        Wait(Duration),
    }

    struct FakeFeed {
        steps: VecDeque<Step>,
        label: Option<String>,
    }

    impl ChatFeed for FakeFeed {
        fn label(&self) -> Option<String> {
            self.label.clone()
        }

        async fn next(&mut self) -> FeedItem {
            loop {
                match self.steps.pop_front() {
                    Some(Step::Item(item)) => return item,
                    Some(Step::Wait(duration)) => tokio::time::sleep(duration).await,
                    None => return FeedItem::Ended,
                }
            }
        }
    }

    struct FakeConnector {
        script: VecDeque<Result<Vec<Step>, ConnectError>>,
        attempts: usize,
        label: Option<String>,
    }

    impl FakeConnector {
        fn new(script: Vec<Result<Vec<Step>, ConnectError>>) -> Self {
            Self {
                script: script.into(),
                attempts: 0,
                label: None,
            }
        }
    }

    impl ChatConnector for FakeConnector {
        type Feed = FakeFeed;

        fn channel(&self) -> &str {
            "streamer"
        }

        async fn connect(&mut self) -> Result<FakeFeed, ConnectError> {
            self.attempts += 1;
            match self.script.pop_front() {
                Some(Ok(steps)) => Ok(FakeFeed {
                    steps: steps.into(),
                    label: self.label.clone(),
                }),
                Some(Err(error)) => Err(error),
                None => Err(network()),
            }
        }
    }

    fn connected() -> ChatEvent {
        ChatEvent::Status(ChatStatus::Connected {
            channel: "streamer".to_owned(),
        })
    }

    /// Takes `count` events and the virtual time at which each arrived.
    async fn take(
        source: &mut ReconnectingSource<FakeConnector>,
        count: usize,
    ) -> Vec<(ChatEvent, Duration)> {
        let start = Instant::now();
        let mut out = Vec::new();
        for _ in 0..count {
            let event = source.next_event().await.unwrap();
            out.push((event, start.elapsed()));
        }
        out
    }

    #[tokio::test(start_paused = true)]
    async fn connects_forwards_messages_and_reports_the_end() {
        let mut source = ReconnectingSource::new(FakeConnector::new(vec![Ok(vec![
            Step::Item(FeedItem::Message(message("a"))),
            Step::Item(FeedItem::Message(message("b"))),
        ])]));
        let events: Vec<ChatEvent> = take(&mut source, 4)
            .await
            .into_iter()
            .map(|(event, _)| event)
            .collect();
        assert_eq!(
            events,
            vec![
                connected(),
                ChatEvent::Message(message("a")),
                ChatEvent::Message(message("b")),
                ChatEvent::Status(ChatStatus::Disconnected),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reconnects_after_a_short_session_with_the_base_delay() {
        let mut source = ReconnectingSource::new(FakeConnector::new(vec![
            Ok(vec![]),
            Ok(vec![Step::Item(FeedItem::Message(message("again")))]),
        ]));
        let events = take(&mut source, 4).await;
        assert_eq!(events[0].0, connected());
        assert_eq!(events[1].0, ChatEvent::Status(ChatStatus::Disconnected));
        assert_eq!(events[2].0, connected());
        assert_eq!(events[2].1, BASE_DELAY);
        assert_eq!(events[3].0, ChatEvent::Message(message("again")));
        assert_eq!(source.connector.attempts, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_name_the_connection_learned_is_shown_in_place_of_the_channel() {
        let mut connector = FakeConnector::new(vec![Ok(vec![])]);
        connector.label = Some("Amanda \u{b7} Live".to_owned());
        let mut source = ReconnectingSource::new(connector);
        let events = take(&mut source, 1).await;
        assert_eq!(
            events[0].0,
            ChatEvent::Status(ChatStatus::Connected {
                channel: "Amanda \u{b7} Live".to_owned()
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn waits_for_the_streamer_and_reports_it_only_once() {
        let mut source = ReconnectingSource::new(FakeConnector::new(vec![
            Err(not_live()),
            Err(not_live()),
            Err(not_live()),
            Ok(vec![]),
        ]));
        let events = take(&mut source, 2).await;
        assert_eq!(
            events[0].0,
            ChatEvent::Status(ChatStatus::WaitingForLive {
                channel: "streamer".to_owned()
            })
        );
        assert_eq!(events[0].1, Duration::ZERO);
        // Two more polls happened silently before the third attempt succeeded.
        assert_eq!(events[1].0, connected());
        assert_eq!(events[1].1, NOT_LIVE_DELAY * 3);
        assert_eq!(source.connector.attempts, 4);
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_failures_back_off_and_are_reported_once() {
        let mut source = ReconnectingSource::new(FakeConnector::new(vec![
            Err(network()),
            Err(network()),
            Err(network()),
            Ok(vec![]),
        ]));
        let events = take(&mut source, 2).await;
        assert_eq!(
            events[0].0,
            ChatEvent::Status(ChatStatus::Reconnecting {
                retry_in: Duration::from_secs(2)
            })
        );
        assert_eq!(events[1].0, connected());
        assert_eq!(events[1].1, Duration::from_secs(2 + 4 + 8));
    }

    #[tokio::test(start_paused = true)]
    async fn a_healthy_session_resets_the_backoff() {
        let healthy = vec![Step::Wait(HEALTHY_SESSION + Duration::from_secs(1))];
        let mut source = ReconnectingSource::new(FakeConnector::new(vec![
            Err(network()),
            Err(network()),
            Ok(healthy),
            Ok(vec![]),
        ]));
        let events = take(&mut source, 4).await;
        // Reconnecting, Connected, Disconnected, Connected.
        assert_eq!(events[1].0, connected());
        assert_eq!(events[2].0, ChatEvent::Status(ChatStatus::Disconnected));
        assert_eq!(events[3].0, connected());
        // After the healthy session the wait is the base delay, not 8 seconds.
        assert_eq!(events[3].1 - events[2].1, BASE_DELAY);
    }

    #[tokio::test(start_paused = true)]
    async fn sessions_that_keep_dying_quickly_back_off() {
        let mut source = ReconnectingSource::new(FakeConnector::new(vec![
            Ok(vec![]),
            Ok(vec![]),
            Ok(vec![]),
            Ok(vec![]),
        ]));
        // Connected, Disconnected, Connected, Disconnected, ...
        let events = take(&mut source, 8).await;
        let connects: Vec<Duration> = events
            .iter()
            .filter(|(event, _)| *event == connected())
            .map(|(_, at)| *at)
            .collect();
        assert_eq!(connects.len(), 4);
        let gaps: Vec<u64> = connects
            .windows(2)
            .map(|w| (w[1] - w[0]).as_secs())
            .collect();
        assert_eq!(gaps, vec![2, 4, 8]);
    }

    #[tokio::test(start_paused = true)]
    async fn feed_status_items_pass_through_without_repeats() {
        let reconnecting = |secs| {
            Step::Item(FeedItem::Status(ChatStatus::Reconnecting {
                retry_in: Duration::from_secs(secs),
            }))
        };
        let mut source = ReconnectingSource::new(FakeConnector::new(vec![Ok(vec![
            reconnecting(2),
            reconnecting(4),
            Step::Item(FeedItem::Message(message("hi"))),
        ])]));
        let events: Vec<ChatEvent> = take(&mut source, 3)
            .await
            .into_iter()
            .map(|(event, _)| event)
            .collect();
        assert_eq!(
            events,
            vec![
                connected(),
                ChatEvent::Status(ChatStatus::Reconnecting {
                    retry_in: Duration::from_secs(2)
                }),
                ChatEvent::Message(message("hi")),
            ]
        );
    }
}
