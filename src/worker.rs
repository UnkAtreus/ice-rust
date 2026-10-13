//! Runs work on a helper thread and waits for it with a deadline (G5).
//!
//! On macOS 26 a window capture can block forever inside the window server
//! proxy. The caller waits at most the timeout. A timed-out thread is left
//! to finish on its own; it holds no lock the caller needs, only an atomic
//! gate that turns away new work until it ends, so stuck threads do not
//! pile up.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundedError {
    /// The work did not finish in time. Its thread keeps running.
    TimedOut,
    /// An earlier timed-out run on the same gate is still running.
    Busy,
    /// The thread could not start, or the work panicked.
    Failed,
}

impl fmt::Display for BoundedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BoundedError::TimedOut => write!(f, "timed out"),
            BoundedError::Busy => write!(f, "an earlier run that timed out is still running"),
            BoundedError::Failed => write!(f, "worker thread failed"),
        }
    }
}

impl std::error::Error for BoundedError {}

/// Clears the gate when the worker ends, also when the work panics.
struct GateGuard(&'static AtomicBool);

impl Drop for GateGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Runs `work` on a new thread and waits up to `timeout` for its result.
/// Only one run per `gate` is in flight at a time; while one is, further
/// calls return `Busy` at once.
pub fn run_bounded<T, F>(
    gate: &'static AtomicBool,
    timeout: Duration,
    work: F,
) -> Result<T, BoundedError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    if gate.swap(true, Ordering::AcqRel) {
        return Err(BoundedError::Busy);
    }
    let (tx, rx) = mpsc::sync_channel(1);
    let spawned = thread::Builder::new()
        .name("ice-rust-worker".into())
        .spawn(move || {
            let guard = GateGuard(gate);
            let result = work();
            // Open the gate before handing over the result, so a caller that
            // received it can start the next run at once.
            drop(guard);
            // After a timeout the receiver is gone and the result is dropped.
            let _ = tx.send(result);
        });
    if spawned.is_err() {
        gate.store(false, Ordering::Release);
        return Err(BoundedError::Failed);
    }
    match rx.recv_timeout(timeout) {
        Ok(value) => Ok(value),
        Err(RecvTimeoutError::Timeout) => Err(BoundedError::TimedOut),
        Err(RecvTimeoutError::Disconnected) => Err(BoundedError::Failed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const SHORT: Duration = Duration::from_millis(50);

    fn wait_open(gate: &AtomicBool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while gate.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn fast_work_returns_its_value() {
        static GATE: AtomicBool = AtomicBool::new(false);
        assert_eq!(run_bounded(&GATE, SHORT * 20, || 7), Ok(7));
        // The gate is open again for the next run.
        assert_eq!(run_bounded(&GATE, SHORT * 20, || 8), Ok(8));
    }

    #[test]
    fn slow_work_times_out_without_waiting_for_it() {
        static GATE: AtomicBool = AtomicBool::new(false);
        let start = Instant::now();
        let result = run_bounded(&GATE, SHORT, || {
            thread::sleep(Duration::from_millis(400));
            1
        });
        let waited = start.elapsed();
        assert_eq!(result, Err(BoundedError::TimedOut));
        assert!(waited < Duration::from_millis(300), "waited {waited:?}");
    }

    #[test]
    fn stuck_run_turns_away_new_work_until_it_ends() {
        static GATE: AtomicBool = AtomicBool::new(false);
        let slow = run_bounded(&GATE, SHORT, || thread::sleep(Duration::from_millis(300)));
        assert_eq!(slow, Err(BoundedError::TimedOut));
        assert_eq!(run_bounded(&GATE, SHORT, || 2), Err(BoundedError::Busy));
        wait_open(&GATE);
        assert_eq!(run_bounded(&GATE, SHORT * 20, || 3), Ok(3));
    }

    #[test]
    fn panicking_work_fails_and_opens_the_gate() {
        static GATE: AtomicBool = AtomicBool::new(false);
        let result: Result<(), _> = run_bounded(&GATE, SHORT * 20, || panic!("boom"));
        assert_eq!(result, Err(BoundedError::Failed));
        wait_open(&GATE);
        assert_eq!(run_bounded(&GATE, SHORT * 20, || 4), Ok(4));
    }
}
