use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use tokio::sync::broadcast;

use super::events::{Envelope, Event, PROTOCOL_VERSION, unix_ms};

/// Fan-out of events to every connected client.
///
/// Each event gets the next sequence number and a timestamp when it is
/// published. A client that falls too far behind loses events (it receives
/// `RecvError::Lagged`); it should then ask for a fresh `Hello` instead of
/// trying to catch up.
pub struct EventBus {
    tx: broadcast::Sender<Arc<Envelope>>,
    /// Sequence number of the last published event. Held while sending so
    /// numbers and delivery order always agree.
    last_seq: Mutex<u64>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self {
            tx,
            last_seq: Mutex::new(0),
        }
    }

    /// Stamps and sends an event. Never blocks; with no listeners the event
    /// is simply dropped.
    pub fn publish(&self, event: Event) -> Arc<Envelope> {
        let mut last = self.last_seq.lock().unwrap_or_else(PoisonError::into_inner);
        *last += 1;
        let envelope = Arc::new(Envelope {
            v: PROTOCOL_VERSION,
            seq: *last,
            ts: unix_ms(SystemTime::now()),
            event,
        });
        let _ = self.tx.send(Arc::clone(&envelope));
        envelope
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Envelope>> {
        self.tx.subscribe()
    }

    /// Sequence number of the most recent event, 0 before the first.
    pub fn last_seq(&self) -> u64 {
        *self.last_seq.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::broadcast::error::{RecvError, TryRecvError};

    use super::*;

    fn volume(n: u8) -> Event {
        Event::Volume { volume: n }
    }

    #[tokio::test]
    async fn events_are_numbered_from_one_and_delivered_in_order() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        assert_eq!(bus.last_seq(), 0);

        let first = bus.publish(volume(1));
        let second = bus.publish(volume(2));
        assert_eq!((first.seq, second.seq), (1, 2));
        assert_eq!(bus.last_seq(), 2);

        assert_eq!(rx.recv().await.unwrap().seq, 1);
        let received = rx.recv().await.unwrap();
        assert_eq!(received.seq, 2);
        assert_eq!(received.v, PROTOCOL_VERSION);
        assert_eq!(received.event, volume(2));
    }

    #[tokio::test]
    async fn every_subscriber_gets_every_event() {
        let bus = EventBus::new(16);
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        bus.publish(volume(7));
        assert_eq!(a.recv().await.unwrap().event, volume(7));
        assert_eq!(b.recv().await.unwrap().event, volume(7));
    }

    #[tokio::test]
    async fn late_subscribers_see_only_later_events() {
        let bus = EventBus::new(16);
        bus.publish(volume(1));
        let mut late = bus.subscribe();
        assert_eq!(late.try_recv().unwrap_err(), TryRecvError::Empty);
        bus.publish(volume(2));
        assert_eq!(late.recv().await.unwrap().seq, 2);
    }

    #[test]
    fn publishing_without_listeners_is_fine_and_still_counts() {
        let bus = EventBus::new(4);
        assert_eq!(bus.publish(volume(1)).seq, 1);
        assert_eq!(bus.publish(volume(2)).seq, 2);
    }

    #[tokio::test]
    async fn a_slow_subscriber_is_told_it_lagged() {
        let bus = EventBus::new(2);
        let mut slow = bus.subscribe();
        for n in 0..5 {
            bus.publish(volume(n));
        }
        assert!(matches!(slow.recv().await, Err(RecvError::Lagged(_))));
        // After the notice it continues with the newest events.
        let next = slow.recv().await.unwrap();
        assert!(next.seq >= 4, "{}", next.seq);
    }

    #[tokio::test]
    async fn timestamps_do_not_go_backwards() {
        let bus = EventBus::new(16);
        let a = bus.publish(volume(1));
        let b = bus.publish(volume(2));
        assert!(a.ts > 1_600_000_000_000, "{}", a.ts);
        assert!(b.ts >= a.ts);
    }
}
