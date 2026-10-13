//! Move engine (v1.1): moves one on-screen menu bar item next to another
//! with a synthesized Command-drag.
//!
//! Independent design. For each request it shows every section the move
//! touches, waits for both windows to be on screen, waits for the user to
//! be idle, posts the drag at the HID tap (cursor restored per G4), and
//! polls the window list until the item abuts its target, retrying with a
//! nudged drop point. One move runs at a time; the rest queue. Afterwards
//! the sections return to their previous state, or to the state the user
//! asked for while the move ran. Every wait is a timer on the main run
//! loop.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_foundation::NSTimer;

use super::events::{self, InputError};
use super::{permissions, timer, windows};
use crate::arrange::{Destination, drag_path, drop_point, is_placed, travel, user_idle};
use crate::model::{
    MOVE_DRAG_STEPS, MOVE_EVENT_INTERVAL, MOVE_IDLE_QUIET, MOVE_IDLE_TIMEOUT, MOVE_POLL,
    MOVE_RETRIES, MOVE_SHOW_TIMEOUT, MOVE_VERIFY_TIMEOUT, Rect, WindowId, WindowRecord,
};
use crate::sections::{MenuBarState, Section};

/// Longest wait for a posted drag to report back before verifying anyway.
const DRAG_REPORT_TIMEOUT: Duration = Duration::from_secs(2);

/// What the move engine needs from the app.
pub trait MoverHost {
    fn menu_bar_state(&self) -> MenuBarState;
    fn set_menu_bar_state(&self, state: MenuBarState);
    /// Bounds (CG space) of the display with the active menu bar.
    fn active_display(&self, windows: &[WindowRecord]) -> Option<Rect>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveRequest {
    pub item: WindowId,
    pub destination: Destination,
    /// Sections of the item and of the target, so both can be shown.
    pub sections: [Section; 2],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveError {
    NoPermission,
    NotFound,
    NotOnScreen,
    UserBusy,
    Input(String),
    NotPlaced,
}

impl fmt::Display for MoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoveError::NoPermission => {
                write!(f, "Accessibility permission is needed to move items")
            }
            MoveError::NotFound => {
                write!(f, "the item or its neighbor is no longer in the menu bar")
            }
            MoveError::NotOnScreen => write!(
                f,
                "the item or its neighbor did not appear on screen within {} s",
                MOVE_SHOW_TIMEOUT.as_secs()
            ),
            MoveError::UserBusy => write!(
                f,
                "user busy: the mouse stayed in use for {} s",
                MOVE_IDLE_TIMEOUT.as_secs()
            ),
            MoveError::Input(message) => write!(f, "{message}"),
            MoveError::NotPlaced => write!(
                f,
                "the item did not move after {} attempts",
                MOVE_RETRIES + 1
            ),
        }
    }
}

pub type MoveCallback = Box<dyn FnOnce(Result<(), MoveError>)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Showing,
    WaitingIdle,
    Dragging,
    Verifying,
}

struct Job {
    request: MoveRequest,
    done: Option<MoveCallback>,
    previous: MenuBarState,
    phase: Phase,
    since: Instant,
    attempt: usize,
}

pub struct Mover {
    mtm: MainThreadMarker,
    me: Weak<Mover>,
    host: Rc<dyn MoverHost>,
    queue: RefCell<VecDeque<(MoveRequest, MoveCallback)>>,
    job: RefCell<Option<Job>>,
    ticker: RefCell<Option<Retained<NSTimer>>>,
}

/// Item and target bounds from one window list, by id.
fn find_pair(windows: &[WindowRecord], item: WindowId, target: WindowId) -> Option<(Rect, Rect)> {
    let find = |id| windows.iter().find(|w| w.id == id).map(|w| w.bounds);
    Some((find(item)?, find(target)?))
}

impl Mover {
    pub fn new(mtm: MainThreadMarker, host: Rc<dyn MoverHost>) -> Rc<Self> {
        Rc::new_cyclic(|me| Self {
            mtm,
            me: me.clone(),
            host,
            queue: RefCell::new(VecDeque::new()),
            job: RefCell::new(None),
            ticker: RefCell::new(None),
        })
    }

    /// Whether a move is running or queued.
    pub fn is_busy(&self) -> bool {
        self.job.borrow().is_some() || !self.queue.borrow().is_empty()
    }

    /// The state the sections return to when the running move ends.
    pub fn restore_state(&self) -> Option<MenuBarState> {
        self.job.borrow().as_ref().map(|job| job.previous)
    }

    /// Makes the running move end in `state` instead of the state it
    /// started from. The move keeps the sections it needs shown until then.
    /// Returns false when no move runs.
    pub fn set_restore_state(&self, state: MenuBarState) -> bool {
        match self.job.borrow_mut().as_mut() {
            Some(job) => {
                job.previous = state;
                true
            }
            None => false,
        }
    }

    /// Drops the running move and the queue without calling back. The
    /// caller finishes any input batch in flight. Used when the app quits.
    pub fn cancel(&self) {
        if let Some(ticker) = self.ticker.borrow_mut().take() {
            ticker.invalidate();
        }
        self.job.borrow_mut().take();
        self.queue.borrow_mut().clear();
    }

    /// Queues a move. `done` runs on the main thread when it ends.
    pub fn request(&self, request: MoveRequest, done: MoveCallback) {
        self.queue.borrow_mut().push_back((request, done));
        if self.job.borrow().is_none() {
            self.start_next();
        }
    }

    fn start_next(&self) {
        // A completion callback may already have started the next move.
        if self.job.borrow().is_some() {
            return;
        }
        let Some((request, done)) = self.queue.borrow_mut().pop_front() else {
            return;
        };
        if !permissions::check().post_events {
            done(Err(MoveError::NoPermission));
            self.start_next();
            return;
        }
        let previous = self.host.menu_bar_state();
        let needed = previous.covering(&request.sections);
        if needed != previous {
            self.host.set_menu_bar_state(needed);
        }
        *self.job.borrow_mut() = Some(Job {
            request,
            done: Some(done),
            previous,
            phase: Phase::Showing,
            since: Instant::now(),
            attempt: 0,
        });
        let me = self.me.clone();
        // Default run loop mode on purpose: no new drag starts while a menu
        // or a control tracks the mouse. A drag already posting runs on
        // common-mode timers in `events`.
        let ticker = timer::schedule(self.mtm, MOVE_POLL, true, move |_| {
            if let Some(mover) = me.upgrade() {
                mover.tick();
            }
        });
        *self.ticker.borrow_mut() = Some(ticker);
    }

    fn set_phase(&self, phase: Phase) {
        if let Some(job) = self.job.borrow_mut().as_mut() {
            job.phase = phase;
            job.since = Instant::now();
        }
    }

    fn finish(&self, result: Result<(), MoveError>) {
        if let Some(ticker) = self.ticker.borrow_mut().take() {
            ticker.invalidate();
        }
        let Some(mut job) = self.job.borrow_mut().take() else {
            return;
        };
        if self.host.menu_bar_state() != job.previous {
            self.host.set_menu_bar_state(job.previous);
        }
        if let Some(done) = job.done.take() {
            done(result);
        }
        self.start_next();
    }

    /// Both windows on screen, centered on the active menu bar display.
    fn both_on_screen(&self, request: &MoveRequest) -> Result<Option<(Rect, Rect)>, MoveError> {
        let on_screen = windows::list_on_screen();
        let Some(display) = self.host.active_display(&on_screen) else {
            return Ok(None);
        };
        let shown = |id: WindowId| {
            on_screen
                .iter()
                .find(|w| w.id == id)
                .filter(|w| display.contains(w.bounds.mid_x(), w.bounds.mid_y()))
                .map(|w| w.bounds)
        };
        match (shown(request.item), shown(request.destination.target())) {
            (Some(item), Some(target)) => Ok(Some((item, target))),
            _ => {
                let all = windows::list_windows();
                if find_pair(&all, request.item, request.destination.target()).is_none() {
                    Err(MoveError::NotFound)
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn tick(&self) {
        let Some((request, phase, since, attempt)) = self
            .job
            .borrow()
            .as_ref()
            .map(|j| (j.request, j.phase, j.since, j.attempt))
        else {
            return;
        };
        let elapsed = since.elapsed();
        match phase {
            Phase::Showing => match self.both_on_screen(&request) {
                Err(err) => self.finish(Err(err)),
                Ok(Some((item, target))) => {
                    if is_placed(request.destination, &item, &target) {
                        self.finish(Ok(()));
                    } else {
                        self.set_phase(Phase::WaitingIdle);
                    }
                }
                Ok(None) if elapsed >= MOVE_SHOW_TIMEOUT => {
                    self.finish(Err(MoveError::NotOnScreen))
                }
                Ok(None) => {}
            },
            Phase::WaitingIdle => {
                if user_idle(
                    events::any_button_down(),
                    events::seconds_since_input(),
                    MOVE_IDLE_QUIET,
                ) && !events::is_posting()
                {
                    self.drag(&request, attempt);
                } else if elapsed >= MOVE_IDLE_TIMEOUT {
                    self.finish(Err(MoveError::UserBusy));
                }
            }
            Phase::Dragging => {
                if elapsed >= DRAG_REPORT_TIMEOUT {
                    self.set_phase(Phase::Verifying);
                }
            }
            Phase::Verifying => {
                let all = windows::list_windows();
                let Some((item, target)) =
                    find_pair(&all, request.item, request.destination.target())
                else {
                    self.finish(Err(MoveError::NotFound));
                    return;
                };
                if is_placed(request.destination, &item, &target) {
                    self.finish(Ok(()));
                } else if elapsed >= MOVE_VERIFY_TIMEOUT {
                    if attempt < MOVE_RETRIES {
                        if let Some(job) = self.job.borrow_mut().as_mut() {
                            job.attempt += 1;
                        }
                        self.set_phase(Phase::Showing);
                    } else {
                        self.finish(Err(MoveError::NotPlaced));
                    }
                }
            }
        }
    }

    /// Posts the Command-drag for `attempt` from fresh window bounds.
    fn drag(&self, request: &MoveRequest, attempt: usize) {
        let on_screen = windows::list_on_screen();
        let Some((item, target)) =
            find_pair(&on_screen, request.item, request.destination.target())
        else {
            self.set_phase(Phase::Showing);
            return;
        };
        let to = drop_point(
            request.destination,
            &target,
            travel(&item, &target),
            attempt,
        );
        let from = (item.mid_x(), item.mid_y());
        let path = drag_path(from, to, MOVE_DRAG_STEPS);
        self.set_phase(Phase::Dragging);
        let me = self.me.clone();
        let posted = events::command_drag(
            self.mtm,
            from,
            &path,
            MOVE_EVENT_INTERVAL,
            Box::new(move |_| {
                if let Some(mover) = me.upgrade() {
                    mover.set_phase(Phase::Verifying);
                }
            }),
        );
        match posted {
            Ok(()) => {}
            // A click is still in flight; wait for idle again.
            Err(InputError::Busy) => self.set_phase(Phase::WaitingIdle),
            Err(err) => self.finish(Err(MoveError::Input(err.to_string()))),
        }
    }
}
