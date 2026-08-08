use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::Serialize;

pub const MAX_FRAME_WIDTH: usize = 2_048;
pub const MAX_FRAME_HEIGHT: usize = 2_048;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const BYTES_PER_PIXEL: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameStatus {
    Complete,
    Incomplete,
    Invalid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameInput<'a> {
    pub status: FrameStatus,
    pub width: usize,
    pub height: usize,
    pub bytes_per_row: usize,
    pub data_size: usize,
    pub timestamp_millis: Option<u64>,
    pub data: &'a [u8],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FrameMetadata {
    pub width: usize,
    pub height: usize,
    pub bytes_per_row: usize,
    pub byte_len: usize,
    pub timestamp_millis: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BgraFrame {
    pub(crate) metadata: FrameMetadata,
    pub(crate) bytes: Vec<u8>,
    pub(crate) digest: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct FrameMetrics {
    pub accepted_frames: u64,
    pub invalid_frames: u64,
    pub duplicate_frames: u64,
    pub dropped_frames: u64,
    pub replaced_frames: u64,
    pub stream_errors: u64,
    pub last_frame: Option<FrameMetadata>,
}

#[derive(Clone, Debug, Default)]
pub struct FrameIngest {
    latest: Option<BgraFrame>,
    metrics: FrameMetrics,
}

impl FrameIngest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ingest(&mut self, input: FrameInput<'_>) -> Result<(), crate::CaptureError> {
        let compact_len = match validate_frame_input(&input) {
            Ok(compact_len) => compact_len,
            Err(error) => {
                self.metrics.invalid_frames = self.metrics.invalid_frames.saturating_add(1);
                return Err(error);
            }
        };
        let mut bytes = Vec::with_capacity(compact_len);
        for row in input.data.chunks(input.bytes_per_row).take(input.height) {
            bytes.extend_from_slice(&row[..input.width * BYTES_PER_PIXEL]);
        }

        self.commit_frame(BgraFrame {
            metadata: FrameMetadata {
                width: input.width,
                height: input.height,
                bytes_per_row: input.width * BYTES_PER_PIXEL,
                byte_len: bytes.len(),
                timestamp_millis: input.timestamp_millis,
            },
            bytes,
            digest: 0,
        })
    }

    pub(crate) fn ingest_owned(&mut self, mut frame: BgraFrame) -> Result<(), crate::CaptureError> {
        let input = FrameInput {
            status: FrameStatus::Complete,
            width: frame.metadata.width,
            height: frame.metadata.height,
            bytes_per_row: frame.metadata.bytes_per_row,
            data_size: frame.bytes.len(),
            timestamp_millis: frame.metadata.timestamp_millis,
            data: &frame.bytes,
        };
        if let Err(error) = validate_frame_input(&input) {
            self.metrics.invalid_frames = self.metrics.invalid_frames.saturating_add(1);
            return Err(error);
        }
        frame.metadata.byte_len = frame.bytes.len();
        self.commit_frame(frame)
    }

    fn commit_frame(&mut self, mut frame: BgraFrame) -> Result<(), crate::CaptureError> {
        let mut hasher = DefaultHasher::new();
        frame.bytes.hash(&mut hasher);
        frame.digest = hasher.finish();
        if self
            .latest
            .as_ref()
            .is_some_and(|latest| latest.digest == frame.digest)
        {
            self.metrics.duplicate_frames = self.metrics.duplicate_frames.saturating_add(1);
            if let Some(latest) = self.latest.as_mut() {
                latest.metadata.timestamp_millis = frame.metadata.timestamp_millis;
                self.metrics.last_frame = Some(latest.metadata.clone());
            }
            return Ok(());
        }
        if self.latest.is_some() {
            self.metrics.replaced_frames = self.metrics.replaced_frames.saturating_add(1);
        }
        self.metrics.last_frame = Some(frame.metadata.clone());
        self.latest = Some(frame);
        self.metrics.accepted_frames = self.metrics.accepted_frames.saturating_add(1);
        Ok(())
    }

    pub fn reject(
        &mut self,
        status: FrameStatus,
        reason: impl Into<String>,
    ) -> crate::CaptureError {
        self.metrics.invalid_frames = self.metrics.invalid_frames.saturating_add(1);
        crate::CaptureError::InvalidFrame {
            status,
            reason: reason.into(),
        }
    }

    pub fn record_drop(&mut self) {
        self.metrics.dropped_frames = self.metrics.dropped_frames.saturating_add(1);
    }

    pub fn record_stream_error(&mut self) {
        self.metrics.stream_errors = self.metrics.stream_errors.saturating_add(1);
    }

    pub fn metrics(&self) -> FrameMetrics {
        self.metrics.clone()
    }

    pub fn latest_metadata(&self) -> Option<FrameMetadata> {
        self.latest.as_ref().map(|frame| frame.metadata.clone())
    }

    pub fn has_raw_frame(&self) -> bool {
        self.latest
            .as_ref()
            .is_some_and(|frame| !frame.bytes.is_empty())
    }

    pub(crate) fn clear_latest(&mut self) {
        self.latest = None;
    }

    #[cfg(test)]
    pub(crate) fn latest_bytes(&self) -> Option<&[u8]> {
        self.latest.as_ref().map(|frame| frame.bytes.as_slice())
    }
}

pub fn validate_frame_input(input: &FrameInput<'_>) -> Result<usize, crate::CaptureError> {
    if input.status != FrameStatus::Complete {
        return Err(crate::CaptureError::InvalidFrame {
            status: input.status,
            reason: "frame status is not complete".to_owned(),
        });
    }
    if input.width == 0 || input.height == 0 {
        return Err(crate::CaptureError::InvalidFrame {
            status: input.status,
            reason: "frame dimensions must be non-zero".to_owned(),
        });
    }
    if input.width > MAX_FRAME_WIDTH || input.height > MAX_FRAME_HEIGHT {
        return Err(crate::CaptureError::InvalidFrame {
            status: input.status,
            reason: "frame dimensions exceed the configured bound".to_owned(),
        });
    }
    let row_bytes = input.width.checked_mul(BYTES_PER_PIXEL).ok_or_else(|| {
        crate::CaptureError::InvalidFrame {
            status: input.status,
            reason: "frame row-byte calculation overflowed".to_owned(),
        }
    })?;
    if input.bytes_per_row < row_bytes {
        return Err(crate::CaptureError::InvalidFrame {
            status: input.status,
            reason: "frame stride is smaller than a BGRA row".to_owned(),
        });
    }
    let required_len = input
        .bytes_per_row
        .checked_mul(input.height)
        .ok_or_else(|| crate::CaptureError::InvalidFrame {
            status: input.status,
            reason: "frame buffer length calculation overflowed".to_owned(),
        })?;
    let compact_len =
        row_bytes
            .checked_mul(input.height)
            .ok_or_else(|| crate::CaptureError::InvalidFrame {
                status: input.status,
                reason: "compact frame length calculation overflowed".to_owned(),
            })?;
    if input.data_size < required_len || input.data_size > MAX_FRAME_BYTES {
        return Err(crate::CaptureError::InvalidFrame {
            status: input.status,
            reason: "frame data size is outside the safe bound".to_owned(),
        });
    }
    if required_len > input.data.len() || compact_len > MAX_FRAME_BYTES {
        return Err(crate::CaptureError::InvalidFrame {
            status: input.status,
            reason: "frame data is incomplete or oversized".to_owned(),
        });
    }
    Ok(compact_len)
}

#[cfg(test)]
mod tests {
    use super::{
        BgraFrame, FrameIngest, FrameInput, FrameStatus, MAX_FRAME_BYTES, MAX_FRAME_HEIGHT,
        MAX_FRAME_WIDTH, validate_frame_input,
    };

    fn input(data: &[u8], timestamp_millis: u64) -> FrameInput<'_> {
        FrameInput {
            status: FrameStatus::Complete,
            width: 2,
            height: 2,
            bytes_per_row: 8,
            data_size: data.len(),
            timestamp_millis: Some(timestamp_millis),
            data,
        }
    }

    #[test]
    fn complete_frame_is_accepted_and_compacted() {
        let data = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let mut ingest = FrameIngest::new();
        ingest.ingest(input(&data, 10)).unwrap();

        assert_eq!(ingest.metrics().accepted_frames, 1);
        assert_eq!(ingest.latest_metadata().unwrap().byte_len, 16);
        assert!(ingest.has_raw_frame());
    }

    #[test]
    fn incomplete_frame_is_rejected_without_replacing_latest() {
        let data = [1; 16];
        let mut ingest = FrameIngest::new();
        let mut frame = input(&data, 10);
        ingest.ingest(frame.clone()).unwrap();
        frame.status = FrameStatus::Incomplete;

        assert!(ingest.ingest(frame).is_err());
        assert_eq!(ingest.metrics().invalid_frames, 1);
        assert_eq!(ingest.metrics().accepted_frames, 1);
        assert_eq!(ingest.latest_metadata().unwrap().timestamp_millis, Some(10));
    }

    #[test]
    fn identical_timestamp_and_content_is_counted_as_duplicate() {
        let data = [1; 16];
        let mut ingest = FrameIngest::new();
        ingest.ingest(input(&data, 10)).unwrap();
        ingest.ingest(input(&data, 10)).unwrap();

        assert_eq!(ingest.metrics().accepted_frames, 1);
        assert_eq!(ingest.metrics().duplicate_frames, 1);
    }

    #[test]
    fn newest_frame_replaces_previous_without_counting_a_pending_drop() {
        let first = [1; 16];
        let second = [2; 16];
        let mut ingest = FrameIngest::new();
        ingest.ingest(input(&first, 10)).unwrap();
        ingest.ingest(input(&second, 11)).unwrap();

        assert_eq!(ingest.metrics().accepted_frames, 2);
        assert_eq!(ingest.metrics().dropped_frames, 0);
        assert_eq!(ingest.metrics().replaced_frames, 1);
        assert_eq!(ingest.latest_metadata().unwrap().timestamp_millis, Some(11));
    }

    #[test]
    fn pending_drop_is_counted_separately_from_latest_replacement() {
        let mut ingest = FrameIngest::new();
        ingest.record_drop();

        assert_eq!(ingest.metrics().dropped_frames, 1);
        assert_eq!(ingest.metrics().replaced_frames, 0);
    }

    #[test]
    fn identical_content_with_a_new_timestamp_is_still_a_duplicate() {
        let data = [1; 16];
        let mut ingest = FrameIngest::new();
        ingest.ingest(input(&data, 10)).unwrap();
        ingest.ingest(input(&data, 11)).unwrap();

        assert_eq!(ingest.metrics().accepted_frames, 1);
        assert_eq!(ingest.metrics().duplicate_frames, 1);
        assert_eq!(ingest.latest_metadata().unwrap().timestamp_millis, Some(11));
        assert_eq!(ingest.metrics().replaced_frames, 0);
    }

    #[test]
    fn dimensions_and_sizes_are_bounded_without_overflow() {
        let data = [0; 16];
        let mut frame = input(&data, 1);
        frame.width = MAX_FRAME_WIDTH + 1;
        assert!(validate_frame_input(&frame).is_err());
        frame = input(&data, 1);
        frame.height = MAX_FRAME_HEIGHT + 1;
        assert!(validate_frame_input(&frame).is_err());
        frame = input(&data, 1);
        frame.bytes_per_row = usize::MAX;
        assert!(validate_frame_input(&frame).is_err());
        frame = input(&data, 1);
        frame.data_size = MAX_FRAME_BYTES + 1;
        assert!(validate_frame_input(&frame).is_err());
    }

    #[test]
    fn stopping_clears_raw_bytes_but_preserves_last_metadata() {
        let data = [1; 16];
        let mut ingest = FrameIngest::new();
        ingest.ingest(input(&data, 10)).unwrap();
        ingest.clear_latest();

        assert!(!ingest.has_raw_frame());
        assert_eq!(
            ingest.metrics().last_frame.unwrap().timestamp_millis,
            Some(10)
        );
        assert!(ingest.latest_bytes().is_none());
    }

    #[allow(dead_code)]
    fn _frame_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BgraFrame>();
    }
}
