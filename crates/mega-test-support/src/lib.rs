//! Deterministic helpers shared by Stalky's infrastructure tests.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mega_core::{CorrelationId, SequenceNumber};
use mega_ipc::{EventEnvelope, InfrastructureEvent};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TestTimestamp {
    pub monotonic_millis: u64,
    pub wall_time_unix_millis: i64,
}

#[derive(Debug)]
pub struct DeterministicClock {
    monotonic_millis: AtomicU64,
    wall_time_unix_millis: AtomicI64,
}

impl DeterministicClock {
    pub fn new(monotonic_millis: u64, wall_time_unix_millis: i64) -> Self {
        Self {
            monotonic_millis: AtomicU64::new(monotonic_millis),
            wall_time_unix_millis: AtomicI64::new(wall_time_unix_millis),
        }
    }

    pub fn now(&self) -> TestTimestamp {
        TestTimestamp {
            monotonic_millis: self.monotonic_millis.load(Ordering::Acquire),
            wall_time_unix_millis: self.wall_time_unix_millis.load(Ordering::Acquire),
        }
    }

    pub fn advance(&self, duration: Duration) -> TestTimestamp {
        let millis = duration.as_millis().min(u128::from(u64::MAX)) as u64;
        self.monotonic_millis.fetch_add(millis, Ordering::AcqRel);
        self.wall_time_unix_millis
            .fetch_add(i64::try_from(millis).unwrap_or(i64::MAX), Ordering::AcqRel);
        self.now()
    }
}

#[derive(Clone, Debug)]
pub struct EnvelopeFactory {
    clock: Arc<DeterministicClock>,
    correlation_id: CorrelationId,
    next_sequence: Arc<AtomicU64>,
}

impl EnvelopeFactory {
    pub fn new(clock: Arc<DeterministicClock>, correlation_id: CorrelationId) -> Self {
        Self {
            clock,
            correlation_id,
            next_sequence: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn next(&self, payload: InfrastructureEvent) -> EventEnvelope<InfrastructureEvent> {
        let timestamp = self.clock.now();
        let sequence = self.next_sequence.fetch_add(1, Ordering::AcqRel);
        EventEnvelope::new(
            timestamp.monotonic_millis,
            Some(timestamp.wall_time_unix_millis),
            self.correlation_id,
            sequence,
            payload,
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct EventRecorder<T> {
    events: Arc<Mutex<Vec<T>>>,
}

impl<T> EventRecorder<T> {
    pub fn record(&self, event: T) {
        self.events
            .lock()
            .expect("event recorder lock poisoned")
            .push(event);
    }

    pub fn snapshot(&self) -> Vec<T>
    where
        T: Clone,
    {
        self.events
            .lock()
            .expect("event recorder lock poisoned")
            .clone()
    }

    pub fn len(&self) -> usize {
        self.events
            .lock()
            .expect("event recorder lock poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub const fn next_sequence_after(sequence: SequenceNumber) -> SequenceNumber {
    sequence.saturating_add(1)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use mega_core::{CorrelationId, LifecycleState};

    use super::{DeterministicClock, EnvelopeFactory, EventRecorder};

    #[test]
    fn clock_advances_both_time_domains_deterministically() {
        let clock = DeterministicClock::new(10, 1_000);

        assert_eq!(
            clock.advance(Duration::from_millis(25)).monotonic_millis,
            35
        );
        assert_eq!(clock.now().wall_time_unix_millis, 1_025);
    }

    #[test]
    fn envelope_factory_assigns_monotonic_sequences() {
        let clock = Arc::new(DeterministicClock::new(0, 0));
        let factory = EnvelopeFactory::new(clock, CorrelationId::new(4));
        let payload = mega_ipc::InfrastructureEvent::LifecycleChanged {
            state: LifecycleState::Running,
        };

        let first = factory.next(payload.clone());
        let second = factory.next(payload);

        assert_eq!(first.sequence, 0);
        assert_eq!(second.sequence, 1);
        assert_eq!(first.correlation_id, second.correlation_id);
    }

    #[test]
    fn recorder_is_cloneable_without_sharing_a_snapshot() {
        let recorder = EventRecorder::default();
        recorder.record(1_u8);
        let copy = recorder.clone();
        copy.record(2_u8);

        assert_eq!(recorder.snapshot(), vec![1, 2]);
        assert!(!recorder.is_empty());
    }
}
