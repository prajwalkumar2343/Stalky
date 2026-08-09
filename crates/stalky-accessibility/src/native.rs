//! macOS Accessibility adapter.
//!
//! All `AXUIElement`, `AXObserver`, and CoreFoundation values are created,
//! used, and released on the worker's CFRunLoop thread. The only values sent
//! to the service are bounded Rust data or bounded action commands. The small
//! unsafe blocks below are limited to generated binding pointer contracts and
//! the AX callback's owner-thread refcon; the callback cannot outlive the
//! boxed runtime because notifications are removed before it is dropped.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::ptr::{NonNull, null, null_mut};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use libc::pid_t;
use mega_permissions::PermissionState;
use objc2_application_services::{
    AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions, AXObserver, AXUIElement, AXValue,
    AXValueType, kAXTrustedCheckOptionPrompt,
};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFRunLoop, CFRunLoopSource, CFString,
    CFType, Type, kCFBooleanTrue, kCFRunLoopDefaultMode,
};

use crate::model::{
    AccessibilityAction, AccessibilityActionRequest, AccessibilityActionResult,
    AccessibilityApplication, AccessibilityElementId, AccessibilityEventKind, AccessibilityRect,
    AccessibilitySnapshot,
};
use crate::normalizer::{
    MAX_DEPTH, MAX_NODES, MAX_STRING_CHARS, RawNode, normalize_tree, sanitize,
};
use crate::policy::{ActionBinding, should_rebind_focused_application, validate_action};
use crate::service::{
    AccessibilityBackend, AccessibilityError, AccessibilityEventSink, AccessibilitySession,
};

const START_TIMEOUT: Duration = Duration::from_secs(3);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);
const ACTION_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CHILDREN: usize = 128;
const MAX_BOUND_COORDINATE: f64 = 1_000_000.0;
const OBSERVATION_MESSAGE_TIMEOUT_SECONDS: f32 = 0.25;
const ACTION_MESSAGE_TIMEOUT_SECONDS: f32 = 1.0;
const SNAPSHOT_BUDGET: Duration = Duration::from_millis(750);

const ATTR_ROLE: &str = "AXRole";
const ATTR_SUBROLE: &str = "AXSubrole";
const ATTR_TITLE: &str = "AXTitle";
const ATTR_VALUE: &str = "AXValue";
const ATTR_CHILDREN: &str = "AXChildren";
const ATTR_ENABLED: &str = "AXEnabled";
const ATTR_FOCUSED: &str = "AXFocused";
const ATTR_POSITION: &str = "AXPosition";
const ATTR_SIZE: &str = "AXSize";
const ATTR_FOCUSED_APPLICATION: &str = "AXFocusedApplication";
const ATTR_FOCUSED_WINDOW: &str = "AXFocusedWindow";
const ATTR_FOCUSED_ELEMENT: &str = "AXFocusedUIElement";
const ATTR_BUNDLE_IDENTIFIER: &str = "AXBundleIdentifier";

const NOTIFICATIONS: &[(&str, AccessibilityEventKind)] = &[
    (
        "AXFocusedApplicationChanged",
        AccessibilityEventKind::FocusedApplication,
    ),
    (
        "AXFocusedWindowChanged",
        AccessibilityEventKind::FocusedWindow,
    ),
    (
        "AXFocusedUIElementChanged",
        AccessibilityEventKind::FocusedElement,
    ),
    ("AXWindowCreated", AccessibilityEventKind::WindowCreated),
    ("AXValueChanged", AccessibilityEventKind::ValueChanged),
    (
        "AXSelectedTextChanged",
        AccessibilityEventKind::SelectionChanged,
    ),
    ("AXTitleChanged", AccessibilityEventKind::TitleChanged),
    (
        "AXUIElementDestroyed",
        AccessibilityEventKind::ElementDestroyed,
    ),
];

#[derive(Clone, Copy, Debug)]
pub(crate) struct NativeBackend;

impl NativeBackend {
    pub(crate) const fn new() -> Self {
        Self
    }
}

impl AccessibilityBackend for NativeBackend {
    fn start(
        &self,
        events: Arc<dyn AccessibilityEventSink>,
    ) -> Result<Box<dyn AccessibilitySession>, AccessibilityError> {
        let (commands, receiver) = mpsc::sync_channel(4);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("stalky-accessibility-ax".to_owned())
            .spawn(move || {
                let mut runtime = match NativeRuntime::create(events) {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                        return;
                    }
                };
                if let Err(error) = runtime.install() {
                    let _ = ready_sender.send(Err(error));
                    runtime.shutdown();
                    return;
                }
                let _ = ready_sender.send(Ok(()));
                runtime.run(receiver);
            })
            .map_err(|_| AccessibilityError::WorkerStart)?;

        match ready_receiver.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => Ok(Box::new(NativeSession {
                commands,
                thread: Some(thread),
            })),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let (ack_sender, _) = mpsc::sync_channel(1);
                let _ = commands.try_send(NativeCommand::Stop(ack_sender));
                let _ = thread.join();
                Err(AccessibilityError::Timeout {
                    operation: "observer_start",
                })
            }
        }
    }

    fn request_permission(&self) -> Result<PermissionState, AccessibilityError> {
        // This is called only by the explicit user-triggered request command.
        let key = unsafe { kAXTrustedCheckOptionPrompt };
        let value = unsafe { kCFBooleanTrue }.ok_or(AccessibilityError::Native {
            operation: "accessibility_prompt_value",
            code: -1,
        })?;
        let options = CFDictionary::from_slices(&[key], &[value]);
        let options: &CFDictionary = unsafe { options.cast_unchecked() };
        let trusted = unsafe { AXIsProcessTrustedWithOptions(Some(options)) };
        Ok(if trusted {
            PermissionState::Granted
        } else {
            PermissionState::Denied
        })
    }

    fn permission_status(&self) -> Result<PermissionState, AccessibilityError> {
        // Retesting must never set kAXTrustedCheckOptionPrompt. This path is
        // used by status polling and window-focus rechecks.
        Ok(if unsafe { AXIsProcessTrusted() } {
            PermissionState::Granted
        } else {
            PermissionState::Denied
        })
    }
}

struct NativeSession {
    commands: SyncSender<NativeCommand>,
    thread: Option<JoinHandle<()>>,
}

impl AccessibilitySession for NativeSession {
    fn stop(&mut self) -> Result<(), AccessibilityError> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        let (ack_sender, ack_receiver) = mpsc::sync_channel(1);
        if self.commands.send(NativeCommand::Stop(ack_sender)).is_err() {
            let _ = thread.join();
            return Err(AccessibilityError::WorkerStopped);
        }
        let result =
            ack_receiver
                .recv_timeout(STOP_TIMEOUT)
                .map_err(|_| AccessibilityError::Timeout {
                    operation: "observer_stop",
                });
        let _ = thread.join();
        result.and_then(|result| result)
    }

    fn execute(
        &mut self,
        request: AccessibilityActionRequest,
    ) -> Result<AccessibilityActionResult, AccessibilityError> {
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        self.commands
            .send(NativeCommand::Action(request, result_sender))
            .map_err(|_| AccessibilityError::WorkerStopped)?;
        result_receiver
            .recv_timeout(ACTION_TIMEOUT)
            .map_err(|_| AccessibilityError::Timeout {
                operation: "accessibility_action",
            })?
    }
}

impl Drop for NativeSession {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

enum NativeCommand {
    Stop(SyncSender<Result<(), AccessibilityError>>),
    Action(
        AccessibilityActionRequest,
        SyncSender<Result<AccessibilityActionResult, AccessibilityError>>,
    ),
}

struct Registration {
    element: CFRetained<AXUIElement>,
    notification: CFRetained<CFString>,
}

struct ElementEntry {
    id: AccessibilityElementId,
    element: CFRetained<AXUIElement>,
    supported_actions: Vec<AccessibilityAction>,
    value_settable: bool,
}

struct ReadContext {
    started: Instant,
    visited: HashSet<u64>,
    nodes: usize,
    handles: Vec<CFRetained<AXUIElement>>,
    bindings: Vec<(Vec<AccessibilityAction>, bool)>,
    deadline: Instant,
    deadline_reported: bool,
}

impl ReadContext {
    fn new(deadline: Instant) -> Self {
        Self {
            started: Instant::now(),
            visited: HashSet::new(),
            nodes: 0,
            handles: Vec::new(),
            bindings: Vec::new(),
            deadline,
            deadline_reported: false,
        }
    }
}

struct NativeRuntime {
    events: Arc<dyn AccessibilityEventSink>,
    run_loop: CFRetained<CFRunLoop>,
    source: Option<CFRetained<CFRunLoopSource>>,
    system_wide: CFRetained<AXUIElement>,
    observer: Option<CFRetained<AXObserver>>,
    focused_application: Option<CFRetained<AXUIElement>>,
    focused_window: Option<CFRetained<AXUIElement>>,
    focused_element: Option<CFRetained<AXUIElement>>,
    app_pid: Option<pid_t>,
    own_pid: pid_t,
    generation: u64,
    elements: HashMap<String, ElementEntry>,
    registrations: Vec<Registration>,
    target_registrations: Vec<Registration>,
    next_focus_poll: Instant,
    stopped: bool,
}

impl NativeRuntime {
    fn create(events: Arc<dyn AccessibilityEventSink>) -> Result<Box<Self>, AccessibilityError> {
        if !unsafe { AXIsProcessTrusted() } {
            return Err(AccessibilityError::NotTrusted);
        }
        let run_loop = CFRunLoop::current().ok_or(AccessibilityError::Native {
            operation: "current_run_loop",
            code: -1,
        })?;
        let system_wide = unsafe { AXUIElement::new_system_wide() };
        configure_ax_timeout(
            &system_wide,
            OBSERVATION_MESSAGE_TIMEOUT_SECONDS,
            "system_messaging_timeout",
        )?;
        Ok(Box::new(Self {
            events,
            run_loop,
            source: None,
            system_wide,
            observer: None,
            focused_application: None,
            focused_window: None,
            focused_element: None,
            app_pid: None,
            own_pid: std::process::id() as pid_t,
            generation: 0,
            elements: HashMap::new(),
            registrations: Vec::new(),
            target_registrations: Vec::new(),
            next_focus_poll: Instant::now(),
            stopped: false,
        }))
    }

    fn install(&mut self) -> Result<(), AccessibilityError> {
        self.refresh_focus(true)?;
        Ok(())
    }

    fn run(&mut self, receiver: Receiver<NativeCommand>) {
        while !self.stopped {
            loop {
                match receiver.try_recv() {
                    Ok(NativeCommand::Stop(ack)) => {
                        let result = self.shutdown_result();
                        let _ = ack.send(result);
                        return;
                    }
                    Ok(NativeCommand::Action(request, sender)) => {
                        let _ = sender.send(self.execute_action(request));
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.shutdown();
                        return;
                    }
                }
            }
            if Instant::now() >= self.next_focus_poll {
                self.next_focus_poll = Instant::now() + Duration::from_millis(300);
                if let Err(error) = self.refresh_focus(false) {
                    self.events.record_error(error);
                }
            }
            let _ = CFRunLoop::run_in_mode(default_mode(), 0.05, false);
        }
    }

    fn refresh_focus(&mut self, force: bool) -> Result<(), AccessibilityError> {
        let Some(value) = copy_attribute(&self.system_wide, ATTR_FOCUSED_APPLICATION)? else {
            if force || self.app_pid.is_some() {
                self.remove_registrations();
                if let Some(source) = self.source.take() {
                    self.run_loop.remove_source(Some(&source), default_mode());
                }
                self.observer = None;
                self.focused_application = None;
                self.focused_window = None;
                self.focused_element = None;
                self.app_pid = None;
                self.elements.clear();
                self.publish_empty_snapshot();
            }
            return Ok(());
        };
        let Some(application) = value.downcast::<AXUIElement>().ok() else {
            return Err(AccessibilityError::Native {
                operation: "focused_application_type",
                code: AXError::IllegalArgument.0,
            });
        };
        let mut pid = 0 as pid_t;
        let pid_error = unsafe { application.pid(NonNull::from(&mut pid)) };
        if pid_error != AXError::Success {
            return Err(native_error("focused_application_pid", pid_error));
        }
        if !should_rebind_focused_application(pid, self.own_pid, self.app_pid) {
            if force && self.app_pid.is_none() {
                self.publish_empty_snapshot();
            }
            return Ok(());
        }
        let changed = force || self.app_pid != Some(pid);
        if changed {
            self.rebind_application(pid, application)?;
            self.events
                .record_event(AccessibilityEventKind::FocusedApplication, None);
            self.refresh_snapshot()?;
        }
        Ok(())
    }

    fn rebind_application(
        &mut self,
        pid: pid_t,
        application: CFRetained<AXUIElement>,
    ) -> Result<(), AccessibilityError> {
        self.remove_registrations();
        if let Some(source) = self.source.take() {
            self.run_loop.remove_source(Some(&source), default_mode());
        }
        self.observer = None;
        self.focused_application = Some(application);
        let Some(application) = self.focused_application.as_ref() else {
            return Err(AccessibilityError::WorkerStopped);
        };
        configure_ax_timeout(
            application,
            OBSERVATION_MESSAGE_TIMEOUT_SECONDS,
            "application_messaging_timeout",
        )?;
        self.focused_window = None;
        self.focused_element = None;
        self.elements.clear();
        self.generation = self.generation.wrapping_add(1).max(1);
        self.app_pid = Some(pid);

        let mut observer_ptr: *mut AXObserver = null_mut();
        let error =
            unsafe { AXObserver::create(pid, Some(ax_callback), NonNull::from(&mut observer_ptr)) };
        if error != AXError::Success {
            return Err(native_error("observer_create", error));
        }
        let observer_ptr = NonNull::new(observer_ptr).ok_or(AccessibilityError::Native {
            operation: "observer_create_pointer",
            code: -1,
        })?;
        // AXObserverCreate follows the Create rule and returns +1.
        let observer = unsafe { CFRetained::from_raw(observer_ptr) };
        let source = unsafe { observer.run_loop_source() };
        self.run_loop.add_source(Some(&source), default_mode());
        self.source = Some(source);
        self.observer = Some(observer);
        self.install_notifications_for_application()?;
        let system_wide = self.system_wide.clone();
        self.install_notifications_for_element(&system_wide, true, false);
        Ok(())
    }

    fn install_notifications_for_application(&mut self) -> Result<(), AccessibilityError> {
        let refcon = (self as *mut Self).cast::<c_void>();
        let Some(observer) = self.observer.as_ref() else {
            return Ok(());
        };
        let Some(application) = self.focused_application.as_ref() else {
            return Ok(());
        };
        for (name, kind) in NOTIFICATIONS {
            let notification = CFString::from_static_str(name);
            let error = unsafe { observer.add_notification(application, &notification, refcon) };
            if error == AXError::Success {
                self.registrations.push(Registration {
                    element: application.retain(),
                    notification,
                });
            } else if error == AXError::NotificationUnsupported || error == AXError::NotImplemented
            {
                self.events.record_unsupported();
            } else if error != AXError::NotificationAlreadyRegistered {
                self.events
                    .record_error(native_error(notification_operation(kind), error));
            }
        }
        Ok(())
    }

    fn install_notifications_for_element(
        &mut self,
        element: &CFRetained<AXUIElement>,
        focus_only: bool,
        target_element: bool,
    ) {
        let Some(observer) = self.observer.clone() else {
            return;
        };
        let refcon = (self as *mut Self).cast::<c_void>();
        let pointer = CFRetained::as_ptr(element);
        for (name, kind) in NOTIFICATIONS {
            if focus_only
                && !matches!(
                    kind,
                    AccessibilityEventKind::FocusedApplication
                        | AccessibilityEventKind::FocusedWindow
                        | AccessibilityEventKind::FocusedElement
                )
            {
                continue;
            }
            if self.registrations.iter().any(|registration| {
                CFRetained::as_ptr(&registration.element) == pointer
                    && registration.notification.to_string() == *name
            }) {
                continue;
            }
            let notification = CFString::from_static_str(name);
            let error = unsafe { observer.add_notification(element, &notification, refcon) };
            if error == AXError::Success {
                let registration = Registration {
                    element: element.retain(),
                    notification,
                };
                if target_element {
                    self.target_registrations.push(registration);
                } else {
                    self.registrations.push(registration);
                }
            } else if error == AXError::NotificationUnsupported || error == AXError::NotImplemented
            {
                self.events.record_unsupported();
            } else if error != AXError::NotificationAlreadyRegistered {
                self.events
                    .record_error(native_error(notification_operation(kind), error));
            }
        }
    }

    fn refresh_snapshot(&mut self) -> Result<(), AccessibilityError> {
        // Every rebuild invalidates all previously issued opaque ids, even if
        // the focused application itself did not change.
        self.generation = self.generation.wrapping_add(1).max(1);
        self.remove_target_registrations();
        let focused_application = self.focused_application.clone();
        let focused_window = copy_focused_element(
            focused_application.as_ref(),
            &self.system_wide,
            ATTR_FOCUSED_WINDOW,
        )?;
        let focused_element = copy_focused_element(
            focused_application.as_ref(),
            &self.system_wide,
            ATTR_FOCUSED_ELEMENT,
        )?;
        self.focused_window = focused_window;
        self.focused_element = focused_element;
        if let Some(window) = self.focused_window.as_ref()
            && let Err(error) = configure_ax_timeout(
                window,
                OBSERVATION_MESSAGE_TIMEOUT_SECONDS,
                "window_messaging_timeout",
            )
        {
            self.events.record_error(error);
        }
        if let Some(element) = self.focused_element.as_ref()
            && let Err(error) = configure_ax_timeout(
                element,
                OBSERVATION_MESSAGE_TIMEOUT_SECONDS,
                "element_messaging_timeout",
            )
        {
            self.events.record_error(error);
        }
        self.elements.clear();
        let tree_root = self
            .focused_window
            .clone()
            .or_else(|| self.focused_element.clone())
            .or_else(|| self.focused_application.clone());
        let deadline = Instant::now() + SNAPSHOT_BUDGET;
        let tree = tree_root.as_ref().and_then(|element| {
            let mut context = ReadContext::new(deadline);
            let raw = match self.read_raw(element, 0, &mut context) {
                Ok(Some(raw)) => raw,
                Ok(None) => return None,
                Err(error) => {
                    self.events.record_error(error);
                    return None;
                }
            };
            let tree = normalize_tree(&raw, self.generation);
            for (index, (handle, (supported_actions, value_settable))) in context
                .handles
                .into_iter()
                .zip(context.bindings)
                .enumerate()
            {
                if let Some(id) = element_id(index, self.generation) {
                    self.elements.insert(
                        id.id.clone(),
                        ElementEntry {
                            id,
                            element: handle,
                            supported_actions,
                            value_settable,
                        },
                    );
                }
            }
            tree.root
        });
        let focused_window_for_notifications = self.focused_window.clone();
        let focused_element_for_notifications = self.focused_element.clone();
        if let Some(window) = focused_window_for_notifications.as_ref() {
            self.install_notifications_for_element(window, false, true);
        }
        if let Some(element) = focused_element_for_notifications.as_ref() {
            self.install_notifications_for_element(element, false, true);
        }
        let normalized_window = self
            .focused_window
            .as_ref()
            .and_then(|element| self.read_shallow_node(element));
        let normalized_element = self
            .focused_element
            .as_ref()
            .and_then(|element| self.read_shallow_node(element));
        let snapshot = AccessibilitySnapshot {
            generation: self.generation,
            observed_at_millis: now_millis(),
            application: self.focused_application.as_ref().map(|application| {
                AccessibilityApplication {
                    pid: self.app_pid.unwrap_or_default(),
                    name: self.read_text(application, ATTR_TITLE),
                    bundle_identifier: self.read_text(application, ATTR_BUNDLE_IDENTIFIER),
                }
            }),
            focused_window: normalized_window,
            focused_element: normalized_element,
            tree,
        };
        self.events.publish_snapshot(snapshot);
        Ok(())
    }

    fn read_shallow_node(
        &self,
        element: &CFRetained<AXUIElement>,
    ) -> Option<crate::AccessibilityNode> {
        let role = self.read_text(element, ATTR_ROLE);
        let secure = role.as_deref() == Some("AXSecureTextField");
        let value = if secure {
            Some("[redacted]".to_owned())
        } else {
            self.read_text(element, ATTR_VALUE)
        };
        let bounds = match read_rect(element) {
            Ok(bounds) => bounds,
            Err(error) => {
                self.events.record_error(error);
                None
            }
        };
        let enabled = match read_bool(element, ATTR_ENABLED) {
            Ok(enabled) => enabled,
            Err(error) => {
                self.events.record_error(error);
                None
            }
        };
        let focused = match read_bool(element, ATTR_FOCUSED) {
            Ok(focused) => focused,
            Err(error) => {
                self.events.record_error(error);
                None
            }
        };
        let children_count = match read_children_count(element) {
            Ok(count) => count,
            Err(error) => {
                self.events.record_error(error);
                0
            }
        };
        Some(crate::AccessibilityNode {
            element: None,
            role,
            subrole: self.read_text(element, ATTR_SUBROLE),
            title: self.read_text(element, ATTR_TITLE),
            value,
            bounds,
            enabled,
            focused,
            children_count,
            children: Vec::new(),
            truncated: children_count > 0,
            supported_actions: Vec::new(),
            value_settable: false,
        })
    }

    fn read_raw(
        &self,
        element: &CFRetained<AXUIElement>,
        depth: usize,
        context: &mut ReadContext,
    ) -> Result<Option<RawNode>, AccessibilityError> {
        let key = CFRetained::as_ptr(element).as_ptr() as usize as u64;
        if Instant::now() >= context.deadline || !snapshot_budget_allows(context.started.elapsed())
        {
            if !context.deadline_reported {
                context.deadline_reported = true;
                self.events.record_error(AccessibilityError::Timeout {
                    operation: "snapshot_budget",
                });
            }
            return Ok(None);
        }
        if depth > MAX_DEPTH || context.nodes >= MAX_NODES || !context.visited.insert(key) {
            return Ok(None);
        }
        context.nodes += 1;
        context.handles.push(element.retain());
        let role = self.read_text_result(element, ATTR_ROLE)?;
        let secure = role.as_deref() == Some("AXSecureTextField");
        let supported_actions = read_actions(element)?;
        let value_settable = read_value_settable(element, role.as_deref())?;
        context
            .bindings
            .push((supported_actions.clone(), value_settable));
        let subrole = self.read_text_result(element, ATTR_SUBROLE)?;
        let title = self.read_text_result(element, ATTR_TITLE)?;
        let value = if secure {
            None
        } else {
            self.read_text_result(element, ATTR_VALUE)?
        };
        let bounds = read_rect(element)?;
        let enabled = read_bool(element, ATTR_ENABLED)?;
        let focused = read_bool(element, ATTR_FOCUSED)?;
        let mut truncated = false;
        let mut children = Vec::new();
        for child in read_children(element)?.into_iter().take(MAX_CHILDREN) {
            if let Err(error) = configure_ax_timeout(
                &child,
                OBSERVATION_MESSAGE_TIMEOUT_SECONDS,
                "child_messaging_timeout",
            ) {
                self.events.record_error(error);
                truncated = true;
                continue;
            }
            match self.read_raw(&child, depth + 1, context) {
                Ok(Some(child)) => children.push(child),
                Ok(None) => truncated = true,
                Err(error) => {
                    self.events.record_error(error);
                    truncated = true;
                }
            }
        }
        Ok(Some(RawNode {
            key,
            role,
            subrole,
            title,
            value,
            bounds,
            enabled,
            focused,
            secure,
            supported_actions,
            value_settable,
            children,
            truncated,
        }))
    }

    fn read_text(&self, element: &AXUIElement, name: &str) -> Option<String> {
        match self.read_text_result(element, name) {
            Ok(value) => value,
            Err(error) => {
                self.events.record_error(error);
                None
            }
        }
    }

    fn read_text_result(
        &self,
        element: &AXUIElement,
        name: &str,
    ) -> Result<Option<String>, AccessibilityError> {
        read_string(element, name)
    }

    fn execute_action(
        &mut self,
        request: AccessibilityActionRequest,
    ) -> Result<AccessibilityActionResult, AccessibilityError> {
        let Some(entry) = self.elements.get(&request.element.id) else {
            self.events.record_stale();
            return Err(AccessibilityError::ActionRejected {
                reason: crate::ActionPolicyError::StaleElement,
            });
        };
        let binding = ActionBinding {
            element: entry.id.clone(),
            supported_actions: entry.supported_actions.clone(),
            value_settable: entry.value_settable,
        };
        validate_action(&request, &binding)
            .map_err(|reason| AccessibilityError::ActionRejected { reason })?;
        let entry = self
            .elements
            .get(&request.element.id)
            .ok_or(AccessibilityError::WorkerStopped)?;
        configure_ax_timeout(
            &entry.element,
            ACTION_MESSAGE_TIMEOUT_SECONDS,
            "action_messaging_timeout",
        )?;
        let live_actions = read_actions(&entry.element)?;
        let live_role = read_string(&entry.element, ATTR_ROLE)?;
        let live_settable = read_value_settable(&entry.element, live_role.as_deref())?;
        let live_binding = ActionBinding {
            element: entry.id.clone(),
            supported_actions: live_actions,
            value_settable: live_settable,
        };
        validate_action(&request, &live_binding)
            .map_err(|reason| AccessibilityError::ActionRejected { reason })?;
        let error = if request.action == AccessibilityAction::Focus {
            let focused = unsafe { kCFBooleanTrue }.ok_or(AccessibilityError::Native {
                operation: "focused_value",
                code: -1,
            })?;
            unsafe {
                entry
                    .element
                    .set_attribute_value(&ax_key(ATTR_FOCUSED), focused)
            }
        } else if request.action == AccessibilityAction::SetValue {
            let value = CFString::from_str(request.value.as_deref().unwrap_or_default());
            unsafe {
                entry
                    .element
                    .set_attribute_value(&ax_key(ATTR_VALUE), value.as_ref())
            }
        } else {
            let Some(action_name) = performed_action_name(request.action) else {
                return Err(AccessibilityError::ActionRejected {
                    reason: crate::ActionPolicyError::UnsupportedAction,
                });
            };
            let action_name = CFString::from_static_str(action_name);
            unsafe { entry.element.perform_action(&action_name) }
        };
        if error != AXError::Success {
            return Err(native_error("perform_accessibility_action", error));
        }
        Ok(AccessibilityActionResult {
            executed: true,
            element: request.element,
            action: request.action,
        })
    }

    fn publish_empty_snapshot(&self) {
        self.events.publish_snapshot(AccessibilitySnapshot {
            generation: self.generation,
            observed_at_millis: now_millis(),
            application: None,
            focused_window: None,
            focused_element: None,
            tree: None,
        });
    }

    fn handle_notification(&mut self, element: &AXUIElement, notification: &CFString) {
        let name = notification.to_string();
        let Some((_, kind)) = NOTIFICATIONS
            .iter()
            .find(|(registered_name, _)| *registered_name == name)
        else {
            return;
        };
        let token = self.token_for(element);
        if *kind == AccessibilityEventKind::ElementDestroyed {
            self.elements
                .retain(|_, entry| CFRetained::as_ptr(&entry.element) != NonNull::from(element));
            if token.is_none() {
                self.events.record_stale();
            }
        }
        self.events.record_event(kind.clone(), token);
        if *kind == AccessibilityEventKind::FocusedApplication {
            if let Err(error) = self.refresh_focus(false) {
                self.events.record_error(error);
            }
        } else if let Err(error) = self.refresh_snapshot() {
            self.events.record_error(error);
        }
    }

    fn token_for(&self, element: &AXUIElement) -> Option<AccessibilityElementId> {
        let ptr = NonNull::from(element);
        self.elements
            .values()
            .find(|entry| CFRetained::as_ptr(&entry.element) == ptr)
            .map(|entry| entry.id.clone())
    }

    fn remove_registrations(&mut self) {
        if let Some(observer) = self.observer.as_ref() {
            for registration in self.registrations.drain(..) {
                let error = unsafe {
                    observer.remove_notification(&registration.element, &registration.notification)
                };
                if error != AXError::Success && error != AXError::NotificationNotRegistered {
                    self.events
                        .record_error(native_error("remove_notification", error));
                }
            }
            for registration in self.target_registrations.drain(..) {
                let error = unsafe {
                    observer.remove_notification(&registration.element, &registration.notification)
                };
                if error != AXError::Success && error != AXError::NotificationNotRegistered {
                    self.events
                        .record_error(native_error("remove_target_notification", error));
                }
            }
        } else {
            self.registrations.clear();
            self.target_registrations.clear();
        }
    }

    fn remove_target_registrations(&mut self) {
        let Some(observer) = self.observer.clone() else {
            self.target_registrations.clear();
            return;
        };
        for registration in self.target_registrations.drain(..) {
            let error = unsafe {
                observer.remove_notification(&registration.element, &registration.notification)
            };
            if error != AXError::Success && error != AXError::NotificationNotRegistered {
                self.events
                    .record_error(native_error("remove_target_notification", error));
            }
        }
    }

    fn shutdown_result(&mut self) -> Result<(), AccessibilityError> {
        self.shutdown();
        Ok(())
    }

    fn shutdown(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.remove_registrations();
        if let Some(source) = self.source.take() {
            self.run_loop.remove_source(Some(&source), default_mode());
        }
        self.run_loop.stop();
        self.observer = None;
        self.elements.clear();
        self.focused_element = None;
        self.focused_window = None;
        self.focused_application = None;
        self.app_pid = None;
    }
}

// SAFETY: AX invokes this callback on the run loop that owns `NativeRuntime`.
// The refcon is a stable Box pointer for the lifetime of all registrations.
unsafe extern "C-unwind" fn ax_callback(
    _observer: NonNull<AXObserver>,
    element: NonNull<AXUIElement>,
    notification: NonNull<CFString>,
    refcon: *mut c_void,
) {
    if refcon.is_null() {
        return;
    }
    // SAFETY: `refcon` was installed from `Box<NativeRuntime>` and the owner
    // thread removes registrations before dropping that box.
    let runtime = unsafe { &mut *refcon.cast::<NativeRuntime>() };
    // SAFETY: objc2's callback pointers are valid for this callback.
    let element = unsafe { element.as_ref() };
    let notification = unsafe { notification.as_ref() };
    runtime.handle_notification(element, notification);
}

fn copy_attribute(
    element: &AXUIElement,
    name: &str,
) -> Result<Option<CFRetained<CFType>>, AccessibilityError> {
    let mut raw: *const CFType = null();
    let key = ax_key(name);
    let error = unsafe { element.copy_attribute_value(&key, NonNull::from(&mut raw)) };
    if error == AXError::Success {
        let Some(raw) = NonNull::new(raw.cast_mut()) else {
            return Ok(None);
        };
        // AXUIElementCopyAttributeValue returns a retained CFType on success.
        return Ok(Some(unsafe { CFRetained::from_raw(raw) }));
    }
    if matches!(
        error,
        AXError::NoValue | AXError::AttributeUnsupported | AXError::InvalidUIElement
    ) {
        Ok(None)
    } else {
        Err(native_error("copy_attribute_value", error))
    }
}

fn copy_focused_element(
    primary: Option<&CFRetained<AXUIElement>>,
    fallback: &AXUIElement,
    name: &str,
) -> Result<Option<CFRetained<AXUIElement>>, AccessibilityError> {
    if let Some(primary) = primary
        && let Some(value) = copy_attribute(primary, name)?
    {
        return Ok(value.downcast::<AXUIElement>().ok());
    }
    Ok(copy_attribute(fallback, name)?.and_then(|value| value.downcast::<AXUIElement>().ok()))
}

fn read_string(element: &AXUIElement, name: &str) -> Result<Option<String>, AccessibilityError> {
    Ok(copy_attribute(element, name)?.and_then(|value| {
        value
            .downcast_ref::<CFString>()
            .and_then(|value| sanitize(Some(&value.to_string()), MAX_STRING_CHARS))
    }))
}

fn read_bool(element: &AXUIElement, name: &str) -> Result<Option<bool>, AccessibilityError> {
    let Some(value) = copy_attribute(element, name)? else {
        return Ok(None);
    };
    if let Some(boolean) = value.downcast_ref::<CFBoolean>() {
        return Ok(Some(boolean.value()));
    }
    Ok(value.downcast_ref::<CFNumber>().and_then(|number| {
        let mut value = 0_i32;
        if unsafe {
            number.value(
                objc2_core_foundation::CFNumberType::IntType,
                (&mut value as *mut i32).cast(),
            )
        } {
            Some(value != 0)
        } else {
            None
        }
    }))
}

fn read_rect(element: &AXUIElement) -> Result<Option<AccessibilityRect>, AccessibilityError> {
    let Some(position) = copy_attribute(element, ATTR_POSITION)? else {
        return Ok(None);
    };
    let Some(size) = copy_attribute(element, ATTR_SIZE)? else {
        return Ok(None);
    };
    let Some(x) = read_ax_point(&position) else {
        return Ok(None);
    };
    let Some(y) = read_ax_point_y(&position) else {
        return Ok(None);
    };
    let Some(width) = read_ax_size_width(&size) else {
        return Ok(None);
    };
    let Some(height) = read_ax_size_height(&size) else {
        return Ok(None);
    };
    if [x, y, width, height]
        .into_iter()
        .all(|value| value.is_finite() && value.abs() <= MAX_BOUND_COORDINATE)
    {
        Ok(Some(AccessibilityRect {
            x,
            y,
            width,
            height,
        }))
    } else {
        Ok(None)
    }
}

fn read_ax_point(value: &CFType) -> Option<f64> {
    let ax = value.downcast_ref::<AXValue>()?;
    if unsafe { ax.r#type() } != AXValueType::CGPoint {
        return None;
    }
    let mut point = MaybeUninit::<objc2_core_foundation::CGPoint>::uninit();
    if unsafe {
        ax.value(
            AXValueType::CGPoint,
            NonNull::new(point.as_mut_ptr().cast()).unwrap(),
        )
    } {
        Some(unsafe { point.assume_init() }.x)
    } else {
        None
    }
}

fn read_ax_point_y(value: &CFType) -> Option<f64> {
    let ax = value.downcast_ref::<AXValue>()?;
    if unsafe { ax.r#type() } != AXValueType::CGPoint {
        return None;
    }
    let mut point = MaybeUninit::<objc2_core_foundation::CGPoint>::uninit();
    if unsafe {
        ax.value(
            AXValueType::CGPoint,
            NonNull::new(point.as_mut_ptr().cast()).unwrap(),
        )
    } {
        Some(unsafe { point.assume_init() }.y)
    } else {
        None
    }
}

fn read_ax_size_width(value: &CFType) -> Option<f64> {
    let ax = value.downcast_ref::<AXValue>()?;
    if unsafe { ax.r#type() } != AXValueType::CGSize {
        return None;
    }
    let mut size = MaybeUninit::<objc2_core_foundation::CGSize>::uninit();
    if unsafe {
        ax.value(
            AXValueType::CGSize,
            NonNull::new(size.as_mut_ptr().cast()).unwrap(),
        )
    } {
        Some(unsafe { size.assume_init() }.width)
    } else {
        None
    }
}

fn read_ax_size_height(value: &CFType) -> Option<f64> {
    let ax = value.downcast_ref::<AXValue>()?;
    if unsafe { ax.r#type() } != AXValueType::CGSize {
        return None;
    }
    let mut size = MaybeUninit::<objc2_core_foundation::CGSize>::uninit();
    if unsafe {
        ax.value(
            AXValueType::CGSize,
            NonNull::new(size.as_mut_ptr().cast()).unwrap(),
        )
    } {
        Some(unsafe { size.assume_init() }.height)
    } else {
        None
    }
}

fn read_children(
    element: &AXUIElement,
) -> Result<Vec<CFRetained<AXUIElement>>, AccessibilityError> {
    let Some(value) = copy_attribute(element, ATTR_CHILDREN)? else {
        return Ok(Vec::new());
    };
    let Ok(array) = value.downcast::<CFArray>() else {
        return Ok(Vec::new());
    };
    let array: &CFArray<CFType> = unsafe { (*array).cast_unchecked() };
    Ok((0..array.len().min(MAX_CHILDREN))
        .filter_map(|index| array.get(index)?.downcast::<AXUIElement>().ok())
        .collect())
}

fn read_children_count(element: &AXUIElement) -> Result<usize, AccessibilityError> {
    let Some(value) = copy_attribute(element, ATTR_CHILDREN)? else {
        return Ok(0);
    };
    let Ok(array) = value.downcast::<CFArray>() else {
        return Ok(0);
    };
    Ok(array.len().min(MAX_CHILDREN))
}

fn read_actions(element: &AXUIElement) -> Result<Vec<AccessibilityAction>, AccessibilityError> {
    let mut raw: *const CFArray = null();
    let error = unsafe { element.copy_action_names(NonNull::from(&mut raw)) };
    if matches!(
        error,
        AXError::ActionUnsupported
            | AXError::AttributeUnsupported
            | AXError::NoValue
            | AXError::NotImplemented
    ) {
        let mut actions = Vec::new();
        if read_focus_settable(element)? {
            actions.push(AccessibilityAction::Focus);
        }
        return Ok(actions);
    }
    if error != AXError::Success {
        return Err(native_error("copy_action_names", error));
    }
    let Some(raw) = NonNull::new(raw.cast_mut()) else {
        return Ok(Vec::new());
    };
    let array: CFRetained<CFArray> = unsafe { CFRetained::from_raw(raw) };
    let array: &CFArray<CFType> = unsafe { (*array).cast_unchecked() };
    let mut actions = Vec::new();
    for index in 0..array.len().min(32) {
        let Some(value) = array.get(index) else {
            continue;
        };
        let Some(name) = value.downcast_ref::<CFString>().map(ToString::to_string) else {
            continue;
        };
        if let Some(action) = parse_action(&name) {
            actions.push(action);
        }
    }
    if read_focus_settable(element)? && !actions.contains(&AccessibilityAction::Focus) {
        actions.push(AccessibilityAction::Focus);
    }
    Ok(actions)
}

fn read_value_settable(
    element: &AXUIElement,
    role: Option<&str>,
) -> Result<bool, AccessibilityError> {
    if !read_attribute_settable(element, ATTR_VALUE)? {
        return Ok(false);
    }
    if matches!(
        role,
        Some("AXTextField" | "AXTextArea" | "AXSearchField" | "AXComboBox" | "AXSecureTextField")
    ) {
        return Ok(true);
    }
    let Some(value) = copy_attribute(element, ATTR_VALUE)? else {
        return Ok(false);
    };
    Ok(value.downcast_ref::<CFString>().is_some())
}

fn read_focus_settable(element: &AXUIElement) -> Result<bool, AccessibilityError> {
    read_attribute_settable(element, ATTR_FOCUSED)
}

fn read_attribute_settable(
    element: &AXUIElement,
    attribute: &str,
) -> Result<bool, AccessibilityError> {
    let mut settable = 0_u8;
    let error =
        unsafe { element.is_attribute_settable(&ax_key(attribute), NonNull::from(&mut settable)) };
    if matches!(
        error,
        AXError::AttributeUnsupported | AXError::NoValue | AXError::NotImplemented
    ) {
        Ok(false)
    } else if error == AXError::Success {
        Ok(settable != 0)
    } else {
        Err(native_error("is_attribute_settable", error))
    }
}

fn parse_action(name: &str) -> Option<AccessibilityAction> {
    Some(match name {
        "AXPress" => AccessibilityAction::Press,
        "AXIncrement" => AccessibilityAction::Increment,
        "AXDecrement" => AccessibilityAction::Decrement,
        "AXShowMenu" => AccessibilityAction::ShowMenu,
        "AXRaise" => AccessibilityAction::Raise,
        _ => return None,
    })
}

fn performed_action_name(action: AccessibilityAction) -> Option<&'static str> {
    Some(match action {
        AccessibilityAction::Press => "AXPress",
        AccessibilityAction::Increment => "AXIncrement",
        AccessibilityAction::Decrement => "AXDecrement",
        AccessibilityAction::ShowMenu => "AXShowMenu",
        AccessibilityAction::Raise => "AXRaise",
        AccessibilityAction::Focus | AccessibilityAction::SetValue => return None,
    })
}

fn ax_key(name: &str) -> CFRetained<CFString> {
    CFString::from_str(name)
}

fn default_mode() -> Option<&'static objc2_core_foundation::CFRunLoopMode> {
    unsafe { kCFRunLoopDefaultMode }
}

fn element_id(index: usize, generation: u64) -> Option<AccessibilityElementId> {
    Some(AccessibilityElementId {
        id: format!("e{index}"),
        generation,
    })
}

fn native_error(operation: &'static str, error: AXError) -> AccessibilityError {
    AccessibilityError::Native {
        operation,
        code: error.0,
    }
}

fn configure_ax_timeout(
    element: &AXUIElement,
    timeout_seconds: f32,
    operation: &'static str,
) -> Result<(), AccessibilityError> {
    let error = unsafe { element.set_messaging_timeout(timeout_seconds) };
    if error == AXError::Success {
        Ok(())
    } else {
        Err(native_error(operation, error))
    }
}

fn snapshot_budget_allows(elapsed: Duration) -> bool {
    elapsed < SNAPSHOT_BUDGET
}

fn notification_operation(kind: &AccessibilityEventKind) -> &'static str {
    match kind {
        AccessibilityEventKind::FocusedApplication => "notification_focused_application",
        AccessibilityEventKind::FocusedWindow => "notification_focused_window",
        AccessibilityEventKind::FocusedElement => "notification_focused_element",
        AccessibilityEventKind::WindowCreated => "notification_window_created",
        AccessibilityEventKind::ValueChanged => "notification_value_changed",
        AccessibilityEventKind::SelectionChanged => "notification_selection_changed",
        AccessibilityEventKind::TitleChanged => "notification_title_changed",
        AccessibilityEventKind::ElementDestroyed => "notification_element_destroyed",
    }
}

fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::performed_action_name;
    use crate::AccessibilityAction;
    use std::time::Duration;

    #[test]
    fn focus_is_not_dispatched_as_ax_focus_action() {
        assert_eq!(performed_action_name(AccessibilityAction::Focus), None);
        assert_eq!(
            performed_action_name(AccessibilityAction::Press),
            Some("AXPress")
        );
    }

    #[test]
    fn snapshot_budget_is_strictly_bounded() {
        assert!(super::snapshot_budget_allows(Duration::from_millis(749)));
        assert!(!super::snapshot_budget_allows(Duration::from_millis(750)));
    }
}
