use std::sync::Arc;

use crate::service::{CaptureBackend, CaptureEvents, CaptureSession};
use crate::{CaptureError, CaptureSource};

#[derive(Debug, Default)]
pub(crate) struct UnsupportedBackend;

impl CaptureBackend for UnsupportedBackend {
    fn start(
        &self,
        _source: CaptureSource,
        _events: Arc<dyn CaptureEvents>,
    ) -> Result<Box<dyn CaptureSession>, CaptureError> {
        Err(CaptureError::UnsupportedTarget)
    }
}
