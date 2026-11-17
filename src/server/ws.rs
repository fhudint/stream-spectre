//! WebSocket endpoints for the overlay and the dashboard.
//!
//! Both receive the event stream described in `docs/websocket-events.md`.
//! The dashboard acts through the REST API; the overlay reports back over its
//! socket (`ended`, `player_error`).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tokio::sync::broadcast::error::RecvError;

use super::ServerState;
use crate::app::{Envelope, Event, OverlayMessage, PROTOCOL_VERSION, unix_ms};
use crate::queue::ItemId;
use crate::youtube::VideoSource;

/// Most clients connected at once. A dashboard and an overlay or two are the
/// normal case; this only stops a runaway client from exhausting the process.
pub const MAX_CONNECTIONS: usize = 32;

/// Largest message accepted from a client. Their messages are tiny.
const MAX_CLIENT_MESSAGE_BYTES: usize = 4 * 1024;

/// How often an idle connection is pinged, to notice vanished peers.
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// A client that cannot take a message within this time is dropped.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Overlay,
    Dashboard,
}

impl Kind {
    /// The overlay needs only what changes what is on screen.
    fn wants(self, event: &Event) -> bool {
        match self {
            Self::Dashboard => true,
            Self::Overlay => matches!(
                event,
                Event::Play { .. }
                    | Event::Skip { .. }
                    | Event::Stop {}
                    | Event::Volume { .. }
                    | Event::PlaybackPaused { .. }
            ),
        }
    }
}

/// Counts open connections and gives the slot back when dropped.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(counter: &Arc<AtomicUsize>) -> Option<Self> {
        let previous = counter.fetch_add(1, Ordering::SeqCst);
        if previous >= MAX_CONNECTIONS {
            counter.fetch_sub(1, Ordering::SeqCst);
            None
        } else {
            Some(Self(Arc::clone(counter)))
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub async fn overlay<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    accept(state, upgrade, Kind::Overlay)
}

pub async fn dashboard<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    accept(state, upgrade, Kind::Dashboard)
}

fn accept<S: VideoSource + 'static>(
    state: ServerState<S>,
    upgrade: WebSocketUpgrade,
    kind: Kind,
) -> Response {
    let Some(slot) = Slot::take(&state.connections) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "too many connections").into_response();
    };
    upgrade
        .max_message_size(MAX_CLIENT_MESSAGE_BYTES)
        .max_frame_size(MAX_CLIENT_MESSAGE_BYTES)
        .on_upgrade(move |socket| run(socket, state, kind, slot))
}

async fn send(socket: &mut WebSocket, message: Message) -> Result<(), ()> {
    match tokio::time::timeout(SEND_TIMEOUT, socket.send(message)).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}

/// Sends the full current state and returns its sequence number.
async fn send_hello<S: VideoSource + 'static>(
    socket: &mut WebSocket,
    state: &ServerState<S>,
    kind: Kind,
) -> Option<u64> {
    let hello = state.pipeline.hello();
    let seq = hello.seq;
    let hello = match kind {
        Kind::Overlay => hello.for_overlay(),
        Kind::Dashboard => hello,
    };
    let envelope = Envelope {
        v: PROTOCOL_VERSION,
        seq,
        ts: unix_ms(SystemTime::now()),
        event: Event::Hello(Box::new(hello)),
    };
    let json = serde_json::to_string(&envelope).ok()?;
    send(socket, Message::text(json)).await.ok()?;
    Some(seq)
}

/// Events at or before the snapshot are already included in it.
fn newer_than_snapshot(seq: u64, snapshot_seq: u64) -> bool {
    seq > snapshot_seq
}

async fn run<S: VideoSource + 'static>(
    mut socket: WebSocket,
    state: ServerState<S>,
    kind: Kind,
    _slot: Slot,
) {
    // Subscribe before taking the snapshot so nothing falls in between.
    let mut events = state.pipeline.subscribe();
    let mut shutdown = state.shutdown.clone();
    let Some(mut snapshot_seq) = send_hello(&mut socket, &state, kind).await else {
        return;
    };
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await;

    loop {
        tokio::select! {
            // The guard returned by `wait_for` is not `Send`, so it must not
            // outlive this block: drop it right away.
            () = async { let _ = shutdown.wait_for(|stop| *stop).await; } => {
                let _ = send(&mut socket, Message::Close(None)).await;
                break;
            }
            _ = ping.tick() => {
                if send(&mut socket, Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => match incoming {
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                Some(Ok(Message::Text(text))) => {
                    if kind == Kind::Overlay {
                        handle_overlay_message(&state, text.as_str());
                    }
                }
                Some(Ok(_)) => {}
            },
            event = events.recv() => match event {
                Ok(envelope) => {
                    if newer_than_snapshot(envelope.seq, snapshot_seq) && kind.wants(&envelope.event) {
                        let Ok(json) = serde_json::to_string(&*envelope) else { continue };
                        if send(&mut socket, Message::text(json)).await.is_err() {
                            break;
                        }
                    }
                }
                // Too slow: some events were lost. Start over from a fresh
                // snapshot instead of showing a state with holes.
                Err(RecvError::Lagged(_)) => {
                    match send_hello(&mut socket, &state, kind).await {
                        Some(seq) => snapshot_seq = seq,
                        None => break,
                    }
                }
                Err(RecvError::Closed) => break,
            },
        }
    }
}

fn handle_overlay_message<S: VideoSource + 'static>(state: &ServerState<S>, text: &str) {
    let message = match serde_json::from_str::<OverlayMessage>(text) {
        Ok(message) => message,
        Err(_) => {
            tracing::debug!("ignored a malformed overlay message");
            return;
        }
    };
    let result = match message {
        OverlayMessage::PlayerReady => Ok(()),
        OverlayMessage::Ended { item_id } => state.pipeline.player_ended(ItemId(item_id)),
        OverlayMessage::PlayerError { item_id, code } => {
            state.pipeline.player_error(ItemId(item_id), &code)
        }
        OverlayMessage::Progress {
            item_id,
            position_secs,
            duration_secs,
        } => state
            .pipeline
            .player_progress(ItemId(item_id), position_secs, duration_secs),
    };
    if let Err(error) = result {
        tracing::debug!(%error, "ignored a stale overlay report");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ItemView, SkipReason};
    use crate::chat::{Role, Sender};
    use crate::queue::{ItemId, Media, QueueItem};
    use crate::youtube::{LiveStatus, VideoDetails, VideoId};

    fn play() -> Event {
        let item = QueueItem {
            id: ItemId(1),
            media: Media::Youtube(VideoDetails {
                id: VideoId::parse("dQw4w9WgXcQ").unwrap(),
                title: "t".to_owned(),
                channel: "c".to_owned(),
                duration_secs: 1,
                embeddable: true,
                age_restricted: false,
                live: LiveStatus::None,
                private: false,
            }),
            sender: Sender::new("u", "n", Role::Follower),
            requested_at: SystemTime::UNIX_EPOCH,
        };
        Event::Play {
            item: ItemView::from(&item),
            volume: 10,
        }
    }

    #[test]
    fn the_overlay_gets_only_events_that_change_the_screen() {
        let screen = [
            play(),
            Event::Skip {
                item_id: 1,
                reason: SkipReason::Moderator,
            },
            Event::Stop {},
            Event::Volume { volume: 5 },
        ];
        for event in &screen {
            assert!(Kind::Overlay.wants(event), "{event:?}");
            assert!(Kind::Dashboard.wants(event));
        }
        let moderation = [
            Event::Notice {
                level: crate::app::NoticeLevel::Info,
                code: "x".to_owned(),
                text: "y".to_owned(),
                sender: None,
            },
            Event::QueueUpdated(crate::app::QueueView {
                pending: vec![],
                approved: vec![],
                now_playing: None,
            }),
        ];
        for event in &moderation {
            assert!(!Kind::Overlay.wants(event), "{event:?}");
            assert!(Kind::Dashboard.wants(event));
        }
    }

    #[test]
    fn events_included_in_the_snapshot_are_skipped() {
        assert!(!newer_than_snapshot(5, 5));
        assert!(!newer_than_snapshot(4, 5));
        assert!(newer_than_snapshot(6, 5));
        assert!(newer_than_snapshot(1, 0));
    }

    #[test]
    fn connection_slots_are_limited_and_returned() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut slots = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            slots.push(Slot::take(&counter).expect("slot"));
        }
        assert!(Slot::take(&counter).is_none());
        assert_eq!(counter.load(Ordering::SeqCst), MAX_CONNECTIONS);

        slots.pop();
        assert!(Slot::take(&counter).is_some());
        drop(slots);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
}
