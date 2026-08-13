use serde::Serialize;

use crate::BgraFrame;

pub const MAX_OCR_TEXT_CHARS: usize = 12_000;

/// Derived text produced from a transient frame. No image bytes are retained
/// in this value or exposed through the capture status boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OcrObservation {
    pub text: String,
    pub mean_confidence_milli: u16,
    pub observed_at_millis: Option<u64>,
    pub frame_sequence: u64,
}

pub(crate) trait FrameOcr: Send + Sync {
    fn recognize(&self, frame: &BgraFrame) -> Result<Option<OcrObservation>, String>;
}

#[cfg(target_os = "macos")]
pub(crate) fn platform_ocr() -> impl FrameOcr {
    VisionFrameOcr
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn platform_ocr() -> impl FrameOcr {
    UnsupportedFrameOcr
}

#[cfg(not(target_os = "macos"))]
struct UnsupportedFrameOcr;

#[cfg(not(target_os = "macos"))]
impl FrameOcr for UnsupportedFrameOcr {
    fn recognize(&self, _frame: &BgraFrame) -> Result<Option<OcrObservation>, String> {
        Ok(None)
    }
}

#[cfg(target_os = "macos")]
struct VisionFrameOcr;

#[cfg(target_os = "macos")]
impl FrameOcr for VisionFrameOcr {
    fn recognize(&self, frame: &BgraFrame) -> Result<Option<OcrObservation>, String> {
        recognize_with_vision(frame)
    }
}

#[cfg(target_os = "macos")]
fn recognize_with_vision(frame: &BgraFrame) -> Result<Option<OcrObservation>, String> {
    use objc2::AnyThread;
    use objc2_core_foundation::CFData;
    use objc2_core_graphics::{
        CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage,
        CGImageAlphaInfo,
    };
    use objc2_foundation::{NSArray, NSDictionary};
    use objc2_vision::{
        VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest,
        VNRequestTextRecognitionLevel,
    };

    let data_len = isize::try_from(frame.bytes.len())
        .map_err(|_| "transient OCR frame is too large".to_owned())?;
    // SAFETY: `CFData::new` copies the bounded frame bytes before returning.
    let data = unsafe { CFData::new(None, frame.bytes.as_ptr(), data_len) }
        .ok_or_else(|| "Vision OCR could not copy the transient frame".to_owned())?;
    let provider = CGDataProvider::with_cf_data(Some(&data))
        .ok_or_else(|| "Vision OCR could not create an image provider".to_owned())?;
    let color_space = CGColorSpace::new_device_rgb()
        .ok_or_else(|| "Vision OCR could not create an RGB color space".to_owned())?;
    let bitmap_info =
        CGBitmapInfo(CGImageAlphaInfo::PremultipliedFirst.0 | CGBitmapInfo::ByteOrder32Host.0);
    // SAFETY: All dimensions and byte lengths were validated by frame ingest;
    // the provider owns a copy for the complete CGImage lifetime.
    let image = unsafe {
        CGImage::new(
            frame.metadata.width,
            frame.metadata.height,
            8,
            32,
            frame.metadata.bytes_per_row,
            Some(&color_space),
            bitmap_info,
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .ok_or_else(|| "Vision OCR could not create a transient image".to_owned())?;

    let options = NSDictionary::<VNImageOption, objc2::runtime::AnyObject>::new();
    // SAFETY: The CGImage and typed empty options dictionary remain alive for
    // the synchronous request.
    let handler = unsafe {
        VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            &image,
            &options,
        )
    };
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Fast);
    request.setUsesLanguageCorrection(false);
    request.setAutomaticallyDetectsLanguage(true);
    let requests: objc2::rc::Retained<NSArray<VNRequest>> = NSArray::from_slice(&[&request]);
    handler
        .performRequests_error(&requests)
        .map_err(|error| format!("Vision OCR failed: {}", error.localizedDescription()))?;

    let Some(results) = request.results() else {
        return Ok(None);
    };
    let mut text = String::new();
    let mut confidence_sum = 0.0_f32;
    let mut accepted = 0_u32;
    for index in 0..results.count() {
        let observation = results.objectAtIndex(index);
        let candidates = observation.topCandidates(1);
        let Some(candidate) = candidates.firstObject() else {
            continue;
        };
        let candidate_text = candidate.string().to_string();
        if candidate_text.trim().is_empty() {
            continue;
        }
        let separator = usize::from(!text.is_empty());
        if text
            .chars()
            .count()
            .saturating_add(separator)
            .saturating_add(candidate_text.chars().count())
            > MAX_OCR_TEXT_CHARS
        {
            break;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&candidate_text);
        confidence_sum += candidate.confidence();
        accepted = accepted.saturating_add(1);
    }
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some(OcrObservation {
        text,
        mean_confidence_milli: ((confidence_sum / accepted.max(1) as f32).clamp(0.0, 1.0) * 1_000.0)
            .round() as u16,
        observed_at_millis: frame.metadata.timestamp_millis,
        frame_sequence: frame.provenance.sequence,
    }))
}
