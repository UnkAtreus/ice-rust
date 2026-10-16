//! Menu Bar Layout on the macOS 27 menu bar (v1.8). The bars come from
//! MenuBarAgent's slots (`layout_pane::read_agent`), and a move is one
//! Command-drag from the item to the drop point next to its target, read
//! back to check the order. Folded items have no place to read or drag, so
//! every section stays shown while the page reads the menu bar, until
//! Settings closes.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::time::Duration;

use super::Controller;
use crate::agent_bar::{AgentLayout, OurFrames, Spacer};
use crate::arrange::{BarModel, drag_path, drop_point, model_matches, travel};
use crate::geometry;
use crate::model::{MOVE_DRAG_STEPS, MOVE_EVENT_INTERVAL, MOVE_RETRIES, Rect};
use crate::platform::events::{self, InputError};
use crate::platform::layout_pane::{self, LayoutSnapshot};
use crate::platform::mover::{MoveCallback, MoveError, MoveRequest};
use crate::platform::{permissions, timer, windows};
use crate::sections::MenuBarState;

/// How long after a change MenuBarAgent has laid the items out again, with
/// room for a timer that fires a little early.
const SETTLE: Duration = Duration::from_millis(500);
/// How often a move checks whether the user let go of the mouse.
const IDLE_POLL: Duration = Duration::from_millis(50);
/// How many idle checks a move makes before it gives up.
const IDLE_TRIES: u32 = 100;

struct AgentMove {
    request: MoveRequest,
    done: Option<MoveCallback>,
    attempt: usize,
    idle_tries: u32,
}

#[derive(Default)]
pub(super) struct AgentArrange {
    /// The state to go back to when Settings closes; set while Menu Bar
    /// Layout keeps every section shown.
    session: Cell<Option<MenuBarState>>,
    queue: RefCell<VecDeque<(MoveRequest, MoveCallback)>>,
    current: RefCell<Option<AgentMove>>,
}

impl AgentArrange {
    pub(super) fn is_moving(&self) -> bool {
        self.current.borrow().is_some()
    }

    pub(super) fn in_session(&self) -> bool {
        self.session.get().is_some()
    }
}

impl Controller {
    /// Our items' frames in CG space.
    fn our_frames(&self) -> OurFrames {
        let frames = self.control_frames();
        OurFrames {
            icon: frames.icon,
            hidden: frames.hidden,
            spacer: self
                .status
                .spacer_frame(Spacer::Hidden)
                .map(|f| geometry::appkit_to_cg(f, windows::primary_height())),
            always_hidden: frames.always_hidden,
            always_spacer: self
                .status
                .spacer_frame(Spacer::AlwaysHidden)
                .map(|f| geometry::appkit_to_cg(f, windows::primary_height())),
        }
    }

    /// The display with our icon and its menu bar height.
    fn agent_display(&self) -> Option<(Rect, f64)> {
        let place = self.menu_bar_place(&windows::list_on_screen(), None)?;
        Some((place.display.bounds, place.menu_bar_height))
    }

    pub(super) fn read_agent_bar(&self) -> Option<AgentLayout> {
        let (display, _) = self.agent_display()?;
        layout_pane::read_agent(&self.our_frames(), &display).ok()
    }

    /// Shows every section for Menu Bar Layout. Returns true when items
    /// were folded, so the read has to wait for them.
    fn begin_layout_session(&self) -> bool {
        if self.agent_arrange.session.get().is_none() {
            self.agent_arrange.session.set(Some(self.menu_state()));
        }
        if self.status.state() == MenuBarState::AllShown {
            return false;
        }
        self.apply_state(MenuBarState::AllShown);
        true
    }

    /// Settings closed: the sections go back to what they were.
    pub(super) fn end_layout_session(&self) {
        if let Some(previous) = self.agent_arrange.session.take()
            && previous != MenuBarState::AllShown
        {
            self.apply_state(previous);
        }
    }

    /// `load_layout` on macOS 27.
    pub(super) fn agent_layout_snapshot(&self) -> LayoutSnapshot {
        if self.begin_layout_session() {
            let me = self.me.clone();
            timer::after(self.mtm, SETTLE, move || {
                if let Some(c) = me.upgrade() {
                    let window = c.settings_window.borrow().clone();
                    if let Some(window) = window
                        && window.is_visible()
                    {
                        window.reload_layout();
                    }
                }
            });
            return LayoutSnapshot::empty("Showing every menu bar item");
        }
        match self.agent_display() {
            Some((display, height)) => {
                layout_pane::load_agent_snapshot(&self.names, &self.our_frames(), &display, height)
            }
            None => LayoutSnapshot::empty("The menu bar with the ice-rust icon was not found"),
        }
    }

    /// `load_layout_order` on macOS 27.
    pub(super) fn agent_layout_order(&self) -> Option<BarModel> {
        let (display, _) = self.agent_display()?;
        layout_pane::load_agent_order(&self.names, &self.our_frames(), &display)
    }

    /// Queues a move from Menu Bar Layout; moves run one at a time.
    pub(super) fn agent_request_move(&self, request: MoveRequest, done: MoveCallback) {
        self.agent_arrange
            .queue
            .borrow_mut()
            .push_back((request, done));
        if !self.agent_arrange.is_moving() {
            self.next_agent_move();
        }
    }

    fn next_agent_move(&self) {
        let next = self.agent_arrange.queue.borrow_mut().pop_front();
        let Some((request, done)) = next else {
            return;
        };
        *self.agent_arrange.current.borrow_mut() = Some(AgentMove {
            request,
            done: Some(done),
            attempt: 0,
            idle_tries: 0,
        });
        if self.status.state() == MenuBarState::AllShown {
            self.agent_drag();
        } else {
            // Show the sections first; folded items cannot be dragged.
            self.apply_state(MenuBarState::AllShown);
            self.after_settle(Controller::agent_drag);
        }
    }

    fn after_settle(&self, then: fn(&Controller)) {
        let me = self.me.clone();
        timer::after(self.mtm, SETTLE, move || {
            if let Some(c) = me.upgrade() {
                then(&c);
            }
        });
    }

    fn finish_agent_move(&self, result: Result<(), MoveError>) {
        let finished = self.agent_arrange.current.borrow_mut().take();
        if let Some(mut job) = finished {
            eprintln!(
                "ice-rust: move {} after {} attempt(s)",
                if result.is_ok() { "done" } else { "failed" },
                job.attempt + 1
            );
            if let Some(done) = job.done.take() {
                done(result);
            }
        }
        self.next_agent_move();
    }

    fn agent_drag(&self) {
        if !permissions::check().can_post_events() {
            self.finish_agent_move(Err(MoveError::NoPermission));
            return;
        }
        // The user's own drag or click comes first.
        if events::any_button_down() {
            let give_up = {
                let mut current = self.agent_arrange.current.borrow_mut();
                let Some(job) = current.as_mut() else {
                    return;
                };
                job.idle_tries += 1;
                job.idle_tries > IDLE_TRIES
            };
            if give_up {
                self.finish_agent_move(Err(MoveError::UserBusy));
            } else {
                let me = self.me.clone();
                timer::after(self.mtm, IDLE_POLL, move || {
                    if let Some(c) = me.upgrade() {
                        c.agent_drag();
                    }
                });
            }
            return;
        }
        let Some((request, attempt)) = self
            .agent_arrange
            .current
            .borrow()
            .as_ref()
            .map(|job| (job.request, job.attempt))
        else {
            return;
        };
        let Some(layout) = self.read_agent_bar() else {
            self.finish_agent_move(Err(MoveError::NotFound));
            return;
        };
        let destination = layout.drag_destination(request.destination);
        let (Some(item), Some(target)) = (
            layout.frame(request.item),
            layout.frame(destination.target()),
        ) else {
            self.finish_agent_move(Err(MoveError::NotFound));
            return;
        };
        let to = drop_point(destination, &target, travel(&item, &target), attempt);
        let from = (item.mid_x(), item.mid_y());
        let me = self.me.clone();
        let posted = events::command_drag(
            self.mtm,
            from,
            &drag_path(from, to, MOVE_DRAG_STEPS),
            MOVE_EVENT_INTERVAL,
            Box::new(move |_| {
                if let Some(c) = me.upgrade() {
                    c.after_settle(Controller::agent_verify);
                }
            }),
        );
        match posted {
            Ok(()) => {}
            // A click is still in flight; try again shortly.
            Err(InputError::Busy) => self.after_settle(Controller::agent_drag),
            Err(err) => self.finish_agent_move(Err(MoveError::Input(err.to_string()))),
        }
    }

    /// Reads the menu bar back; a drop that missed is tried again a little
    /// further in the travel direction.
    fn agent_verify(&self) {
        let Some((request, attempt)) = self
            .agent_arrange
            .current
            .borrow()
            .as_ref()
            .map(|job| (job.request, job.attempt))
        else {
            return;
        };
        let placed = self.read_agent_bar().is_some_and(|layout| {
            model_matches(
                &layout.model(&self.names),
                request.item,
                request.destination,
            )
        });
        if placed {
            self.finish_agent_move(Ok(()));
        } else if attempt + 1 < MOVE_RETRIES {
            if let Some(job) = self.agent_arrange.current.borrow_mut().as_mut() {
                job.attempt += 1;
            }
            self.agent_drag();
        } else {
            self.finish_agent_move(Err(MoveError::NotPlaced));
        }
    }
}
