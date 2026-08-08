//! Domain types shared by Stalky's runtime and local UI boundary.
//!
//! This crate intentionally contains state and contracts only. Platform APIs,
//! persistence, and application control belong in adapters outside this crate.

mod audio;
mod capture;
mod identifiers;
mod lifecycle;
mod platform;
mod state;
mod subsystem;

pub use audio::{AudioHealth, AudioState, AudioStatus};
pub use capture::{CaptureHealth, CaptureMode, CaptureSource, CaptureState, CaptureStopReason};
pub use identifiers::{CorrelationId, SequenceNumber};
pub use lifecycle::{LifecycleState, LifecycleTransition, LifecycleTransitionError};
pub use mega_permissions::{
    PermissionCapability, PermissionOperation, PermissionRegistry, PermissionSnapshot,
    PermissionState,
};
pub use platform::{DisplayEvent, LifecycleEvent, PlatformEvent};
pub use state::InfrastructureState;
pub use subsystem::{HealthStatus, Subsystem, SubsystemHealth};
