//! Bounded live macOS Accessibility observation for Stalky.
//!
//! The public API contains only owned, normalized data and opaque element
//! tokens. Native AX objects never cross this crate's owner-thread boundary
//! and are never exposed through Tauri IPC.

mod model;
mod normalizer;
mod policy;
mod service;

#[cfg(target_os = "macos")]
mod native;
#[cfg(not(target_os = "macos"))]
mod unsupported;

pub use mega_permissions::PermissionState;
pub use model::{
    AccessibilityAction, AccessibilityActionRequest, AccessibilityActionResult,
    AccessibilityApplication, AccessibilityElementId, AccessibilityEvent, AccessibilityEventKind,
    AccessibilityMetrics, AccessibilityNode, AccessibilityRect, AccessibilitySnapshot,
    AccessibilityState, AccessibilityStatus,
};
pub use normalizer::{
    MAX_DEPTH, MAX_NODES, MAX_STRING_CHARS, MAX_VALUE_CHARS, NormalizedTree, RawNode,
    normalize_tree,
};
pub use policy::{ActionPolicyError, should_rebind_focused_application, validate_action};
pub use service::{AccessibilityError, AccessibilityService};

#[cfg(target_os = "macos")]
use native::NativeBackend;
#[cfg(not(target_os = "macos"))]
use unsupported::UnsupportedBackend;

impl AccessibilityService {
    /// Construct a stopped service. No permission query, observer, or run loop
    /// is created here; all native work begins only after [`Self::start`].
    pub fn new() -> Self {
        #[cfg(target_os = "macos")]
        let backend = NativeBackend::new();
        #[cfg(not(target_os = "macos"))]
        let backend = UnsupportedBackend;
        Self::with_backend(backend)
    }
}

impl Default for AccessibilityService {
    fn default() -> Self {
        Self::new()
    }
}
