//! Synthetic mouse input without blocking the main thread (M9, G4).
//!
//! `CGEvent::post` only queues events, so warping the cursor back right
//! after posting leaves it on the item. Each batch therefore tags its events
//! in field 42, posts them, and restores the cursor once a listen-only,
//! tail-append session tap has seen the last tag, or after
//! `CURSOR_RESTORE_FALLBACK`, whichever comes first. The tap, the timers and
//! every callback run on the main run loop; nothing waits in a loop. The
//! timers fire in the common run loop modes, so an open menu or a control
//! tracking the mouse does not hold a posted mouse-down or the hidden
//! cursor. One batch runs at a time; a second one is refused as busy.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopMode, CFRunLoopSource, CGPoint,
    kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayHideCursor, CGDisplayShowCursor, CGEvent, CGEventField,
    CGEventFlags, CGEventMask, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType, CGMainDisplayID,
    CGMouseButton, CGWarpMouseCursorPosition,
};
use objc2_foundation::NSTimer;

use super::timer;
use crate::input_tag::{RestoreGate, TagSource, is_ours};
use crate::model::{
    CURSOR_RESTORE_FALLBACK, CURSOR_RESTORE_SETTLE, FIELD_CLICK_STATE,
    FIELD_EVENT_SOURCE_USER_DATA, FIELD_WINDOW_THAT_CAN_HANDLE, FIELD_WINDOW_UNDER_POINTER,
    KEY_CODE_COMMAND, Rect, WindowId,
};

/// Current pointer location in CG space.
pub fn pointer_location() -> Option<(f64, f64)> {
    let event = CGEvent::new(None)?;
    let p = CGEvent::location(Some(&event));
    Some((p.x, p.y))
}

/// Why a batch's cursor restore happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restore {
    /// The session tap saw the batch's last event.
    TapSawLastEvent,
    /// The fallback timer fired first, or no tap could be created.
    Fallback,
    /// The batch was cancelled; the rest of it was posted at once.
    Cancelled,
}

/// Called on the main thread once the cursor is back.
pub type DoneHandler = Box<dyn FnOnce(Restore)>;

/// A pacer tick this late means the main thread stalled; the rest of the
/// batch is then posted at once.
const PACER_STALL: Duration = Duration::from_millis(100);

#[derive(Debug, PartialEq, Eq)]
pub enum InputError {
    NoEventSource,
    NoEvent,
    /// Another batch is still in flight.
    Busy,
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputError::NoEventSource => write!(f, "could not create an event source"),
            InputError::NoEvent => write!(f, "could not create a mouse event"),
            InputError::Busy => write!(f, "other synthetic input is still in progress"),
        }
    }
}

impl std::error::Error for InputError {}

/// Hides the cursor and remembers where it was. Dropping it warps the
/// cursor back and shows it again.
struct CursorHold {
    saved: Option<CGPoint>,
    display: CGDirectDisplayID,
}

impl CursorHold {
    fn hide() -> Self {
        let saved = pointer_location().map(|(x, y)| CGPoint::new(x, y));
        let display = CGMainDisplayID();
        let _ = CGDisplayHideCursor(display);
        Self { saved, display }
    }
}

impl Drop for CursorHold {
    fn drop(&mut self) {
        if let Some(saved) = self.saved {
            let _ = CGWarpMouseCursorPosition(saved);
        }
        let _ = CGDisplayShowCursor(self.display);
    }
}

fn common_modes() -> Option<&'static CFRunLoopMode> {
    // SAFETY: kCFRunLoopCommonModes is an immutable CFString constant
    // exported by CoreFoundation.
    unsafe { kCFRunLoopCommonModes }
}

/// A listen-only session tap attached to the main run loop. Dropping it
/// detaches and invalidates it.
struct Tap {
    port: CFRetained<CFMachPort>,
    source: CFRetained<CFRunLoopSource>,
}

impl Tap {
    fn install(mask: CGEventMask) -> Option<Self> {
        // SAFETY: `tap_callback` matches CGEventTapCallBack and only reads
        // the event. The user info pointer is null and never read.
        let port = unsafe {
            CGEvent::tap_create(
                CGEventTapLocation::SessionEventTap,
                CGEventTapPlacement::TailAppendEventTap,
                CGEventTapOptions::ListenOnly,
                mask,
                Some(tap_callback),
                ptr::null_mut(),
            )
        }?;
        let source = CFMachPort::new_run_loop_source(None, Some(&port), 0)?;
        CFRunLoop::main()?.add_source(Some(&source), common_modes());
        CGEvent::tap_enable(&port, true);
        Some(Self { port, source })
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        CGEvent::tap_enable(&self.port, false);
        if let Some(main) = CFRunLoop::main() {
            main.remove_source(Some(&self.source), common_modes());
        }
        self.port.invalidate();
    }
}

/// The batch in flight. One at a time.
struct Batch {
    gate: RestoreGate,
    queued: VecDeque<CFRetained<CGEvent>>,
    location: CGEventTapLocation,
    last_post: Instant,
    pacer: Option<Retained<NSTimer>>,
    /// Posts whatever is still queued once the whole batch is overdue.
    deadline: Option<Retained<NSTimer>>,
    fallback: Option<Retained<NSTimer>>,
    // Dropped in declaration order after the timers: the tap first, then
    // the cursor comes back.
    tap: Option<Tap>,
    cursor: Option<CursorHold>,
    on_done: Option<DoneHandler>,
    /// Post a Command key-up after the last event (L4).
    release_command: bool,
}

thread_local! {
    static ACTIVE: RefCell<Option<Batch>> = const { RefCell::new(None) };
    static TAGS: RefCell<TagSource> = RefCell::new(TagSource::new(std::process::id()));
}

/// Posts the batch's remaining events, tears it down and reports.
fn finish(mut batch: Batch, cause: Restore) {
    for timer in [
        batch.pacer.take(),
        batch.deadline.take(),
        batch.fallback.take(),
    ]
    .into_iter()
    .flatten()
    {
        timer.invalidate();
    }
    // Whatever was not posted yet includes the mouse up, so every posted
    // down gets its up on every path.
    for event in batch.queued.drain(..) {
        CGEvent::post(batch.location, Some(&event));
    }
    if batch.release_command {
        release_command_key();
    }
    drop(batch.tap.take());
    drop(batch.cursor.take());
    if let Some(done) = batch.on_done.take() {
        done(cause);
    }
}

/// Posts a Command key-up with no modifier flags at the HID tap. Mouse
/// events posted there with the Command flag leave Command down in the HID
/// system state until a key event clears it (L4, observed on macOS 26.3).
/// A key-up while Command is already up changes nothing.
fn release_command_key() {
    let Ok(source) = source() else {
        eprintln!("ice-rust: no event source to release the Command key");
        return;
    };
    match CGEvent::new_keyboard_event(Some(&source), KEY_CODE_COMMAND, false) {
        Some(up) => {
            CGEvent::set_flags(Some(&up), CGEventFlags(0));
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up));
        }
        None => eprintln!("ice-rust: could not create the Command key-up event"),
    }
}

/// Takes the active batch if its last tag is `last` (and, for the
/// fallback, if the tap has not already claimed the restore).
fn take_active(last: i64, fallback: bool) -> Option<Batch> {
    ACTIVE.with(|active| {
        let mut active = active.try_borrow_mut().ok()?;
        let batch = active.as_mut()?;
        if batch.gate.last_tag() != last {
            return None;
        }
        if fallback && !batch.gate.expire() {
            return None;
        }
        active.take()
    })
}

fn complete(last: i64, cause: Restore) {
    if let Some(batch) = take_active(last, cause == Restore::Fallback) {
        finish(batch, cause);
    }
}

fn arm_fallback(mtm: MainThreadMarker, last: i64) -> Retained<NSTimer> {
    timer::after_common(mtm, CURSOR_RESTORE_FALLBACK, move || {
        complete(last, Restore::Fallback)
    })
}

/// Whether the batch in flight, if any, has `last` as its last tag.
fn is_active(last: i64) -> bool {
    ACTIVE.with(|active| {
        active
            .try_borrow()
            .ok()
            .and_then(|a| a.as_ref().map(|b| b.gate.last_tag() == last))
            .unwrap_or(false)
    })
}

/// Posts everything still queued in the batch whose last tag is `last`.
fn flush(mtm: MainThreadMarker, last: i64) {
    if is_active(last) {
        while !post_next(mtm) {}
    }
}

/// Whether the batch whose last tag is `last` posted nothing for longer
/// than `PACER_STALL`.
fn stalled(last: i64) -> bool {
    ACTIVE.with(|active| {
        active
            .try_borrow()
            .ok()
            .and_then(|a| {
                a.as_ref()
                    .filter(|b| b.gate.last_tag() == last)
                    .map(|b| b.last_post.elapsed() > PACER_STALL)
            })
            .unwrap_or(false)
    })
}

/// Posts the next queued event. Returns true when the queue is empty.
fn post_next(mtm: MainThreadMarker) -> bool {
    ACTIVE.with(|active| {
        let Ok(mut active) = active.try_borrow_mut() else {
            return true;
        };
        let Some(batch) = active.as_mut() else {
            return true;
        };
        if let Some(event) = batch.queued.pop_front() {
            CGEvent::post(batch.location, Some(&event));
            batch.last_post = Instant::now();
        }
        if !batch.queued.is_empty() {
            return false;
        }
        for timer in [batch.pacer.take(), batch.deadline.take()]
            .into_iter()
            .flatten()
        {
            timer.invalidate();
        }
        if batch.fallback.is_none() && !batch.gate.is_done() {
            batch.fallback = Some(arm_fallback(mtm, batch.gate.last_tag()));
        }
        true
    })
}

/// The session tap's callback. It runs on the main run loop, where the tap's
/// source is attached, and only reads the event.
///
/// # Safety
///
/// Called by the window server with a valid event for the duration of the
/// call, as `CGEventTapCallBack` documents.
unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    _user_info: *mut c_void,
) -> *mut CGEvent {
    if kind == CGEventType::TapDisabledByTimeout || kind == CGEventType::TapDisabledByUserInput {
        ACTIVE.with(|active| {
            if let Ok(active) = active.try_borrow()
                && let Some(tap) = active.as_ref().and_then(|b| b.tap.as_ref())
            {
                CGEvent::tap_enable(&tap.port, true);
            }
        });
        return event.as_ptr();
    }
    // SAFETY: the window server passes a valid event that lives for the
    // duration of the callback.
    let observed = unsafe { event.as_ref() };
    let tag =
        CGEvent::integer_value_field(Some(observed), CGEventField(FIELD_EVENT_SOURCE_USER_DATA));
    if is_ours(tag) {
        let seen_last = ACTIVE.with(|active| {
            active
                .try_borrow_mut()
                .ok()
                .and_then(|mut active| active.as_mut().map(|b| b.gate.observe(tag)))
                .unwrap_or(false)
        });
        // The tap runs on the main run loop. Tear down on the next pass, not
        // inside the tap's own callback, and give the event a moment to
        // reach its target.
        if seen_last && let Some(mtm) = MainThreadMarker::new() {
            timer::after_common(mtm, CURSOR_RESTORE_SETTLE, move || {
                complete(tag, Restore::TapSawLastEvent)
            });
        }
    }
    // A listen-only tap's return value is ignored.
    event.as_ptr()
}

fn event_mask(events: &[CFRetained<CGEvent>]) -> CGEventMask {
    events
        .iter()
        .map(|e| CGEvent::r#type(Some(e)).0)
        .filter(|&t| t < 64)
        .fold(0, |mask, t| mask | (1u64 << t))
}

/// Tags and posts `events` to `location`, `step` apart (all at once when
/// `step` is zero), with the cursor hidden. `on_done` runs on the main
/// thread after the cursor is restored. With `release_command`, a Command
/// key-up follows the last event on every path. Returns `InputError::Busy`
/// without posting anything while another batch is in flight.
pub fn post_batch(
    mtm: MainThreadMarker,
    events: Vec<CFRetained<CGEvent>>,
    location: CGEventTapLocation,
    step: Duration,
    release_command: bool,
    on_done: DoneHandler,
) -> Result<(), InputError> {
    if events.is_empty() {
        return Err(InputError::NoEvent);
    }
    if is_posting() {
        return Err(InputError::Busy);
    }
    let count = events.len();

    let tags = TAGS.with(|tags| tags.borrow_mut().batch(count));
    for (event, &tag) in events.iter().zip(&tags) {
        CGEvent::set_integer_value_field(
            Some(event),
            CGEventField(FIELD_EVENT_SOURCE_USER_DATA),
            tag,
        );
    }
    let last = *tags.last().ok_or(InputError::NoEvent)?;
    let tap = Tap::install(event_mask(&events));
    if tap.is_none() {
        eprintln!("ice-rust: no event tap; the cursor returns after the fallback delay");
    }
    let batch = Batch {
        gate: RestoreGate::new(last),
        queued: events.into(),
        location,
        last_post: Instant::now(),
        pacer: None,
        deadline: None,
        fallback: None,
        tap,
        cursor: Some(CursorHold::hide()),
        on_done: Some(on_done),
        release_command,
    };
    let unstored = ACTIVE.with(|active| match active.try_borrow_mut() {
        Ok(mut active) => {
            *active = Some(batch);
            None
        }
        Err(_) => Some(batch),
    });
    if let Some(batch) = unstored {
        // Only reachable while something up the stack borrows the active
        // slot: post everything at once and restore right away.
        finish(batch, Restore::Fallback);
        return Ok(());
    }

    if step.is_zero() {
        while !post_next(mtm) {}
        return Ok(());
    }
    if !post_next(mtm) {
        let pacer = timer::schedule_common(mtm, step, true, move |timer| {
            let done = if stalled(last) {
                flush(mtm, last);
                true
            } else {
                post_next(mtm)
            };
            if done {
                timer.invalidate();
            }
        });
        // Bounds the whole batch even when the pacer falls behind.
        let overdue = step * u32::try_from(count).unwrap_or(u32::MAX) + CURSOR_RESTORE_FALLBACK;
        let deadline = timer::after_common(mtm, overdue, move || flush(mtm, last));
        ACTIVE.with(|active| {
            if let Ok(mut active) = active.try_borrow_mut()
                && let Some(batch) = active.as_mut()
                && batch.gate.last_tag() == last
                && !batch.queued.is_empty()
            {
                batch.pacer = Some(pacer.clone());
                batch.deadline = Some(deadline.clone());
            } else {
                pacer.invalidate();
                deadline.invalidate();
            }
        });
    }
    Ok(())
}

/// Finishes the batch in flight, if any: posts its remaining events at
/// once, which always end with the mouse-up, removes the tap and restores
/// the cursor. Used when the app quits.
pub fn cancel_active() {
    let active = ACTIVE.with(|active| active.try_borrow_mut().ok().and_then(|mut a| a.take()));
    if let Some(batch) = active {
        finish(batch, Restore::Cancelled);
    }
}

/// Whether a batch is in flight.
pub fn is_posting() -> bool {
    ACTIVE.with(|active| active.try_borrow().map_or(true, |a| a.is_some()))
}

fn source() -> Result<CFRetained<CGEventSource>, InputError> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok_or(InputError::NoEventSource)
}

fn mouse_event(
    source: &CGEventSource,
    kind: CGEventType,
    at: CGPoint,
    flags: CGEventFlags,
) -> Result<CFRetained<CGEvent>, InputError> {
    let event = CGEvent::new_mouse_event(Some(source), kind, at, CGMouseButton::Left)
        .ok_or(InputError::NoEvent)?;
    CGEvent::set_flags(Some(&event), flags);
    Ok(event)
}

fn click_event(
    source: &CGEventSource,
    kind: CGEventType,
    at: CGPoint,
    window: WindowId,
    click_state: i64,
) -> Result<CFRetained<CGEvent>, InputError> {
    let event = mouse_event(source, kind, at, CGEventFlags(0))?;
    let target = i64::from(window);
    CGEvent::set_integer_value_field(
        Some(&event),
        CGEventField(FIELD_WINDOW_UNDER_POINTER),
        target,
    );
    CGEvent::set_integer_value_field(
        Some(&event),
        CGEventField(FIELD_WINDOW_THAT_CAN_HANDLE),
        target,
    );
    CGEvent::set_integer_value_field(Some(&event), CGEventField(FIELD_CLICK_STATE), click_state);
    Ok(event)
}

/// Clicks the item window at the center of `bounds` (CG space): one
/// left-mouse-down and two left-mouse-ups from an HID-state source, posted
/// to the session tap while the cursor is hidden (M9). Returns at once; the
/// cursor comes back per G4.
pub fn click_item(
    mtm: MainThreadMarker,
    window: WindowId,
    bounds: &Rect,
) -> Result<(), InputError> {
    let at = CGPoint::new(bounds.mid_x(), bounds.mid_y());
    let source = source()?;
    // Two separate up events, so the last tag is the second up.
    let events = vec![
        click_event(&source, CGEventType::LeftMouseDown, at, window, 1)?,
        click_event(&source, CGEventType::LeftMouseUp, at, window, 0)?,
        click_event(&source, CGEventType::LeftMouseUp, at, window, 0)?,
    ];
    post_batch(
        mtm,
        events,
        CGEventTapLocation::SessionEventTap,
        Duration::ZERO,
        false,
        Box::new(|_| {}),
    )
}

/// Command-drags from `from` along `path` (CG space) at the HID tap: a
/// mouse move to `from`, a Command left-mouse-down there, one Command
/// dragged event per path point, and a left-mouse-up without flags at the
/// last point, followed by a Command key-up (L4). Every event is built
/// before anything is posted, so a down is never posted without its up.
pub fn command_drag(
    mtm: MainThreadMarker,
    from: (f64, f64),
    path: &[(f64, f64)],
    step: Duration,
    on_done: DoneHandler,
) -> Result<(), InputError> {
    let source = source()?;
    let command = CGEventFlags::MaskCommand;
    let start = CGPoint::new(from.0, from.1);
    let end = path.last().copied().unwrap_or(from);
    let mut events = vec![
        mouse_event(&source, CGEventType::MouseMoved, start, CGEventFlags(0))?,
        mouse_event(&source, CGEventType::LeftMouseDown, start, command)?,
    ];
    for &(x, y) in path {
        events.push(mouse_event(
            &source,
            CGEventType::LeftMouseDragged,
            CGPoint::new(x, y),
            command,
        )?);
    }
    events.push(mouse_event(
        &source,
        CGEventType::LeftMouseUp,
        CGPoint::new(end.0, end.1),
        CGEventFlags(0),
    )?);
    post_batch(
        mtm,
        events,
        CGEventTapLocation::HIDEventTap,
        step,
        true,
        on_done,
    )
}

/// Whether any mouse button is down, from the combined session state.
pub fn any_button_down() -> bool {
    (0..5u32).any(|button| {
        CGEventSource::button_state(
            CGEventSourceStateID::CombinedSessionState,
            CGMouseButton(button),
        )
    })
}

/// Seconds since the last input event of any kind in the session.
pub fn seconds_since_input() -> f64 {
    // kCGAnyInputEventType is ((CGEventType)(~0)) in CGEventTypes.h.
    CGEventSource::seconds_since_last_event_type(
        CGEventSourceStateID::CombinedSessionState,
        CGEventType(u32::MAX),
    )
}
