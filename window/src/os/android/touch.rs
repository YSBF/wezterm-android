//! Touch gesture recognition.
//!
//! `mouseevent.rs` in wezterm-gui is a mouse state machine: hover, drag-select,
//! click-to-focus. None of that has a direct touch analogue, so rather than
//! pretending a finger is a mouse we recognise gestures here and decide what
//! each one means before synthesising events.
//!
//! The mapping:
//!
//! | gesture              | meaning                                          |
//! |----------------------|--------------------------------------------------|
//! | tap                  | move the mouse there and click (focus a pane)     |
//! | drag (one finger)    | scroll the viewport, with momentum on release     |
//! | long press           | begin a selection drag at that cell               |
//! | drag after long press| extend the selection                              |
//! | pinch                | increase/decrease font size                       |
//!
//! A finger drag scrolls rather than selects because scrolling is the far more
//! common intent and because selection needs a deliberate gesture to be usable
//! at all without a magnifier. Selection is reachable via long press, which is
//! also the platform convention for "start selecting text".
//!
//! Nothing here is forwarded as terminal mouse reporting: an application that
//! has requested mouse tracking still sees the synthesised press/release/move
//! events produced by tap and long-press, which is the behaviour a user
//! expects when tapping a TUI's buttons, while scroll gestures are consumed by
//! the client so that scrollback still works inside `less`.

use crate::{Point, ScreenPoint, WindowEvent};
use std::time::{Duration, Instant};
use wezterm_input_types::{MouseButtons, MouseEvent, MouseEventKind, MousePress, Modifiers};

/// How long a finger must stay down, without moving more than
/// `TAP_SLOP_PIXELS`, before the gesture becomes a long press.
const LONG_PRESS: Duration = Duration::from_millis(500);

/// How far a finger may drift and still count as stationary. Expressed in
/// device-independent pixels and scaled by the display density at runtime.
const TAP_SLOP_DP: f64 = 8.0;

/// Momentum decays by this factor per frame, and stops below
/// `MIN_FLING_VELOCITY`.
const FLING_FRICTION: f64 = 0.92;
const MIN_FLING_VELOCITY: f64 = 0.35;

/// Pinch must change the distance between fingers by this ratio before it
/// counts as one font size step.
const PINCH_STEP_RATIO: f64 = 1.25;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// No fingers down.
    Idle,
    /// One finger down, still might become a tap, a drag or a long press.
    Undecided,
    /// One finger down and moving; scrolling the viewport.
    Scrolling,
    /// Long press fired; the finger is extending a selection.
    Selecting,
    /// Two or more fingers down; tracking a pinch.
    Pinching,
    /// No fingers down, but the viewport is still coasting.
    Flinging,
}

pub struct TouchState {
    phase: Phase,
    /// Where the active gesture started, in window pixels.
    origin: (f64, f64),
    /// The most recent position of the primary finger.
    current: (f64, f64),
    /// When the primary finger went down.
    down_at: Option<Instant>,
    /// Accumulated vertical travel that has not yet been converted into whole
    /// wheel clicks.
    scroll_residue: f64,
    /// Velocity in pixels per millisecond, for the fling.
    velocity: f64,
    last_move_at: Option<Instant>,
    /// Distance between the two fingers when the pinch began or last stepped.
    pinch_reference: f64,

    /// Pixels per wheel click; one click should scroll one cell.
    cell_height: f64,
    /// Device pixels per dp, for slop thresholds.
    density: f64,
}

/// What the caller should do as a result of feeding in a motion event.
pub enum TouchOutcome {
    None,
    /// Increase or decrease the font size by one step.
    FontSizeStep(i32),
}

impl TouchState {
    pub fn new(density: f64) -> Self {
        Self {
            phase: Phase::Idle,
            origin: (0., 0.),
            current: (0., 0.),
            down_at: None,
            scroll_residue: 0.,
            velocity: 0.,
            last_move_at: None,
            pinch_reference: 0.,
            cell_height: 20.,
            density: density.max(1.0),
        }
    }

    /// Tell the gesture layer how tall a cell is, so that a drag scrolls by a
    /// believable number of lines. Called whenever the font metrics change.
    pub fn set_cell_height(&mut self, cell_height: f64) {
        if cell_height > 0. {
            self.cell_height = cell_height;
        }
    }

    fn slop(&self) -> f64 {
        TAP_SLOP_DP * self.density
    }

    fn moved_beyond_slop(&self) -> bool {
        let dx = self.current.0 - self.origin.0;
        let dy = self.current.1 - self.origin.1;
        (dx * dx + dy * dy).sqrt() > self.slop()
    }

    fn mouse_event(&self, kind: MouseEventKind, buttons: MouseButtons) -> WindowEvent {
        let (x, y) = self.current;
        WindowEvent::MouseEvent(MouseEvent {
            kind,
            coords: Point::new(x as isize, y as isize),
            screen_coords: ScreenPoint::new(x as isize, y as isize),
            mouse_buttons: buttons,
            modifiers: Modifiers::NONE,
        })
    }

    pub fn pointer_down(&mut self, x: f64, y: f64, events: &mut Vec<WindowEvent>) {
        // A new touch cancels any coasting.
        if self.phase == Phase::Flinging {
            self.velocity = 0.;
        }
        self.phase = Phase::Undecided;
        self.origin = (x, y);
        self.current = (x, y);
        self.down_at = Some(Instant::now());
        self.last_move_at = self.down_at;
        self.scroll_residue = 0.;
        self.velocity = 0.;

        // Move the cursor there immediately so that any hover-sensitive UI
        // (the tab bar, hyperlinks) sees the right position before a click.
        events.push(self.mouse_event(MouseEventKind::Move, MouseButtons::NONE));
    }

    pub fn pointer_move(&mut self, x: f64, y: f64, events: &mut Vec<WindowEvent>) {
        let now = Instant::now();
        let prev = self.current;
        self.current = (x, y);

        match self.phase {
            Phase::Undecided => {
                if self.long_press_elapsed(now) {
                    self.begin_selection(events);
                } else if self.moved_beyond_slop() {
                    self.phase = Phase::Scrolling;
                }
            }
            Phase::Selecting => {
                events.push(self.mouse_event(MouseEventKind::Move, MouseButtons::LEFT));
            }
            Phase::Scrolling => {}
            _ => return,
        }

        if self.phase == Phase::Scrolling {
            let dy = y - prev.1;
            // Dragging the content down scrolls back through history, which
            // is the direction a touch surface implies.
            self.scroll_residue += dy;
            self.emit_wheel_clicks(events);

            if let Some(last) = self.last_move_at {
                let dt = now.duration_since(last).as_secs_f64() * 1000.;
                if dt > 0. {
                    // Smooth the velocity a little so a jittery final sample
                    // does not dominate the fling.
                    let instant = dy / dt;
                    self.velocity = self.velocity * 0.6 + instant * 0.4;
                }
            }
        }

        self.last_move_at = Some(now);
    }

    pub fn pointer_up(&mut self, x: f64, y: f64, events: &mut Vec<WindowEvent>) {
        self.current = (x, y);
        let now = Instant::now();

        match self.phase {
            Phase::Undecided => {
                if self.long_press_elapsed(now) {
                    // Held still, then lifted without moving: treat as a
                    // selection that selects nothing, i.e. just a click.
                    self.emit_click(events);
                } else {
                    self.emit_click(events);
                }
            }
            Phase::Selecting => {
                events.push(self.mouse_event(
                    MouseEventKind::Release(MousePress::Left),
                    MouseButtons::NONE,
                ));
            }
            Phase::Scrolling => {
                if self.velocity.abs() >= MIN_FLING_VELOCITY {
                    self.phase = Phase::Flinging;
                    self.down_at = None;
                    return;
                }
            }
            _ => {}
        }

        self.phase = Phase::Idle;
        self.down_at = None;
        self.velocity = 0.;
    }

    pub fn pointer_cancel(&mut self, events: &mut Vec<WindowEvent>) {
        if self.phase == Phase::Selecting {
            events.push(self.mouse_event(
                MouseEventKind::Release(MousePress::Left),
                MouseButtons::NONE,
            ));
        }
        self.phase = Phase::Idle;
        self.down_at = None;
        self.velocity = 0.;
        self.scroll_residue = 0.;
    }

    /// A second finger arrived; switch to pinch tracking.
    pub fn pinch_begin(&mut self, distance: f64, events: &mut Vec<WindowEvent>) {
        if self.phase == Phase::Selecting {
            events.push(self.mouse_event(
                MouseEventKind::Release(MousePress::Left),
                MouseButtons::NONE,
            ));
        }
        self.phase = Phase::Pinching;
        self.pinch_reference = distance.max(1.);
        self.velocity = 0.;
    }

    pub fn pinch_update(&mut self, distance: f64) -> TouchOutcome {
        if self.phase != Phase::Pinching || self.pinch_reference <= 0. {
            return TouchOutcome::None;
        }
        let ratio = distance.max(1.) / self.pinch_reference;
        if ratio >= PINCH_STEP_RATIO {
            self.pinch_reference = distance;
            TouchOutcome::FontSizeStep(1)
        } else if ratio <= 1. / PINCH_STEP_RATIO {
            self.pinch_reference = distance;
            TouchOutcome::FontSizeStep(-1)
        } else {
            TouchOutcome::None
        }
    }

    pub fn pinch_end(&mut self) {
        if self.phase == Phase::Pinching {
            self.phase = Phase::Idle;
        }
    }

    /// Advance the fling by one frame. Returns true while still coasting, in
    /// which case the caller should keep requesting frames.
    pub fn tick_fling(&mut self, events: &mut Vec<WindowEvent>) -> bool {
        if self.phase != Phase::Flinging {
            return false;
        }
        // Assume a 60Hz frame; the exact figure only affects how far a fling
        // travels, not whether it terminates.
        const FRAME_MS: f64 = 16.0;
        self.scroll_residue += self.velocity * FRAME_MS;
        self.emit_wheel_clicks(events);

        self.velocity *= FLING_FRICTION;
        if self.velocity.abs() < MIN_FLING_VELOCITY {
            self.phase = Phase::Idle;
            self.velocity = 0.;
            self.scroll_residue = 0.;
            return false;
        }
        true
    }

    pub fn is_flinging(&self) -> bool {
        self.phase == Phase::Flinging
    }

    /// True while a long press could still fire, so the caller knows to keep
    /// waking up rather than blocking indefinitely on the looper.
    pub fn long_press_pending(&self) -> bool {
        self.phase == Phase::Undecided
    }

    /// Called from the event loop when it has been idle; converts a held
    /// finger into a long press without waiting for further motion events.
    pub fn poll_long_press(&mut self, events: &mut Vec<WindowEvent>) {
        if self.phase == Phase::Undecided
            && self.long_press_elapsed(Instant::now())
            && !self.moved_beyond_slop()
        {
            self.begin_selection(events);
        }
    }

    fn long_press_elapsed(&self, now: Instant) -> bool {
        match self.down_at {
            Some(at) => now.duration_since(at) >= LONG_PRESS,
            None => false,
        }
    }

    fn begin_selection(&mut self, events: &mut Vec<WindowEvent>) {
        self.phase = Phase::Selecting;
        events.push(self.mouse_event(MouseEventKind::Move, MouseButtons::NONE));
        events.push(self.mouse_event(
            MouseEventKind::Press(MousePress::Left),
            MouseButtons::LEFT,
        ));
    }

    fn emit_click(&mut self, events: &mut Vec<WindowEvent>) {
        events.push(self.mouse_event(MouseEventKind::Move, MouseButtons::NONE));
        events.push(self.mouse_event(
            MouseEventKind::Press(MousePress::Left),
            MouseButtons::LEFT,
        ));
        events.push(self.mouse_event(
            MouseEventKind::Release(MousePress::Left),
            MouseButtons::NONE,
        ));
    }

    fn emit_wheel_clicks(&mut self, events: &mut Vec<WindowEvent>) {
        while self.scroll_residue.abs() >= self.cell_height {
            let direction = if self.scroll_residue > 0. { 1 } else { -1 };
            self.scroll_residue -= direction as f64 * self.cell_height;
            events.push(self.mouse_event(
                MouseEventKind::VertWheel(direction as i16),
                MouseButtons::NONE,
            ));
        }
    }
}
