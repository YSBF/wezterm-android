//! PROBE STUB: minimal Android backend skeleton.
//!
//! This exists purely to answer "what does the rest of the tree demand from a
//! backend?" — every method is `todo!()`. Nothing here draws pixels.

use crate::connection::ConnectionOps;
use crate::screen::Screens;
use crate::{
    Appearance, Clipboard, MouseCursor, Rect, RequestedWindowGeometry, ResizeIncrement,
    ScreenPoint, WindowEvent, WindowOps,
};
use async_trait::async_trait;
use config::ConfigHandle;
use promise::Future;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use std::rc::Rc;
use wezterm_font::FontConfiguration;

pub struct Connection {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Window {}

impl Connection {
    pub(crate) fn create_new() -> anyhow::Result<Connection> {
        todo!("android: bind to ALooper / android-activity event loop")
    }

    pub async fn new_window<F>(
        &self,
        _class_name: &str,
        _name: &str,
        _geometry: RequestedWindowGeometry,
        _config: Option<&ConfigHandle>,
        _font_config: Rc<FontConfiguration>,
        _event_handler: F,
    ) -> anyhow::Result<Window>
    where
        F: 'static + FnMut(WindowEvent, &Window),
    {
        todo!("android: wrap ANativeWindow from the Activity surface")
    }

    pub(crate) fn advise_of_appearance_change(&self, _appearance: Appearance) {}
}

impl ConnectionOps for Connection {
    fn name(&self) -> String {
        "android".to_string()
    }
    fn terminate_message_loop(&self) {
        todo!()
    }
    fn run_message_loop(&self) -> anyhow::Result<()> {
        todo!("android: the OS owns the loop; this inversion is the real problem")
    }
    fn screens(&self) -> anyhow::Result<Screens> {
        todo!()
    }
}

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
            .unwrap()
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
}

impl HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        todo!()
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        todo!("android: RawWindowHandle::AndroidNdk")
    }
}

#[async_trait(?Send)]
impl WindowOps for Window {
    async fn enable_opengl(&self) -> anyhow::Result<Rc<glium::backend::Context>> {
        todo!("android: EGL via the shared window/src/egl.rs")
    }
    fn finish_frame(&self, _frame: glium::Frame) -> anyhow::Result<()> {
        todo!()
    }
    fn notify<T: std::any::Any + Send + Sync>(&self, _t: T)
    where
        Self: Sized,
    {
        todo!("android: post a user event into the ALooper queue")
    }
    fn show(&self) {
        todo!()
    }
    fn hide(&self) {
        todo!()
    }
    fn close(&self) {
        todo!()
    }
    fn set_cursor(&self, _cursor: Option<MouseCursor>) {}
    fn invalidate(&self) {
        todo!()
    }
    fn set_title(&self, _title: &str) {}
    fn set_inner_size(&self, _width: usize, _height: usize) {}
    fn set_text_cursor_position(&self, _cursor: Rect) {}
    fn set_resize_increments(&self, _incr: ResizeIncrement) {}
    fn set_window_position(&self, _coords: ScreenPoint) {}
    fn get_clipboard(&self, _clipboard: Clipboard) -> Future<String> {
        todo!("android: ClipboardManager over JNI")
    }
    fn set_clipboard(&self, _clipboard: Clipboard, _text: String) {
        todo!()
    }
}
