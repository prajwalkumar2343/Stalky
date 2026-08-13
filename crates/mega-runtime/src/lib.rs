//! Async runtime primitives for bounded local infrastructure work.

mod event_bus;
mod extraction_worker;
mod supervisor;

pub use event_bus::{
    BoundedQueue, BoundedQueueReceiver, EventBusError, LatestState, NotificationBus,
    NotificationError, NotificationReceiver, OverflowPolicy, PublishOutcome, QueueStats,
    bounded_queue,
};
pub use extraction_worker::{
    ExtractionJobHandler, ExtractionWorker, ExtractionWorkerConfig, ExtractionWorkerError,
    ExtractionWorkerHealthSnapshot, ExtractionWorkerHealthStatus, ExtractionWorkerPhase,
    ExtractionWorkerReport, WorkerShutdown,
};
pub use supervisor::{SupervisionReport, Supervisor, SupervisorError, SupervisorTransition};
