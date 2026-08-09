//! The Android connection and event loop.
//!
//! Every other backend owns its loop: `run_message_loop` blocks on a socket or
//! a message queue and calls back into the app. Android inverts that. The OS
//! owns the loop, delivers lifecycle transitions through it, and expects the
//! app to return promptly. `android-activity` bridges the two by running
//! `android_main` on its own native thread with an `ALooper` we can poll, so
//! `run_message_loop` here is a real loop again — but it must service three
//! sources rather than one:
//!
//! 1. The `ALooper`, for lifecycle and input, via `AndroidApp::poll_events`.
//! 2. wezterm's spawn queue, which is how the mux and every async task get
//!    scheduled onto this thread.
//! 3. Painting, which is driven by invalidation rather than by an OS event.
//!
//! The spawn queue signals a pipe rather than the looper, so a small watcher
//! thread translates pipe readability into `ALooper` wakeups. That is cheaper
//! and less invasive than teaching `window/src/spawn.rs` about Android.

use super::app;
use super::touch::TouchOutcome;
use super::window::{Window, WindowInner};
use crate::connection::ConnectionOps;
use crate::screen::{ScreenInfo, Screens};
use crate::spawn::SPAWN_QUEUE;
use crate::{
    Appearance, Dimensions, RequestedWindowGeometry, WindowEvent, WindowEventSender, WindowState,
};
use android_activity::input::{InputEvent, KeyCharacterMap, MotionAction};
use android_activity::{AndroidApp, AndroidAppWaker, MainEvent, PollEvent};
use config::ConfigHandle;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;
use wezterm_font::FontConfiguration;

/// How long to sleep between frames while a fling is coasting or a long press
/// is pending. Both need the loop to wake without an OS event.
const ANIMATION_TICK: Duration = Duration::from_millis(16);

pub struct Connection {
    pub(crate) app: &'static AndroidApp,
    windows: RefCell<HashMap<usize, Rc<RefCell<WindowInner>>>>,
    should_terminate: Cell<bool>,
    next_window_id: Cell<usize>,
    waker: AndroidAppWaker,
    /// Character maps, keyed by input device id. Looking one up costs a JNI
    /// round trip, so they are cached for the life of the connection.
    key_maps: RefCell<HashMap<i32, Option<KeyCharacterMap>>>,
}

impl Connection {
    pub(crate) fn create_new() -> anyhow::Result<Connection> {
        let app = app::android_app()?;
        let waker = app.create_waker();

        // Translate spawn-queue activity into ALooper wakeups. Without this
        // the loop would sleep through work scheduled from the mux threads.
        spawn_queue_watcher(waker.clone());

        Ok(Connection {
            app,
            windows: RefCell::new(HashMap::new()),
            should_terminate: Cell::new(false),
            next_window_id: Cell::new(1),
            waker,
            key_maps: RefCell::new(HashMap::new()),
        })
    }

    pub(crate) fn window_by_id(&self, id: usize) -> Option<Rc<RefCell<WindowInner>>> {
        self.windows.borrow().get(&id).map(Rc::clone)
    }

    pub(crate) fn forget_window(&self, id: usize) {
        self.windows.borrow_mut().remove(&id);
        if self.windows.borrow().is_empty() {
            self.terminate_message_loop();
        }
    }

    /// Wake the looper from any thread.
    #[allow(dead_code)]
    pub(crate) fn wake(&self) {
        self.waker.wake();
    }

    pub async fn new_window<F>(
        &self,
        _class_name: &str,
        _name: &str,
        _geometry: RequestedWindowGeometry,
        config: Option<&ConfigHandle>,
        _font_config: Rc<FontConfiguration>,
        event_handler: F,
    ) -> anyhow::Result<Window>
    where
        F: 'static + FnMut(WindowEvent, &Window),
    {
        // The requested geometry is ignored: the activity dictates the size,
        // and there is nowhere for a smaller window to live.
        let window_id = self.next_window_id.get();
        self.next_window_id.set(window_id + 1);

        let config = match config {
            Some(config) => config.clone(),
            None => config::configuration(),
        };

        let native_window = self.app.native_window();
        let (pixel_width, pixel_height) = match native_window.as_ref() {
            Some(w) => (w.width().max(0) as usize, w.height().max(0) as usize),
            None => (0, 0),
        };

        let mut window_state = WindowState::FULL_SCREEN;
        if native_window.is_none() {
            window_state |= WindowState::HIDDEN;
        }

        let density = self.app.config().density().unwrap_or(160) as f64 / 160.0;

        let inner = Rc::new(RefCell::new(WindowInner {
            window_id,
            events: WindowEventSender::new(event_handler),
            config,
            native_window,
            gl_state: None,
            gl_context: None,
            dimensions: Dimensions {
                pixel_width,
                pixel_height,
                dpi: self.default_dpi() as usize,
            },
            window_state,
            invalidated: true,
            has_focus: false,
            surface_waiters: vec![],
            last_ime_text: String::new(),
            dead_key: None,
            touch: super::touch::TouchState::new(density),
        }));

        self.windows.borrow_mut().insert(window_id, Rc::clone(&inner));

        let window_handle = Window(window_id);
        inner
            .borrow_mut()
            .events
            .assign_window(window_handle.clone());

        Ok(window_handle)
    }

    pub(crate) fn advise_of_appearance_change(&self, appearance: Appearance) {
        for inner in self.windows.borrow().values() {
            inner
                .borrow_mut()
                .events
                .dispatch(WindowEvent::AppearanceChanged(appearance));
        }
    }

    fn with_windows<F: FnMut(&mut WindowInner)>(&self, mut f: F) {
        let windows: Vec<_> = self.windows.borrow().values().map(Rc::clone).collect();
        for inner in windows {
            f(&mut inner.borrow_mut());
        }
    }

    fn handle_main_event(&self, event: MainEvent) {
        log::trace!("MainEvent {event:?}");
        match event {
            MainEvent::InitWindow { .. } => match self.app.native_window() {
                Some(native_window) => self.with_windows(|inner| {
                    if let Err(err) = inner.surface_created(native_window.clone()) {
                        log::error!("failed to bind to the new ANativeWindow: {err:#}");
                    }
                }),
                None => log::warn!("InitWindow with no native window"),
            },

            MainEvent::TerminateWindow { .. } => {
                self.with_windows(|inner| inner.surface_destroyed());
            }

            MainEvent::WindowResized { .. } | MainEvent::ContentRectChanged { .. } => {
                if let Some(native_window) = self.app.native_window() {
                    let width = native_window.width().max(0) as usize;
                    let height = native_window.height().max(0) as usize;
                    self.with_windows(|inner| {
                        inner.resized(width, height);
                        inner.invalidate();
                    });
                }
            }

            MainEvent::RedrawNeeded { .. } => {
                self.with_windows(|inner| inner.invalidate());
            }

            MainEvent::ConfigChanged { .. } => {
                let appearance = self.get_appearance();
                self.advise_of_appearance_change(appearance);
                self.with_windows(|inner| inner.invalidate());
            }

            MainEvent::GainedFocus => {
                self.with_windows(|inner| {
                    inner.reset_ime_state();
                    inner.focus_changed(true);
                });
            }

            MainEvent::LostFocus => {
                self.with_windows(|inner| inner.focus_changed(false));
            }

            MainEvent::LowMemory => {
                log::warn!("system is low on memory");
            }

            MainEvent::Destroy => {
                log::info!("activity is being destroyed");
                self.terminate_message_loop();
            }

            MainEvent::InputAvailable => {
                self.drain_input();
            }

            _ => {}
        }
    }

    fn key_character_map(&self, device_id: i32) -> Option<KeyCharacterMap> {
        let mut maps = self.key_maps.borrow_mut();
        maps.entry(device_id)
            .or_insert_with(|| match self.app.device_key_character_map(device_id) {
                Ok(map) => Some(map),
                Err(err) => {
                    log::warn!("no key character map for device {device_id}: {err:#}");
                    None
                }
            })
            .clone()
    }

    fn drain_input(&self) {
        let mut iter = match self.app.input_events_iter() {
            Ok(iter) => iter,
            Err(err) => {
                log::error!("failed to read input events: {err:#}");
                return;
            }
        };

        while iter.next(|event| self.handle_input_event(event)) {}
    }

    fn handle_input_event(&self, event: &InputEvent) -> android_activity::InputStatus {
        let windows: Vec<_> = self.windows.borrow().values().map(Rc::clone).collect();
        let inner = match windows.first() {
            Some(inner) => inner,
            None => return android_activity::InputStatus::Unhandled,
        };

        match event {
            InputEvent::KeyEvent(key) => {
                let key_map = self.key_character_map(key.device_id());
                let mut inner = inner.borrow_mut();
                inner.key_event(
                    key.key_code(),
                    key.action(),
                    key.meta_state(),
                    key.repeat_count(),
                    key_map.as_ref(),
                );
                android_activity::InputStatus::Handled
            }

            InputEvent::TextEvent(state) => {
                inner.borrow_mut().text_input_state_changed(state);
                android_activity::InputStatus::Handled
            }

            InputEvent::MotionEvent(motion) => {
                let mut events = vec![];
                let mut font_step = 0;

                {
                    let mut inner = inner.borrow_mut();
                    let touch = &mut inner.touch;

                    match motion.action() {
                        MotionAction::Down => {
                            let p = motion.pointer_at_index(0);
                            touch.pointer_down(p.x() as f64, p.y() as f64, &mut events);
                        }
                        MotionAction::Move => {
                            if motion.pointer_count() >= 2 {
                                if let TouchOutcome::FontSizeStep(step) =
                                    touch.pinch_update(pinch_distance(motion))
                                {
                                    font_step = step;
                                }
                            } else {
                                let p = motion.pointer_at_index(0);
                                touch.pointer_move(p.x() as f64, p.y() as f64, &mut events);
                            }
                        }
                        MotionAction::Up => {
                            let p = motion.pointer_at_index(0);
                            touch.pointer_up(p.x() as f64, p.y() as f64, &mut events);
                        }
                        MotionAction::PointerDown => {
                            touch.pinch_begin(pinch_distance(motion), &mut events);
                        }
                        MotionAction::PointerUp => {
                            touch.pinch_end();
                        }
                        MotionAction::Cancel => {
                            touch.pointer_cancel(&mut events);
                        }
                        MotionAction::Scroll => {
                            // A physical mouse wheel or a trackpad; forward it
                            // directly rather than through gesture recognition.
                            let p = motion.pointer_at_index(0);
                            let scroll = p.axis_value(android_activity::input::Axis::Vscroll);
                            if scroll != 0.0 {
                                events.push(wheel_event(
                                    p.x() as f64,
                                    p.y() as f64,
                                    scroll.round() as i16,
                                ));
                            }
                        }
                        _ => return android_activity::InputStatus::Unhandled,
                    }
                }

                let mut inner = inner.borrow_mut();
                for event in events {
                    inner.events.dispatch(event);
                }
                if font_step != 0 {
                    use config::keyassignment::KeyAssignment;
                    inner
                        .events
                        .dispatch(WindowEvent::PerformKeyAssignment(if font_step > 0 {
                            KeyAssignment::IncreaseFontSize
                        } else {
                            KeyAssignment::DecreaseFontSize
                        }));
                }
                inner.invalidate();
                android_activity::InputStatus::Handled
            }

            _ => android_activity::InputStatus::Unhandled,
        }
    }

    /// Drive animation that has no corresponding OS event: fling momentum and
    /// the long-press timer. Returns true if the loop must not block.
    fn tick_animations(&self) -> bool {
        let mut busy = false;
        self.with_windows(|inner| {
            let mut events = vec![];
            if inner.touch.is_flinging() {
                busy |= inner.touch.tick_fling(&mut events);
            }
            if inner.touch.long_press_pending() {
                inner.touch.poll_long_press(&mut events);
                busy = true;
            }
            if !events.is_empty() {
                for event in events {
                    inner.events.dispatch(event);
                }
                inner.invalidate();
            }
        });
        busy
    }

    fn needs_paint(&self) -> bool {
        self.windows
            .borrow()
            .values()
            .any(|inner| inner.borrow().needs_paint())
    }

    fn dispatch_paints(&self) {
        self.with_windows(|inner| inner.dispatch_paint());
    }
}

fn pinch_distance(motion: &android_activity::input::MotionEvent) -> f64 {
    if motion.pointer_count() < 2 {
        return 0.;
    }
    let a = motion.pointer_at_index(0);
    let b = motion.pointer_at_index(1);
    let dx = (a.x() - b.x()) as f64;
    let dy = (a.y() - b.y()) as f64;
    (dx * dx + dy * dy).sqrt()
}

fn wheel_event(x: f64, y: f64, clicks: i16) -> WindowEvent {
    use wezterm_input_types::{MouseButtons, MouseEvent, MouseEventKind, Modifiers};
    WindowEvent::MouseEvent(MouseEvent {
        kind: MouseEventKind::VertWheel(clicks),
        coords: crate::Point::new(x as isize, y as isize),
        screen_coords: crate::ScreenPoint::new(x as isize, y as isize),
        mouse_buttons: MouseButtons::NONE,
        modifiers: Modifiers::NONE,
    })
}

/// Watch the spawn queue's wakeup pipe and translate readability into an
/// `ALooper` wakeup.
///
/// The pipe is polled rather than read, so the main loop remains the only
/// consumer. To avoid spinning while the main loop has not yet drained the
/// pipe, the watcher backs off after each wakeup; the main loop is guaranteed
/// to call `SPAWN_QUEUE.run()` on every iteration regardless, so a missed
/// wakeup only ever delays work, it does not drop it.
fn spawn_queue_watcher(waker: AndroidAppWaker) {
    let fd = SPAWN_QUEUE.raw_fd();

    std::thread::Builder::new()
        .name("wezterm-spawn-waker".into())
        .spawn(move || {
            use filedescriptor::{poll, pollfd, POLLIN};

            loop {
                let mut pfd = [pollfd {
                    fd,
                    events: POLLIN,
                    revents: 0,
                }];

                match poll(&mut pfd, None) {
                    Ok(_) => {
                        waker.wake();
                        // Give the main loop a chance to drain before we look
                        // again, otherwise a still-readable pipe would spin
                        // this thread.
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(err) => {
                        log::error!("spawn queue watcher poll failed: {err:#}");
                        return;
                    }
                }
            }
        })
        .expect("failed to spawn the spawn-queue watcher thread");
}

impl ConnectionOps for Connection {
    fn name(&self) -> String {
        "android".to_string()
    }

    fn terminate_message_loop(&self) {
        self.should_terminate.set(true);
        self.waker.wake();
    }

    fn default_dpi(&self) -> f64 {
        // Android reports density in dpi directly; 160 is the baseline at
        // which one dp equals one pixel.
        self.app.config().density().unwrap_or(160) as f64
    }

    fn get_appearance(&self) -> Appearance {
        use ndk::configuration::UiModeNight;
        match self.app.config().ui_mode_night() {
            UiModeNight::Yes => Appearance::Dark,
            _ => Appearance::Light,
        }
    }

    fn screens(&self) -> anyhow::Result<Screens> {
        let (width, height) = match self.app.native_window() {
            Some(w) => (w.width().max(0) as isize, w.height().max(0) as isize),
            None => anyhow::bail!("no ANativeWindow, so no screen geometry"),
        };

        let rect = euclid::rect(0, 0, width, height);
        let dpi = self.default_dpi();
        let info = ScreenInfo {
            name: "android".to_string(),
            rect,
            scale: dpi / 160.0,
            max_fps: None,
            effective_dpi: Some(dpi),
        };

        let mut by_name = HashMap::new();
        by_name.insert(info.name.clone(), info.clone());

        Ok(Screens {
            main: info.clone(),
            active: info,
            by_name,
            virtual_rect: rect,
        })
    }

    fn run_message_loop(&self) -> anyhow::Result<()> {
        while !self.should_terminate.get() {
            // Service one queued task before considering sleep. The queue does
            // not guarantee a wakeup per enqueue, so checking unconditionally
            // is what makes it reliable.
            if SPAWN_QUEUE.run() {
                continue;
            }

            let animating = self.tick_animations();

            let timeout = if animating {
                Some(ANIMATION_TICK)
            } else if self.needs_paint() {
                // There is a frame to draw; poll without blocking so that we
                // reach the paint below immediately.
                Some(Duration::ZERO)
            } else {
                None
            };

            self.app.poll_events(timeout, |event| match event {
                PollEvent::Main(main_event) => self.handle_main_event(main_event),
                PollEvent::Wake | PollEvent::Timeout => {}
                _ => {}
            });

            self.dispatch_paints();
        }

        Ok(())
    }

    fn hide_application(&self) {}

    fn beep(&self) {}
}
