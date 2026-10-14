//! Block-based `NSTimer`s on the main run loop.
//!
//! `schedule` and `after` fire in the default run loop mode only, so they
//! pause while a menu or a control tracks the mouse. The `*_common`
//! variants fire in the common modes, which include tracking; the input
//! code uses them so a posted mouse-down always gets its mouse-up and the
//! cursor always comes back.
//!
//! Timers fire while NSApplication waits for the next event, inside the
//! autorelease pool it drains only after that event arrives. An idle menu
//! bar app can wait a long time, so every callback runs in its own pool
//! (L7: a closed Settings window stayed alive this way).

use std::ptr::NonNull;
use std::time::Duration;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::{Retained, autoreleasepool};
use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes, NSTimer};

fn block<F>(f: F) -> RcBlock<dyn Fn(NonNull<NSTimer>)>
where
    F: Fn(&NSTimer) + 'static,
{
    RcBlock::new(move |timer: NonNull<NSTimer>| {
        // SAFETY: the timer is alive while its own block runs.
        let timer = unsafe { timer.as_ref() };
        autoreleasepool(|_| f(timer));
    })
}

/// Schedules `f` on the main run loop in the default mode. The marker
/// proves the caller is on the main thread, which the timer blocks rely on.
pub fn schedule<F>(
    _mtm: MainThreadMarker,
    interval: Duration,
    repeats: bool,
    f: F,
) -> Retained<NSTimer>
where
    F: Fn(&NSTimer) + 'static,
{
    let block = block(f);
    // SAFETY: the binding asks for a sendable block because the timer fires
    // on the run loop of the thread that schedules it. `_mtm` proves that is
    // the main thread, so the timer fires on the main run loop and the
    // block's non-Send captures and its AppKit calls only run there. The run
    // loop retains the timer, and the timer the block.
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(
            interval.as_secs_f64(),
            repeats,
            &block,
        )
    }
}

/// Runs `f` once after `delay` on the main run loop in the default mode.
pub fn after<F>(mtm: MainThreadMarker, delay: Duration, f: F) -> Retained<NSTimer>
where
    F: Fn() + 'static,
{
    schedule(mtm, delay, false, move |_| f())
}

/// Like `schedule`, but the timer also fires while the main run loop runs
/// a tracking mode (an open menu, a slider drag).
pub fn schedule_common<F>(
    _mtm: MainThreadMarker,
    interval: Duration,
    repeats: bool,
    f: F,
) -> Retained<NSTimer>
where
    F: Fn(&NSTimer) + 'static,
{
    let block = block(f);
    // SAFETY: as in `schedule`: the timer is added to the main run loop
    // below, from the main thread (`_mtm`), so the block only runs there.
    let timer = unsafe {
        NSTimer::timerWithTimeInterval_repeats_block(interval.as_secs_f64(), repeats, &block)
    };
    // SAFETY: NSRunLoopCommonModes is an immutable NSString constant
    // exported by Foundation, and a valid run loop mode for addTimer:forMode:.
    // The run loop retains the timer.
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    timer
}

/// Runs `f` once after `delay`, also while the main run loop tracks.
pub fn after_common<F>(mtm: MainThreadMarker, delay: Duration, f: F) -> Retained<NSTimer>
where
    F: Fn() + 'static,
{
    schedule_common(mtm, delay, false, move |_| f())
}
