//! The Android window.
//!
//! There is exactly one of these per activity: Android gives an app a single
//! `ANativeWindow` that fills the activity, so the usual desktop notion of
//! several independent OS windows does not apply. wezterm's own tabs and panes
//! provide the multiplexing instead.
//!
//! The interesting difference from every other backend is that the native
//! window is *not* stable for the lifetime of the app. It is destroyed and
//! recreated on background/foreground transitions, rotation and configuration
//! changes. See `surface_created`/`surface_destroyed` below, and the
//! `release_surface`/`rebuild_surface` pair in `crate::egl`.

use super::keyboard::{self, Translated};
use crate::connection::ConnectionOps;
use crate::os::android::connection::Connection;
use crate::{
    Clipboard, DeadKeyStatus, Dimensions, MouseCursor, Rect, RequestedWindowGeometry,
    ResizeIncrement, ScreenPoint, WindowEvent, WindowEventSender, WindowOps, WindowState,
};
use anyhow::Context as _;
use async_trait::async_trait;
use config::ConfigHandle;
use ndk::native_window::NativeWindow;
use promise::{Future, Promise};
use raw_window_handle::{
    AndroidDisplayHandle, AndroidNdkWindowHandle, DisplayHandle, HandleError, HasDisplayHandle,
    HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};
use std::rc::Rc;
use wezterm_font::FontConfiguration;
use wezterm_input_types::{KeyEvent, KeyboardLedStatus, Modifiers};

pub(crate) struct WindowInner {
    pub(crate) window_id: usize,
    pub(crate) events: WindowEventSender,
    pub(crate) config: ConfigHandle,

    /// The native window we are currently bound to, if any. `None` between a
    /// `TerminateWindow` and the next `InitWindow`.
    pub(crate) native_window: Option<NativeWindow>,
    /// The EGL state. Retained across surface loss: only the EGLSurface is
    /// destroyed, so textures and programs survive and the glyph atlas does
    /// not need repopulating.
    pub(crate) gl_state: Option<Rc<crate::egl::GlState>>,
    pub(crate) gl_context: Option<Rc<glium::backend::Context>>,

    pub(crate) dimensions: Dimensions,
    pub(crate) window_state: WindowState,
    pub(crate) invalidated: bool,
    pub(crate) has_focus: bool,

    /// Promises awaiting the first native window; `enable_opengl` may be
    /// called before Android has given us a surface.
    pub(crate) surface_waiters: Vec<Promise<()>>,

    /// The last IME buffer we saw, used to recover committed text by diffing.
    pub(crate) last_ime_text: String,
    /// A pending dead key from a physical keyboard.
    pub(crate) dead_key: Option<char>,

    pub(crate) touch: super::touch::TouchState,
}

impl WindowInner {
    pub(crate) fn surface_created(&mut self, native_window: NativeWindow) -> anyhow::Result<()> {
        let width = native_window.width().max(0) as usize;
        let height = native_window.height().max(0) as usize;

        let raw = native_window.ptr().as_ptr() as *const std::ffi::c_void;
        self.native_window.replace(native_window);

        match self.gl_state.as_ref() {
            Some(state) => {
                // Reattach the existing context to the new surface. The
                // context, and everything uploaded into it, survives.
                state
                    .rebuild_surface(raw)
                    .context("rebuilding EGL surface for the new ANativeWindow")?;
                log::debug!("rebound EGL context to the recreated ANativeWindow");
            }
            None => {
                // First surface: nothing to rebuild yet. enable_opengl will
                // pick it up once the waiters below are released.
            }
        }

        for mut promise in self.surface_waiters.drain(..) {
            promise.ok(());
        }

        self.window_state -= WindowState::HIDDEN;
        self.resized(width, height);
        self.invalidate();
        Ok(())
    }

    pub(crate) fn surface_destroyed(&mut self) {
        if let Some(state) = self.gl_state.as_ref() {
            state.release_surface();
        }
        self.native_window.take();
        self.window_state |= WindowState::HIDDEN;
        self.invalidated = false;
        log::debug!("ANativeWindow destroyed; EGL context retained");
    }

    pub(crate) fn resized(&mut self, pixel_width: usize, pixel_height: usize) {
        let dpi = Connection::get()
            .map(|conn| conn.default_dpi())
            .unwrap_or(crate::DEFAULT_DPI) as usize;

        let dimensions = Dimensions {
            pixel_width,
            pixel_height,
            dpi,
        };

        if dimensions == self.dimensions {
            return;
        }
        self.dimensions = dimensions;
        self.events.dispatch(WindowEvent::Resized {
            dimensions,
            window_state: self.window_state,
            // Android resizes are discrete, not an interactive drag.
            live_resizing: false,
        });
    }

    pub(crate) fn invalidate(&mut self) {
        self.invalidated = true;
    }

    /// True when there is something to draw and somewhere to draw it.
    pub(crate) fn needs_paint(&self) -> bool {
        if !self.invalidated || self.native_window.is_none() {
            return false;
        }
        // Belt and braces: a paint against a released EGL surface would be
        // silently discarded, so do not even ask for the frame.
        !matches!(self.gl_state.as_ref(), Some(state) if state.is_surfaceless())
    }

    pub(crate) fn dispatch_paint(&mut self) {
        if !self.needs_paint() {
            return;
        }
        self.invalidated = false;
        self.events.dispatch(WindowEvent::NeedRepaint);
    }

    pub(crate) fn focus_changed(&mut self, focused: bool) {
        if self.has_focus == focused {
            return;
        }
        self.has_focus = focused;
        self.events.dispatch(WindowEvent::FocusChanged(focused));
    }

    pub(crate) fn enable_opengl(&mut self) -> anyhow::Result<Rc<glium::backend::Context>> {
        if let Some(ctx) = self.gl_context.as_ref() {
            return Ok(Rc::clone(ctx));
        }

        let native_window = self
            .native_window
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no ANativeWindow is available"))?;

        // EGL_DEFAULT_DISPLAY is the only display on Android.
        let state = crate::egl::GlState::create(
            None,
            native_window.ptr().as_ptr() as *const std::ffi::c_void,
        )
        .context("creating EGL context for the ANativeWindow")?;
        let state = Rc::new(state);

        // Safety: `state` is a freshly created EGL context that stays alive
        // for as long as the glium Context we hand back.
        let context = unsafe {
            glium::backend::Context::new(
                Rc::clone(&state),
                true,
                if cfg!(debug_assertions) {
                    glium::debug::DebugCallbackBehavior::DebugMessageOnError
                } else {
                    glium::debug::DebugCallbackBehavior::Ignore
                },
            )?
        };

        self.gl_state.replace(state);
        self.gl_context.replace(Rc::clone(&context));
        Ok(context)
    }

    /// Handle a key event from a physical keyboard, or from the few soft
    /// keyboard keys that Android models as keycodes.
    pub(crate) fn key_event(
        &mut self,
        code: android_activity::input::Keycode,
        action: android_activity::input::KeyAction,
        meta: android_activity::input::MetaState,
        repeat_count: i32,
        key_map: Option<&android_activity::input::KeyCharacterMap>,
    ) {
        let key_is_down = match keyboard::key_is_down(action) {
            Some(down) => down,
            None => return,
        };
        let modifiers = keyboard::modifiers_from_meta_state(meta);

        let key = match keyboard::translate_key(code, meta, key_map) {
            Translated::Key(key) => key,
            Translated::DeadKey(accent) => {
                if key_is_down {
                    self.set_dead_key(Some(accent));
                }
                return;
            }
            Translated::None => return,
        };

        // If a dead key is pending, try to combine it with this keystroke.
        let key = match (self.dead_key, &key) {
            (Some(accent), wezterm_input_types::KeyCode::Char(base)) if key_is_down => {
                let combined = key_map
                    .and_then(|map| map.get_dead_char(accent, *base).ok().flatten())
                    .map(wezterm_input_types::KeyCode::Char)
                    .unwrap_or(key.clone());
                self.set_dead_key(None);
                combined
            }
            _ => key,
        };

        self.events.dispatch(WindowEvent::KeyEvent(KeyEvent {
            key,
            modifiers,
            leds: leds_from_meta_state(meta),
            repeat_count: repeat_count.max(0) as u16,
            key_is_down,
            raw: None,
        }));
    }

    fn set_dead_key(&mut self, accent: Option<char>) {
        self.dead_key = accent;
        self.events
            .dispatch(WindowEvent::AdviseDeadKeyStatus(match accent {
                Some(c) => DeadKeyStatus::Composing(c.to_string()),
                None => DeadKeyStatus::None,
            }));
    }

    /// Handle a GameTextInput state update.
    ///
    /// Android hands us the entire edit buffer rather than a delta, so the
    /// committed text has to be recovered by diffing against what we last
    /// saw. Text inside the composing region is provisional and must not be
    /// sent to the pty yet; it is reported as dead-key-style composition
    /// status so that the renderer can show it in place.
    pub(crate) fn text_input_state_changed(
        &mut self,
        state: &android_activity::input::TextInputState,
    ) {
        let composing = state
            .compose_region
            .map(|span| {
                let start = span.start.min(span.end).min(state.text.len());
                let end = span.start.max(span.end).min(state.text.len());
                (start, end)
            })
            .filter(|(start, end)| start < end);

        // The committed portion is everything outside the composing region.
        let committed: String = match composing {
            Some((start, end)) => {
                let mut s = String::with_capacity(state.text.len() - (end - start));
                s.push_str(char_boundary_slice(&state.text, 0, start));
                s.push_str(char_boundary_slice(&state.text, end, state.text.len()));
                s
            }
            None => state.text.clone(),
        };

        // Diff against the previous committed buffer. GameTextInput keeps a
        // persistent buffer, so ordinary typing shows up as a newly appended
        // tail. A *shrinking* buffer is how backspace reaches us: while the
        // IME holds an input connection it consumes KEYCODE_DEL itself and
        // calls deleteSurroundingText, so no key event is ever delivered.
        let new_text = if committed.starts_with(&self.last_ime_text) {
            committed[self.last_ime_text.len()..].to_string()
        } else if self.last_ime_text.starts_with(&committed) {
            let deleted = self.last_ime_text[committed.len()..].chars().count();
            self.last_ime_text = committed;
            for _ in 0..deleted {
                self.send_ime_char('\u{8}');
            }
            return self.advise_composing(state, composing);
        } else {
            // The buffer was replaced wholesale (autocorrect, suggestion
            // pick). Send the whole thing rather than trying to be clever.
            committed.clone()
        };
        self.last_ime_text = committed;

        for c in new_text.chars() {
            // With a multi-line editor the enter key inserts a newline rather
            // than firing an editor action, but a terminal expects carriage
            // return from the enter key.
            self.send_ime_char(if c == '\n' { '\r' } else { c });
        }

        self.advise_composing(state, composing);
    }

    /// Deliver one character that arrived through the IME. Such an event
    /// carries no modifier information at all -- that is what the extra-keys
    /// row exists to supply.
    fn send_ime_char(&mut self, c: char) {
        self.events.dispatch(WindowEvent::KeyEvent(KeyEvent {
            key: wezterm_input_types::KeyCode::Char(c),
            modifiers: Modifiers::NONE,
            leds: KeyboardLedStatus::empty(),
            repeat_count: 1,
            key_is_down: true,
            raw: None,
        }));
    }

    /// Report the provisional text inside the composing region so that the
    /// renderer can show it in place, the same way a dead key is shown.
    fn advise_composing(
        &mut self,
        state: &android_activity::input::TextInputState,
        composing: Option<(usize, usize)>,
    ) {
        let status = match composing {
            Some((start, end)) => {
                DeadKeyStatus::Composing(char_boundary_slice(&state.text, start, end).to_string())
            }
            None => DeadKeyStatus::None,
        };
        self.events.dispatch(WindowEvent::AdviseDeadKeyStatus(status));
    }

    /// Reset the IME buffer. Called whenever the edit session is restarted so
    /// that the next diff is taken against an empty buffer rather than stale
    /// text from a previous focus.
    pub(crate) fn reset_ime_state(&mut self) {
        self.last_ime_text.clear();
        if let Ok(app) = super::app::android_app() {
            configure_ime_editor(app);
            app.set_text_input_state(android_activity::input::TextInputState {
                text: String::new(),
                selection: android_activity::input::TextSpan { start: 0, end: 0 },
                compose_region: None,
            });
        }
    }

    pub(crate) fn close(&mut self) {
        self.events.dispatch(WindowEvent::Destroyed);
        if let Some(conn) = Connection::get() {
            conn.forget_window(self.window_id);
        }
    }
}

/// Clamp `start`/`end` to char boundaries so that slicing a UTF-8 buffer with
/// indices that came from Java (which counts UTF-16 code units after
/// GameTextInput's conversion) cannot panic.
fn char_boundary_slice(s: &str, start: usize, end: usize) -> &str {
    let mut start = start.min(s.len());
    let mut end = end.min(s.len());
    while start > 0 && !s.is_char_boundary(start) {
        start -= 1;
    }
    while end < s.len() && !s.is_char_boundary(end) {
        end += 1;
    }
    if start > end {
        return "";
    }
    &s[start..end]
}

/// Tell the IME what kind of editor it is talking to.
///
/// The default GameTextInput editor is single-line with `IME_ACTION_DONE`,
/// which means the enter key fires an editor action and is never seen by the
/// app at all -- fatal for a terminal. Asking for a multi-line editor with no
/// action makes enter insert a newline into the buffer instead, which the
/// diff in `text_input_state_changed` turns back into a keypress.
///
/// Suggestions and auto-correct are turned off for the same reason a password
/// field turns them off: the IME rewriting what was typed is never wanted, and
/// a shell command line is not prose.
fn configure_ime_editor(app: &android_activity::AndroidApp) {
    use android_activity::input::{ImeOptions, InputType, TextInputAction};

    app.set_ime_editor_info(
        InputType::TYPE_CLASS_TEXT
            | InputType::TYPE_TEXT_FLAG_MULTI_LINE
            | InputType::TYPE_TEXT_FLAG_NO_SUGGESTIONS,
        TextInputAction::None,
        // IMG_FLAG_NO_EXTRACT_UI is spelled that way by android-activity; the
        // Android constant is IME_FLAG_NO_EXTRACT_UI.
        ImeOptions::IME_FLAG_NO_FULLSCREEN | ImeOptions::IMG_FLAG_NO_EXTRACT_UI,
    );
}

fn leds_from_meta_state(meta: android_activity::input::MetaState) -> KeyboardLedStatus {
    let mut leds = KeyboardLedStatus::empty();
    if meta.caps_lock_on() {
        leds |= KeyboardLedStatus::CAPS_LOCK;
    }
    if meta.num_lock_on() {
        leds |= KeyboardLedStatus::NUM_LOCK;
    }
    leds
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Window(pub(crate) usize);

impl Window {
    pub async fn new_window<F>(
        class_name: &str,
        name: &str,
        geometry: RequestedWindowGeometry,
        config: Option<&ConfigHandle>,
        font_config: Rc<FontConfiguration>,
        event_handler: F,
    ) -> anyhow::Result<Window>
    where
        F: 'static + FnMut(WindowEvent, &Window),
    {
        Connection::get()
            .ok_or_else(|| anyhow::anyhow!("no Connection"))?
            .new_window(
                class_name,
                name,
                geometry,
                config,
                font_config,
                event_handler,
            )
            .await
    }

    /// Run `f` against the window's inner state, on the main thread, once the
    /// current call stack has unwound.
    ///
    /// The deferral is not optional. `WindowOps` methods are routinely called
    /// from inside an event handler, and that handler is itself running while
    /// `Connection::with_windows` holds a mutable borrow of this very
    /// `RefCell` -- so borrowing inline panics with "already mutably borrowed"
    /// the first time a resize handler asks its window to repaint. Every other
    /// backend routes through the spawn queue for exactly this reason.
    fn with_window_inner<R, F>(&self, f: F) -> Future<R>
    where
        F: FnOnce(&mut WindowInner) -> anyhow::Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let window_id = self.0;
        let mut promise = Promise::new();
        let future = promise.get_future().unwrap();

        promise::spawn::spawn_into_main_thread(async move {
            match Connection::get().and_then(|conn| conn.window_by_id(window_id)) {
                Some(inner) => {
                    let mut inner = inner.borrow_mut();
                    promise.result(f(&mut inner));
                }
                None => {
                    promise.err(anyhow::anyhow!("window {window_id} has been destroyed"));
                }
            }
        })
        .detach();

        future
    }

    /// Read the inner state without deferring.
    ///
    /// Only for the `raw_window_handle` traits, which are synchronous by
    /// signature and so have nowhere to defer to. It takes a shared borrow and
    /// gives up rather than panicking if the window is mid-dispatch.
    fn try_with_inner<R, F>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&WindowInner) -> R,
    {
        let conn = Connection::get()?;
        let inner = conn.window_by_id(self.0)?;
        let inner = inner.try_borrow().ok()?;
        Some(f(&inner))
    }
}

impl HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // Safety: the Android display handle carries no pointer; it is a
        // marker that identifies the platform.
        Ok(unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::Android(AndroidDisplayHandle::new())) })
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let ptr = self
            .try_with_inner(|inner| {
                inner
                    .native_window
                    .as_ref()
                    .map(|w| w.ptr().cast::<std::ffi::c_void>())
            })
            .flatten()
            .ok_or(HandleError::Unavailable)?;

        // Safety: the pointer is a live ANativeWindow owned by WindowInner,
        // which outlives this borrow because the caller holds `self`, and the
        // Connection keeps the WindowInner alive.
        Ok(unsafe {
            WindowHandle::borrow_raw(RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(ptr)))
        })
    }
}

#[async_trait(?Send)]
impl WindowOps for Window {
    async fn enable_opengl(&self) -> anyhow::Result<Rc<glium::backend::Context>> {
        let window_id = self.0;

        // Android may not have given us a surface yet. Wait for the first
        // InitWindow rather than failing; the message loop keeps running
        // while this future is parked.
        let waiter = {
            let conn = Connection::get().ok_or_else(|| anyhow::anyhow!("no Connection"))?;
            let inner = conn
                .window_by_id(window_id)
                .ok_or_else(|| anyhow::anyhow!("invalid window"))?;
            let mut inner = inner.borrow_mut();
            if inner.native_window.is_some() {
                None
            } else {
                let mut promise = Promise::new();
                let future = promise.get_future().unwrap();
                inner.surface_waiters.push(promise);
                Some(future)
            }
        };

        if let Some(future) = waiter {
            log::debug!("enable_opengl: waiting for the first ANativeWindow");
            future.await?;
        }

        promise::spawn::spawn(async move {
            let conn = Connection::get().ok_or_else(|| anyhow::anyhow!("no Connection"))?;
            let inner = conn
                .window_by_id(window_id)
                .ok_or_else(|| anyhow::anyhow!("invalid window"))?;
            let mut inner = inner.borrow_mut();
            inner.enable_opengl()
        })
        .await
    }

    fn finish_frame(&self, frame: glium::Frame) -> anyhow::Result<()> {
        // Presenting to a destroyed surface is not an error worth propagating
        // to the caller; it just means the app was backgrounded mid-frame.
        match frame.finish() {
            Ok(()) => Ok(()),
            Err(glium::SwapBuffersError::AlreadySwapped) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    fn notify<T: std::any::Any + Send + Sync>(&self, t: T)
    where
        Self: Sized,
    {
        let window_id = self.0;
        // Route through the spawn queue so that the notification is delivered
        // on the main thread; the queue's pipe wakes the ALooper.
        promise::spawn::spawn_into_main_thread(async move {
            if let Some(conn) = Connection::get() {
                if let Some(inner) = conn.window_by_id(window_id) {
                    inner
                        .borrow_mut()
                        .events
                        .dispatch(WindowEvent::Notification(Box::new(t)));
                }
            }
        })
        .detach();
    }

    fn show(&self) {
        self.with_window_inner(|inner| {
            inner.invalidate();
            Ok(())
        });
    }

    fn hide(&self) {}

    fn close(&self) {
        self.with_window_inner(|inner| {
            inner.close();
            Ok(())
        });
    }

    /// Android has no cursor to set; a terminal that wants to indicate a
    /// hyperlink does so with underlining, not a pointer shape.
    fn set_cursor(&self, _cursor: Option<MouseCursor>) {}

    fn invalidate(&self) {
        self.with_window_inner(|inner| {
            inner.invalidate();
            Ok(())
        });
    }

    /// The activity title is not visible while the app is in the foreground,
    /// so there is nothing useful to do with it.
    fn set_title(&self, _title: &str) {}

    /// The window always fills the activity; the app cannot resize itself.
    fn set_inner_size(&self, _width: usize, _height: usize) {}

    fn set_text_cursor_position(&self, _cursor: Rect) {}

    /// Android windows are not resizable, but this is the one call that tells
    /// the backend how large a cell is, which is what a drag gesture needs in
    /// order to scroll by a whole number of lines.
    fn set_resize_increments(&self, incr: ResizeIncrement) {
        self.with_window_inner(move |inner| {
            inner.touch.set_cell_height(incr.y as f64);
            Ok(())
        });
    }

    fn set_window_position(&self, _coords: ScreenPoint) {}

    fn get_clipboard(&self, _clipboard: Clipboard) -> Future<String> {
        super::clipboard::get_clipboard()
    }

    fn set_clipboard(&self, _clipboard: Clipboard, text: String) {
        super::clipboard::set_clipboard(text);
    }

    fn config_did_change(&self, config: &ConfigHandle) {
        let config = config.clone();
        self.with_window_inner(move |inner| {
            inner.config = config;
            inner.invalidate();
            Ok(())
        });
    }
}
