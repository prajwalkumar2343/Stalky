//! macOS-only ScreenCaptureKit adapter.
//!
//! The adapter uses the maintained `objc2-screen-capture-kit` bindings. The
//! Objective-C boundary is kept here; the service, frame policy, and tests do
//! not depend on Apple framework types.

use std::slice;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQoS, DispatchQueue, DispatchQueueAttr, DispatchRetained};
use mega_permissions::PermissionState;
use mega_platform_macos::MacOsPlatform;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_graphics::CGMainDisplayID;
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVImageBuffer, CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetDataSize, CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType,
    CVPixelBufferGetWidth, CVPixelBufferIsPlanar, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVReturnSuccess,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCFrameStatus, SCRunningApplication, SCShareableContent, SCStream,
    SCStreamConfiguration, SCStreamDelegate, SCStreamFrameInfoStatus, SCStreamOutput,
    SCStreamOutputType,
};

use crate::service::{CaptureBackend, CaptureEvents, CaptureSession};
use crate::{
    CaptureError, CaptureSource, FrameInput, FrameStatus, MAX_FRAME_BYTES, MAX_FRAME_HEIGHT,
    MAX_FRAME_WIDTH,
};

const COMPLETION_TIMEOUT: Duration = Duration::from_secs(15);
const BGRA_PIXEL_FORMAT: u32 = 0x4247_5241;

#[derive(Debug)]
pub(crate) struct NativeBackend;

impl CaptureBackend for NativeBackend {
    fn start(
        &self,
        source: CaptureSource,
        events: Arc<dyn CaptureEvents>,
    ) -> Result<Box<dyn CaptureSession>, CaptureError> {
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let (commands, command_receiver) = mpsc::sync_channel(1);
        let control_source = source;
        let control = thread::Builder::new()
            .name("stalky-capture-control".to_owned())
            .spawn(move || {
                native_control_thread(control_source, events, command_receiver, startup_sender)
            })
            .map_err(|error| CaptureError::StreamStart {
                capture_source: source.to_string(),
                message: format!("could not create native control thread: {error}"),
            })?;

        match startup_receiver.recv_timeout(COMPLETION_TIMEOUT) {
            Ok(Ok(())) => Ok(Box::new(NativeSession {
                source: source.to_string(),
                commands,
                control: Some(control),
            })),
            Ok(Err(error)) => {
                let _ = control.join();
                Err(error)
            }
            Err(_) => {
                // Dropping the startup receiver makes a late startup result
                // trigger cleanup in the control thread. Join rather than
                // detaching it so a timeout cannot leak a live SCStream.
                drop(startup_receiver);
                drop(commands);
                let _ = control.join();
                Err(CaptureError::StreamStart {
                    capture_source: source.to_string(),
                    message: "timed out while starting the native capture control thread"
                        .to_owned(),
                })
            }
        }
    }
}

struct NativeSession {
    source: String,
    commands: mpsc::SyncSender<NativeCommand>,
    control: Option<JoinHandle<Result<(), String>>>,
}

impl CaptureSession for NativeSession {
    fn stop(&mut self) -> Result<(), CaptureError> {
        let Some(control) = self.control.take() else {
            return Ok(());
        };

        let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
        let mut errors = Vec::new();
        if let Err(error) = self.commands.send(NativeCommand::Stop(reply_sender)) {
            errors.push(format!("could not send stop command: {error}"));
        } else {
            match reply_receiver.recv_timeout(COMPLETION_TIMEOUT) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => errors.push(error),
                Err(_) => errors.push("timed out waiting for native capture shutdown".to_owned()),
            }
        }

        match control.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => errors.push(error),
            Err(_) => {
                errors.push("native capture control thread terminated unexpectedly".to_owned())
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(CaptureError::StreamStop {
                capture_source: self.source.clone(),
                message: errors.join("; "),
            })
        }
    }
}

impl Drop for NativeSession {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

enum NativeCommand {
    Stop(mpsc::SyncSender<Result<(), String>>),
}

fn native_control_thread(
    source: CaptureSource,
    events: Arc<dyn CaptureEvents>,
    commands: mpsc::Receiver<NativeCommand>,
    startup_sender: mpsc::SyncSender<Result<(), CaptureError>>,
) -> Result<(), String> {
    let mut runtime = match setup_native_runtime(source, events) {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = startup_sender.send(Err(error));
            return Ok(());
        }
    };

    // If the caller timed out, its receiver is gone. Cleanup synchronously on
    // this owner thread before returning instead of entering the command loop.
    if startup_sender.send(Ok(())).is_err() {
        return result_from_messages(runtime.shutdown());
    }

    match commands.recv() {
        Ok(NativeCommand::Stop(reply)) => {
            let result = result_from_messages(runtime.shutdown());
            if reply.send(result.clone()).is_ok() {
                Ok(())
            } else {
                result
            }
        }
        Err(_) => result_from_messages(runtime.shutdown()),
    }
}

struct NativeRuntime {
    stream: Option<Retained<SCStream>>,
    callbacks: Retained<NativeCallbacks>,
    queue: DispatchRetained<DispatchQueue>,
    inbox: Arc<NativeFrameInbox>,
    worker: Option<JoinHandle<()>>,
    output_registered: bool,
}

impl NativeRuntime {
    fn output(&self) -> &ProtocolObject<dyn SCStreamOutput> {
        ProtocolObject::from_ref(&*self.callbacks)
    }

    fn shutdown(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        if let Some(stream) = self.stream.as_ref() {
            // A failed start can still have an in-flight start operation. A
            // bounded stop attempt gives ScreenCaptureKit a chance to settle
            // before the output handler and stream are released.
            if self.output_registered {
                if let Err(message) = stop_stream(stream) {
                    errors.push(message);
                }
                if let Err(error) = unsafe {
                    stream.removeStreamOutput_type_error(self.output(), SCStreamOutputType::Screen)
                } {
                    errors.push(error_message(&error));
                }
            }
        }
        self.output_registered = false;
        self.inbox.shutdown();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            errors.push("frame worker terminated unexpectedly".to_owned());
        }
        self.stream.take();
        errors
    }
}

impl Drop for NativeRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn setup_native_runtime(
    source: CaptureSource,
    events: Arc<dyn CaptureEvents>,
) -> Result<NativeRuntime, CaptureError> {
    let platform = MacOsPlatform::new();
    let permission = platform
        .screen_recording_permission_status()
        .map_err(|error| CaptureError::PermissionPreflight {
            message: error.to_string(),
        })?;
    if permission != PermissionState::Granted {
        return Err(CaptureError::PermissionNotGranted {
            observed: permission,
        });
    }

    // This is intentionally after the permission preflight. Calling
    // `getShareableContentWithCompletionHandler:` first can enter a TCC path
    // before the caller has received a typed denial.
    let content = get_shareable_content().map_err(|message| CaptureError::StreamStart {
        capture_source: source.to_string(),
        message,
    })?;
    let displays = unsafe { content.displays() };
    if displays.is_empty() {
        return Err(CaptureError::NoDisplays);
    }

    let primary_display_id = CGMainDisplayID();
    let display_id = source.display_id(primary_display_id);
    let display = displays
        .to_vec()
        .into_iter()
        .find(|display| unsafe { display.displayID() } == display_id)
        .ok_or(CaptureError::DisplayNotFound { display_id })?;
    let filter = make_display_filter(&content, &display);

    let (width, height) = bounded_dimensions(
        usize::try_from(unsafe { display.width() }).unwrap_or(MAX_FRAME_WIDTH),
        usize::try_from(unsafe { display.height() }).unwrap_or(MAX_FRAME_HEIGHT),
    );
    let configuration = unsafe { SCStreamConfiguration::new() };
    // SAFETY: These setters are generated bindings for the retained
    // configuration object, and all values are bounded by policy.
    unsafe {
        configuration.setWidth(width);
        configuration.setHeight(height);
        configuration.setPixelFormat(BGRA_PIXEL_FORMAT);
        configuration.setQueueDepth(crate::DEFAULT_QUEUE_DEPTH as isize);
        configuration.setMinimumFrameInterval(CMTime::new(1, 1));
    }

    let inbox = Arc::new(NativeFrameInbox::default());
    let worker = spawn_frame_worker(Arc::clone(&inbox), Arc::clone(&events), &source)?;
    let callbacks = NativeCallbacks::new(CallbackIvars {
        inbox: Arc::clone(&inbox),
        events,
        source: source.to_string(),
    });
    let delegate = ProtocolObject::<dyn SCStreamDelegate>::from_ref(&*callbacks);
    let stream = unsafe {
        SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &configuration,
            Some(delegate),
        )
    };
    let queue_attribute =
        DispatchQueueAttr::with_qos_class(DispatchQueueAttr::SERIAL, DispatchQoS::Utility, 0);
    let queue = DispatchQueue::new("com.stalky.screen-capture", Some(&queue_attribute));
    let mut runtime = NativeRuntime {
        stream: Some(stream),
        callbacks,
        queue,
        inbox,
        worker: Some(worker),
        output_registered: false,
    };

    let output = runtime.output();
    let stream = runtime.stream.as_ref().expect("runtime stream initialized");
    if let Err(error) = unsafe {
        stream.addStreamOutput_type_sampleHandlerQueue_error(
            output,
            SCStreamOutputType::Screen,
            Some(&runtime.queue),
        )
    } {
        let error = CaptureError::OutputHandlerRegistration {
            capture_source: source.to_string(),
            message: error_message(&error),
        };
        return Err(cleanup_start_failure(error, &mut runtime));
    }
    runtime.output_registered = true;

    if let Err(message) = start_stream(stream) {
        let error = CaptureError::StreamStart {
            capture_source: source.to_string(),
            message,
        };
        return Err(cleanup_start_failure(error, &mut runtime));
    }

    Ok(runtime)
}

fn cleanup_start_failure(error: CaptureError, runtime: &mut NativeRuntime) -> CaptureError {
    let mut error = error;
    for message in runtime.shutdown() {
        append_error_message(&mut error, message);
    }
    error
}

fn result_from_messages(messages: Vec<String>) -> Result<(), String> {
    if messages.is_empty() {
        Ok(())
    } else {
        Err(messages.join("; "))
    }
}

struct CallbackIvars {
    inbox: Arc<NativeFrameInbox>,
    events: Arc<dyn CaptureEvents>,
    source: String,
}

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - `NativeCallbacks` has no `Drop` implementation; `define_class!`
    //   owns and releases its Rust ivars with the Objective-C instance.
    #[unsafe(super(NSObject))]
    #[ivars = CallbackIvars]
    struct NativeCallbacks;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for NativeCallbacks {}

    // SAFETY: The selector and argument types match SCStreamOutput.
    unsafe impl SCStreamOutput for NativeCallbacks {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn did_output_sample_buffer(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            output_type: SCStreamOutputType,
        ) {
            if output_type != SCStreamOutputType::Screen {
                return;
            }
            match decode_sample(sample_buffer) {
                Ok(frame) => match self.ivars().inbox.try_push(frame) {
                    PushOutcome::Accepted => {}
                    PushOutcome::Replaced | PushOutcome::Busy | PushOutcome::Closed => {
                        self.ivars().events.record_drop();
                    }
                },
                Err(status) => self.ivars().events.ingest(FrameInput {
                    status,
                    width: 0,
                    height: 0,
                    bytes_per_row: 0,
                    data_size: 0,
                    timestamp_millis: None,
                    data: &[],
                }),
            }
        }
    }

    // SAFETY: The selector and argument types match SCStreamDelegate.
    unsafe impl SCStreamDelegate for NativeCallbacks {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn did_stop_with_error(&self, _stream: &SCStream, error: &NSError) {
            self.ivars().events.record_stream_error(format!(
                "{}: {}",
                self.ivars().source,
                error_message(error)
            ));
        }
    }
);

impl NativeCallbacks {
    fn new(ivars: CallbackIvars) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ivars);
        // SAFETY: `NSObject`'s `init` is the designated initializer for this
        // ivar-only callback object.
        unsafe { msg_send![super(this), init] }
    }
}

fn spawn_frame_worker(
    inbox: Arc<NativeFrameInbox>,
    events: Arc<dyn CaptureEvents>,
    source: &CaptureSource,
) -> Result<JoinHandle<()>, CaptureError> {
    thread::Builder::new()
        .name("stalky-capture-frame-worker".to_owned())
        .spawn(move || frame_worker(inbox, events))
        .map_err(|error| CaptureError::StreamStart {
            capture_source: source.to_string(),
            message: format!("could not create frame worker: {error}"),
        })
}

fn get_shareable_content() -> Result<Retained<SCShareableContent>, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let completion: RcBlock<dyn Fn(*mut SCShareableContent, *mut NSError)> = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let result = if let Some(content) = unsafe { content.as_ref() } {
                // SAFETY: ScreenCaptureKit owns the callback object for the
                // duration of this callback; retaining it gives this thread a
                // stable +1 reference before the callback returns.
                unsafe { Retained::retain(content as *const _ as *mut SCShareableContent) }
                    .ok_or_else(|| "ScreenCaptureKit returned a null content object".to_owned())
            } else if let Some(error) = unsafe { error.as_ref() } {
                Err(error_message(error))
            } else {
                Err("ScreenCaptureKit returned neither content nor an error".to_owned())
            };
            let _ = sender.send(result);
        },
    );
    // SAFETY: The completion block is heap-backed and remains alive for the
    // duration of the Objective-C asynchronous request.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&completion) };
    receiver
        .recv_timeout(COMPLETION_TIMEOUT)
        .map_err(|_| "timed out while enumerating shareable content".to_owned())?
}

fn make_display_filter(
    content: &SCShareableContent,
    display: &SCDisplay,
) -> Retained<SCContentFilter> {
    let applications = unsafe { content.applications() };
    let own_pid = i32::try_from(std::process::id()).ok();
    let own_application =
        applications
            .to_vec()
            .into_iter()
            .find(|application: &Retained<SCRunningApplication>| {
                own_pid == Some(unsafe { application.processID() })
            });
    let excluded_windows: Retained<NSArray<objc2_screen_capture_kit::SCWindow>> =
        NSArray::from_slice(&[]);
    if let Some(application) = own_application {
        let excluded_applications = NSArray::from_slice(&[&*application]);
        // SAFETY: All objects are live retained ScreenCaptureKit objects and
        // the initializer retains the filter inputs as required by ObjC.
        unsafe {
            SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                display,
                &excluded_applications,
                &excluded_windows,
            )
        }
    } else {
        // SAFETY: The display is retained by the content enumeration and the
        // initializer retains it while constructing the filter.
        unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                display,
                &excluded_windows,
            )
        }
    }
}

fn start_stream(stream: &SCStream) -> Result<(), String> {
    wait_stream_completion(|completion| unsafe {
        stream.startCaptureWithCompletionHandler(Some(completion));
    })
}

fn stop_stream(stream: &SCStream) -> Result<(), String> {
    wait_stream_completion(|completion| unsafe {
        stream.stopCaptureWithCompletionHandler(Some(completion));
    })
}

fn wait_stream_completion<F>(operation: F) -> Result<(), String>
where
    F: FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>),
{
    let (sender, receiver) = mpsc::sync_channel(1);
    let completion: RcBlock<dyn Fn(*mut NSError)> = RcBlock::new(move |error: *mut NSError| {
        let result = match unsafe { error.as_ref() } {
            Some(error) => Err(error_message(error)),
            None => Ok(()),
        };
        let _ = sender.send(result);
    });
    operation(&completion);
    match receiver.recv_timeout(COMPLETION_TIMEOUT) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(message)) => Err(message),
        Err(_) => Err("timed out waiting for ScreenCaptureKit completion".to_owned()),
    }
}

fn error_message(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

fn append_error_message(error: &mut CaptureError, message: String) {
    match error {
        CaptureError::StreamStart {
            message: current, ..
        }
        | CaptureError::StreamStop {
            message: current, ..
        }
        | CaptureError::OutputHandlerRegistration {
            message: current, ..
        } => {
            current.push_str("; ");
            current.push_str(&message);
        }
        _ => {}
    }
}

#[derive(Default)]
struct NativeFrameInbox {
    state: Mutex<InboxState>,
    wake: Condvar,
}

#[derive(Default)]
struct InboxState {
    frame: Option<CapturedFrame>,
    closed: bool,
}

impl NativeFrameInbox {
    fn try_push(&self, frame: CapturedFrame) -> PushOutcome {
        let Ok(mut state) = self.state.try_lock() else {
            return PushOutcome::Busy;
        };
        if state.closed {
            return PushOutcome::Closed;
        }
        let outcome = if state.frame.is_some() {
            PushOutcome::Replaced
        } else {
            PushOutcome::Accepted
        };
        state.frame = Some(frame);
        self.wake.notify_one();
        outcome
    }

    fn take(&self) -> Option<CapturedFrame> {
        let mut state = self.state.lock().ok()?;
        loop {
            if let Some(frame) = state.frame.take() {
                return Some(frame);
            }
            if state.closed {
                return None;
            }
            state = self.wake.wait(state).ok()?;
        }
    }

    fn shutdown(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.frame = None;
            self.wake.notify_all();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PushOutcome {
    Accepted,
    Replaced,
    Busy,
    Closed,
}

struct CapturedFrame {
    width: usize,
    height: usize,
    bytes_per_row: usize,
    timestamp_millis: Option<u64>,
    data: Vec<u8>,
}

fn frame_worker(inbox: Arc<NativeFrameInbox>, events: Arc<dyn CaptureEvents>) {
    while let Some(frame) = inbox.take() {
        events.ingest_owned(crate::BgraFrame {
            metadata: crate::FrameMetadata {
                width: frame.width,
                height: frame.height,
                bytes_per_row: frame.bytes_per_row,
                byte_len: frame.data.len(),
                timestamp_millis: frame.timestamp_millis,
            },
            bytes: frame.data,
            digest: 0,
        });
    }
}

fn decode_sample(sample: &CMSampleBuffer) -> Result<CapturedFrame, FrameStatus> {
    if !unsafe { sample.is_valid() } || !unsafe { sample.data_is_ready() } {
        return Err(FrameStatus::Invalid);
    }
    match frame_status(sample) {
        Some(SCFrameStatus::Complete) => {}
        Some(_) => return Err(FrameStatus::Incomplete),
        None => return Err(FrameStatus::Invalid),
    }
    let image = unsafe { sample.image_buffer() }.ok_or(FrameStatus::Invalid)?;
    let pixel_buffer = pixel_buffer_ref(&image);
    if CVPixelBufferIsPlanar(pixel_buffer)
        || CVPixelBufferGetPixelFormatType(pixel_buffer) != BGRA_PIXEL_FORMAT
    {
        return Err(FrameStatus::Invalid);
    }
    let width = CVPixelBufferGetWidth(pixel_buffer);
    let height = CVPixelBufferGetHeight(pixel_buffer);
    if width == 0 || height == 0 || width > MAX_FRAME_WIDTH || height > MAX_FRAME_HEIGHT {
        return Err(FrameStatus::Invalid);
    }
    let bytes_per_row = CVPixelBufferGetBytesPerRow(pixel_buffer);
    let data_size = CVPixelBufferGetDataSize(pixel_buffer);
    let row_bytes = width.checked_mul(4).ok_or(FrameStatus::Invalid)?;
    let required_len = bytes_per_row
        .checked_mul(height)
        .ok_or(FrameStatus::Invalid)?;
    let compact_len = row_bytes.checked_mul(height).ok_or(FrameStatus::Invalid)?;
    if bytes_per_row < row_bytes
        || required_len > MAX_FRAME_BYTES
        || data_size < required_len
        || data_size > MAX_FRAME_BYTES
        || compact_len > MAX_FRAME_BYTES
    {
        return Err(FrameStatus::Invalid);
    }
    let read_lock = PixelBufferReadLock::lock(pixel_buffer)?;
    let base_address = CVPixelBufferGetBaseAddress(pixel_buffer) as *const u8;
    if base_address.is_null() {
        return Err(FrameStatus::Incomplete);
    }
    // SAFETY: The successful read-only lock keeps the pixel buffer's base
    // address valid for `required_len` bytes until `read_lock` is dropped.
    let source = unsafe { slice::from_raw_parts(base_address, required_len) };
    let mut data = Vec::with_capacity(compact_len);
    for row in source.chunks(bytes_per_row).take(height) {
        if row.len() < row_bytes {
            return Err(FrameStatus::Incomplete);
        }
        data.extend_from_slice(&row[..row_bytes]);
    }
    drop(read_lock);
    Ok(CapturedFrame {
        width,
        height,
        bytes_per_row: row_bytes,
        timestamp_millis: timestamp_millis(sample),
        data,
    })
}

struct PixelBufferReadLock<'a> {
    buffer: &'a CVPixelBuffer,
}

impl<'a> PixelBufferReadLock<'a> {
    fn lock(buffer: &'a CVPixelBuffer) -> Result<Self, FrameStatus> {
        if unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) }
            != kCVReturnSuccess
        {
            return Err(FrameStatus::Invalid);
        }
        Ok(Self { buffer })
    }
}

impl Drop for PixelBufferReadLock<'_> {
    fn drop(&mut self) {
        let _ = unsafe {
            CVPixelBufferUnlockBaseAddress(self.buffer, CVPixelBufferLockFlags::ReadOnly)
        };
    }
}

fn pixel_buffer_ref(image: &CFRetained<CVImageBuffer>) -> &CVPixelBuffer {
    // SAFETY: ScreenCaptureKit's image buffer for a screen output is a
    // CVPixelBuffer. The generated CoreVideo API exposes the common result as
    // CVImageBuffer/CVBuffer, so this is the narrow cast at the adapter edge.
    unsafe { &*CFRetained::as_ptr(image).as_ptr().cast::<CVPixelBuffer>() }
}

fn frame_status(sample: &CMSampleBuffer) -> Option<SCFrameStatus> {
    let attachments = unsafe { sample.sample_attachments_array(false) }?;
    if attachments.is_empty() {
        return None;
    }
    let raw_attachments: &CFArray = attachments.as_ref();
    let attachments: &CFArray<CFDictionary<CFString, CFType>> =
        unsafe { raw_attachments.cast_unchecked() };
    let attachment = unsafe { attachments.get_unchecked(0) };
    // ScreenCaptureKit documents the first sample attachment as a dictionary
    // containing SCStreamFrameInfoStatus -> NSNumber. NSNumber is toll-free
    // bridged to CFNumber, so use CoreFoundation's checked type downcast.
    let key: &CFString = unsafe { &*(SCStreamFrameInfoStatus as *const _ as *const CFString) };
    let value = unsafe { attachment.get_unchecked(key) }?;
    let status = value.downcast_ref::<CFNumber>()?.as_isize()?;
    Some(SCFrameStatus(status))
}

fn timestamp_millis(sample: &CMSampleBuffer) -> Option<u64> {
    let time = unsafe { sample.presentation_time_stamp() };
    let value = time.value;
    let timescale = time.timescale;
    if timescale <= 0 || value < 0 {
        return None;
    }
    u64::try_from((i128::from(value) * 1_000) / i128::from(timescale)).ok()
}

fn bounded_dimensions(width: usize, height: usize) -> (usize, usize) {
    let width = width.max(1);
    let height = height.max(1);
    let scale = (MAX_FRAME_WIDTH as f64 / width as f64)
        .min(MAX_FRAME_HEIGHT as f64 / height as f64)
        .min(1.0);
    let bounded_width = ((width as f64 * scale).floor() as usize).max(1);
    let bounded_height = ((height as f64 * scale).floor() as usize).max(1);
    (bounded_width, bounded_height)
}
