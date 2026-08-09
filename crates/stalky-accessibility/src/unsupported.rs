use std::sync::Arc;

use crate::service::{
    AccessibilityBackend, AccessibilityError, AccessibilityEventSink, AccessibilitySession,
};
use crate::{AccessibilityActionRequest, AccessibilityActionResult};
use mega_permissions::PermissionState;

pub(crate) struct UnsupportedBackend;

impl AccessibilityBackend for UnsupportedBackend {
    fn start(
        &self,
        _events: Arc<dyn AccessibilityEventSink>,
    ) -> Result<Box<dyn AccessibilitySession>, AccessibilityError> {
        Err(AccessibilityError::UnsupportedTarget)
    }

    fn request_permission(&self) -> Result<PermissionState, AccessibilityError> {
        Err(AccessibilityError::UnsupportedTarget)
    }

    fn permission_status(&self) -> Result<PermissionState, AccessibilityError> {
        Err(AccessibilityError::UnsupportedTarget)
    }
}

#[allow(dead_code)]
struct UnsupportedSession;

impl AccessibilitySession for UnsupportedSession {
    fn stop(&mut self) -> Result<(), AccessibilityError> {
        Err(AccessibilityError::UnsupportedTarget)
    }

    fn execute(
        &mut self,
        _request: AccessibilityActionRequest,
    ) -> Result<AccessibilityActionResult, AccessibilityError> {
        Err(AccessibilityError::UnsupportedTarget)
    }
}
