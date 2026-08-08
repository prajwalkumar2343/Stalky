use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use thiserror::Error;
use tokio::sync::{Notify, broadcast, watch};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverflowPolicy {
    Reject,
    DropNewest,
    DropOldest,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueueStats {
    pub accepted: u64,
    pub dropped_newest: u64,
    pub dropped_oldest: u64,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum EventBusError {
    #[error("event queue capacity must be greater than zero")]
    InvalidCapacity,
    #[error("event queue is full")]
    Full,
    #[error("event queue is closed")]
    Closed,
    #[error("event queue lock is poisoned")]
    Poisoned,
}

struct QueueState<T> {
    items: VecDeque<T>,
    accepted: AtomicU64,
    dropped_newest: AtomicU64,
    dropped_oldest: AtomicU64,
    closed: AtomicBool,
    notify: Arc<Notify>,
    capacity: usize,
    policy: OverflowPolicy,
}

#[derive(Clone)]
pub struct BoundedQueue<T> {
    state: Arc<Mutex<QueueState<T>>>,
}

pub struct BoundedQueueReceiver<T> {
    state: Arc<Mutex<QueueState<T>>>,
}

pub fn bounded_queue<T>(
    capacity: usize,
    policy: OverflowPolicy,
) -> Result<(BoundedQueue<T>, BoundedQueueReceiver<T>), EventBusError> {
    if capacity == 0 {
        return Err(EventBusError::InvalidCapacity);
    }

    let state = Arc::new(Mutex::new(QueueState {
        items: VecDeque::with_capacity(capacity),
        accepted: AtomicU64::new(0),
        dropped_newest: AtomicU64::new(0),
        dropped_oldest: AtomicU64::new(0),
        closed: AtomicBool::new(false),
        notify: Arc::new(Notify::new()),
        capacity,
        policy,
    }));
    Ok((
        BoundedQueue {
            state: state.clone(),
        },
        BoundedQueueReceiver { state },
    ))
}

impl<T> BoundedQueue<T> {
    pub fn publish(&self, item: T) -> Result<PublishOutcome, EventBusError> {
        let mut state = self.state.lock().map_err(|_| EventBusError::Poisoned)?;
        if state.closed.load(Ordering::Acquire) {
            return Err(EventBusError::Closed);
        }

        if state.items.len() >= state.capacity {
            return match state.policy {
                OverflowPolicy::Reject => Err(EventBusError::Full),
                OverflowPolicy::DropNewest => {
                    state.dropped_newest.fetch_add(1, Ordering::Relaxed);
                    Ok(PublishOutcome::DroppedNewest)
                }
                OverflowPolicy::DropOldest => {
                    state.items.pop_front();
                    state.dropped_oldest.fetch_add(1, Ordering::Relaxed);
                    state.items.push_back(item);
                    state.accepted.fetch_add(1, Ordering::Relaxed);
                    state.notify.notify_one();
                    Ok(PublishOutcome::DroppedOldest)
                }
            };
        }

        state.items.push_back(item);
        state.accepted.fetch_add(1, Ordering::Relaxed);
        state.notify.notify_one();
        Ok(PublishOutcome::Published)
    }

    pub fn close(&self) -> Result<(), EventBusError> {
        let state = self.state.lock().map_err(|_| EventBusError::Poisoned)?;
        state.closed.store(true, Ordering::Release);
        // This queue has one receiver. `notify_one` retains a permit if the
        // receiver is between releasing the mutex and awaiting notification,
        // so close cannot be lost in that race window.
        state.notify.notify_one();
        Ok(())
    }

    pub fn stats(&self) -> Result<QueueStats, EventBusError> {
        let state = self.state.lock().map_err(|_| EventBusError::Poisoned)?;
        Ok(QueueStats {
            accepted: state.accepted.load(Ordering::Relaxed),
            dropped_newest: state.dropped_newest.load(Ordering::Relaxed),
            dropped_oldest: state.dropped_oldest.load(Ordering::Relaxed),
        })
    }

    pub fn len(&self) -> Result<usize, EventBusError> {
        let state = self.state.lock().map_err(|_| EventBusError::Poisoned)?;
        Ok(state.items.len())
    }

    pub fn is_empty(&self) -> Result<bool, EventBusError> {
        Ok(self.len()? == 0)
    }
}

impl<T> BoundedQueueReceiver<T> {
    pub async fn recv(&mut self) -> Result<Option<T>, EventBusError> {
        loop {
            let notify = {
                let mut state = self.state.lock().map_err(|_| EventBusError::Poisoned)?;
                if let Some(item) = state.items.pop_front() {
                    return Ok(Some(item));
                }
                if state.closed.load(Ordering::Acquire) {
                    return Ok(None);
                }
                state.notify.clone()
            };
            notify.notified().await;
        }
    }

    pub fn close(&self) -> Result<(), EventBusError> {
        let state = self.state.lock().map_err(|_| EventBusError::Poisoned)?;
        state.closed.store(true, Ordering::Release);
        state.notify.notify_one();
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishOutcome {
    Published,
    DroppedNewest,
    DroppedOldest,
}

#[derive(Clone)]
pub struct LatestState<T> {
    sender: watch::Sender<T>,
}

impl<T: Clone> LatestState<T> {
    pub fn new(initial: T) -> (Self, watch::Receiver<T>) {
        let (sender, receiver) = watch::channel(initial);
        (Self { sender }, receiver)
    }

    pub fn publish(&self, value: T) -> Result<(), watch::error::SendError<T>> {
        self.sender.send(value)
    }

    pub fn current(&self) -> T {
        self.sender.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<T> {
        self.sender.subscribe()
    }
}

#[derive(Clone)]
pub struct NotificationBus<T> {
    sender: broadcast::Sender<T>,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum NotificationError {
    #[error("notification subscriber lagged by {0} events")]
    Lagged(u64),
    #[error("notification bus is closed")]
    Closed,
}

impl<T: Clone> NotificationBus<T> {
    pub fn new(capacity: usize) -> Result<Self, EventBusError> {
        if capacity == 0 {
            return Err(EventBusError::InvalidCapacity);
        }
        let (sender, _) = broadcast::channel(capacity);
        Ok(Self { sender })
    }

    pub fn publish(&self, event: T) -> Result<usize, NotificationError> {
        self.sender
            .send(event)
            .map_err(|_| NotificationError::Closed)
    }

    pub fn subscribe(&self) -> NotificationReceiver<T> {
        NotificationReceiver {
            receiver: self.sender.subscribe(),
        }
    }
}

pub struct NotificationReceiver<T> {
    receiver: broadcast::Receiver<T>,
}

impl<T: Clone> NotificationReceiver<T> {
    pub async fn recv(&mut self) -> Result<T, NotificationError> {
        self.receiver.recv().await.map_err(|error| match error {
            broadcast::error::RecvError::Lagged(count) => NotificationError::Lagged(count),
            broadcast::error::RecvError::Closed => NotificationError::Closed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EventBusError, LatestState, NotificationBus, NotificationError, OverflowPolicy,
        PublishOutcome, bounded_queue,
    };

    #[tokio::test]
    async fn drop_oldest_keeps_queue_bounded_and_reports_overflow() {
        let (publisher, mut receiver) = bounded_queue(2, OverflowPolicy::DropOldest).unwrap();
        assert_eq!(publisher.publish(1).unwrap(), PublishOutcome::Published);
        assert_eq!(publisher.publish(2).unwrap(), PublishOutcome::Published);
        assert_eq!(publisher.publish(3).unwrap(), PublishOutcome::DroppedOldest);

        assert_eq!(receiver.recv().await.unwrap(), Some(2));
        assert_eq!(receiver.recv().await.unwrap(), Some(3));
        assert_eq!(publisher.stats().unwrap().dropped_oldest, 1);
    }

    #[tokio::test]
    async fn reject_policy_preserves_existing_items() {
        let (publisher, mut receiver) = bounded_queue(1, OverflowPolicy::Reject).unwrap();
        publisher.publish("first").unwrap();

        assert_eq!(publisher.publish("second"), Err(EventBusError::Full));
        assert_eq!(receiver.recv().await.unwrap(), Some("first"));
    }

    #[tokio::test]
    async fn drop_newest_preserves_existing_items_and_counts_drop() {
        let (publisher, mut receiver) = bounded_queue(1, OverflowPolicy::DropNewest).unwrap();
        publisher.publish("first").unwrap();

        assert_eq!(
            publisher.publish("second").unwrap(),
            PublishOutcome::DroppedNewest
        );
        assert_eq!(receiver.recv().await.unwrap(), Some("first"));
        assert_eq!(publisher.stats().unwrap().dropped_newest, 1);
    }

    #[tokio::test]
    async fn latest_state_exposes_only_the_newest_value() {
        let (state, mut receiver) = LatestState::new(1_u8);
        state.publish(2).unwrap();
        assert_eq!(state.current(), 2);
        receiver.changed().await.unwrap();
        assert_eq!(*receiver.borrow_and_update(), 2);
    }

    #[tokio::test]
    async fn broadcast_lag_is_an_explicit_error() {
        let bus = NotificationBus::new(1).unwrap();
        let mut receiver = bus.subscribe();
        bus.publish(1_u8).unwrap();
        bus.publish(2_u8).unwrap();

        assert_eq!(receiver.recv().await, Err(NotificationError::Lagged(1)));
    }

    #[tokio::test]
    async fn closed_queue_wakes_receiver() {
        let (publisher, mut receiver) = bounded_queue::<u8>(1, OverflowPolicy::Reject).unwrap();
        publisher.close().unwrap();

        assert_eq!(receiver.recv().await.unwrap(), None);
    }

    #[test]
    fn zero_capacity_is_rejected_as_invalid_configuration() {
        assert!(matches!(
            bounded_queue::<u8>(0, OverflowPolicy::Reject),
            Err(EventBusError::InvalidCapacity)
        ));
        assert!(matches!(
            NotificationBus::<u8>::new(0),
            Err(EventBusError::InvalidCapacity)
        ));
    }
}
