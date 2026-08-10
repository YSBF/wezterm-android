//! The extra-keys row.
//!
//! A terminal needs Ctrl, Alt, Esc, Tab, arrows and function keys. An Android
//! soft keyboard supplies none of them: it delivers IME text commits, which
//! carry no modifier information at all. Without a way to press Ctrl there is
//! no Ctrl-C, and the terminal is unusable for anything beyond reading.
//!
//! So wezterm draws its own row. It is built from the same box model as the
//! fancy tab bar rather than being Android-native UI, which means it renders
//! through the existing glyph cache and needs no new plumbing between Kotlin
//! and Rust.
//!
//! Modifier keys latch rather than repeat: tapping Ctrl arms it, the next key
//! -- whether from this row, a physical keyboard or the soft keyboard's IME --
//! consumes it, and tapping it again disarms it. That is the only way a
//! modifier can work when the key it modifies arrives from an IME, and it
//! matches what every other mobile terminal does.
//!
//! The row is laid out in the window's bottom padding; see
//! `effective_bottom_padding` in `resize.rs`, which is what reserves the space
//! so that the terminal grid does not draw underneath it.

use crate::termwindow::box_model::{
    BoxDimension, ComputedElement, Corners, DisplayType, Element, ElementColors, ElementContent,
    Float, InheritableColor, LayoutContext, SizedPoly, VerticalAlign,
};
use crate::termwindow::render::corners::{
    BOTTOM_LEFT_ROUNDED_CORNER, BOTTOM_RIGHT_ROUNDED_CORNER, TOP_LEFT_ROUNDED_CORNER,
    TOP_RIGHT_ROUNDED_CORNER,
};
use crate::termwindow::{TermWindow, UIItem, UIItemType};
use crate::utilsprites::RenderMetrics;
use config::{ConfigHandle, Dimension, DimensionContext};
use window::color::LinearRgba;
use window::{KeyCode, KeyEvent, KeyboardLedStatus, Modifiers, WindowOps};

/// One key in the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyRowKey {
    /// A latching modifier. Arms on tap, disarms on a second tap, and is
    /// consumed by the next non-modifier key.
    Modifier(KeyRowModifier),
    /// An ordinary key, sent immediately.
    Key(KeyRowPlain),
    /// Show or hide the soft keyboard.
    ToggleSoftKeyboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyRowModifier {
    Ctrl,
    Alt,
    Shift,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyRowPlain {
    Escape,
    Tab,
    Left,
    Down,
    Up,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

impl KeyRowModifier {
    pub fn modifiers(self) -> Modifiers {
        match self {
            Self::Ctrl => Modifiers::CTRL,
            Self::Alt => Modifiers::ALT,
            Self::Shift => Modifiers::SHIFT,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ctrl => "CTRL",
            Self::Alt => "ALT",
            Self::Shift => "SHIFT",
        }
    }
}

impl KeyRowPlain {
    pub fn key_code(self) -> KeyCode {
        match self {
            // wezterm models these as control characters rather than as named
            // keys; see the KeyCode enum.
            Self::Escape => KeyCode::Char('\u{1b}'),
            Self::Tab => KeyCode::Char('\t'),
            Self::Left => KeyCode::LeftArrow,
            Self::Down => KeyCode::DownArrow,
            Self::Up => KeyCode::UpArrow,
            Self::Right => KeyCode::RightArrow,
            Self::Home => KeyCode::Home,
            Self::End => KeyCode::End,
            Self::PageUp => KeyCode::PageUp,
            Self::PageDown => KeyCode::PageDown,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Escape => "ESC",
            Self::Tab => "TAB",
            Self::Left => "\u{2190}",
            Self::Down => "\u{2193}",
            Self::Up => "\u{2191}",
            Self::Right => "\u{2192}",
            Self::Home => "HOME",
            Self::End => "END",
            Self::PageUp => "PGUP",
            Self::PageDown => "PGDN",
        }
    }
}

impl KeyRowKey {
    fn label(self) -> &'static str {
        match self {
            Self::Modifier(m) => m.label(),
            Self::Key(k) => k.label(),
            // A keyboard glyph would need a font that has one; the vendored
            // fonts do not, so use an unambiguous word.
            Self::ToggleSoftKeyboard => "KBD",
        }
    }
}

/// The keys, in order. Kept short enough that every key stays comfortably
/// tappable on a phone in portrait; the navigation cluster earns its place
/// because scrollback and editors are unusable without it.
pub const KEYS: &[KeyRowKey] = &[
    KeyRowKey::Key(KeyRowPlain::Escape),
    KeyRowKey::Modifier(KeyRowModifier::Ctrl),
    KeyRowKey::Modifier(KeyRowModifier::Alt),
    KeyRowKey::Modifier(KeyRowModifier::Shift),
    KeyRowKey::Key(KeyRowPlain::Tab),
    KeyRowKey::Key(KeyRowPlain::Left),
    KeyRowKey::Key(KeyRowPlain::Down),
    KeyRowKey::Key(KeyRowPlain::Up),
    KeyRowKey::Key(KeyRowPlain::Right),
    KeyRowKey::Key(KeyRowPlain::PageUp),
    KeyRowKey::Key(KeyRowPlain::PageDown),
    KeyRowKey::ToggleSoftKeyboard,
];

/// Height of the row, as a multiple of the title font's cell height.
///
/// 2.4 cells lands around 9mm on a typical phone, which is the low end of what
/// is comfortably tappable; the row would otherwise eat too much of a short
/// screen.
const HEIGHT_IN_CELLS: f32 = 2.4;

/// Horizontal padding either side of a key's label, in pixels.
const KEY_PADDING: f32 = 1.;

/// The radius of a key's rounded corners, in cells.
const KEY_CORNER_CELLS: f32 = 0.25;

impl TermWindow {
    /// The height the extra-keys row occupies, or zero when it is disabled.
    pub fn key_row_pixel_height(&self) -> anyhow::Result<f32> {
        Self::key_row_pixel_height_impl(&self.config, &self.fonts, &self.render_metrics)
    }

    pub fn key_row_pixel_height_impl(
        config: &ConfigHandle,
        fontconfig: &wezterm_font::FontConfiguration,
        _render_metrics: &RenderMetrics,
    ) -> anyhow::Result<f32> {
        if !key_row_enabled(config) {
            return Ok(0.);
        }
        let font = fontconfig.title_font()?;
        let metrics = RenderMetrics::with_font_metrics(&font.metrics());
        Ok((metrics.cell_size.height as f32 * HEIGHT_IN_CELLS).ceil())
    }

    pub fn build_key_row(&self) -> anyhow::Result<ComputedElement> {
        let height = self.key_row_pixel_height()?;
        let font = self.fonts.title_font()?;
        let metrics = RenderMetrics::with_font_metrics(&font.metrics());

        let colors = self.key_row_colors();

        // Size each key to its own label rather than giving every key an equal
        // share of the width. An equal share is narrower than "SHIFT" and wider
        // than an arrow, and `min_width` is only a minimum: a key whose label
        // does not fit grows anyway, so the row overflows the screen and the
        // labels run into one another.
        //
        // Whatever is left over becomes an even margin between the keys, which
        // both spans the row across the window and keeps the tap targets
        // visibly apart. The widths depend only on the labels, so the row still
        // does not reflow as latches are armed and a user's muscle memory for
        // key positions holds.
        // The rounded corners add nothing to a key's width: they are drawn
        // inside the box that `min_width` asks for, eating into the label's
        // room rather than extending past it. Counting them here instead
        // reserved space that no key ever occupies, which left the row ending
        // short of the window edge with every key crowded against the left.
        let cell_width = metrics.cell_size.width as f32;
        let occupied_besides_label = 2. * KEY_PADDING;
        let label_widths: Vec<f32> = KEYS
            .iter()
            .map(|key| key.label().chars().count() as f32 * cell_width)
            .collect();

        let natural: f32 = label_widths
            .iter()
            .map(|width| width + occupied_besides_label)
            .sum();
        // On a screen too narrow to hold every label the row overflows rather
        // than shrinking the text, which would be illegible long before it fit.
        let gap = ((self.dimensions.pixel_width as f32 - natural) / KEYS.len() as f32).max(0.);

        let children = KEYS
            .iter()
            .zip(&label_widths)
            .map(|(key, label_width)| self.build_key(*key, &font, *label_width, gap, height))
            .collect::<Vec<_>>();

        let row = Element::new(&font, ElementContent::Children(children))
            .display(DisplayType::Block)
            .min_width(Some(Dimension::Pixels(self.dimensions.pixel_width as f32)))
            .min_height(Some(Dimension::Pixels(height)))
            .vertical_align(VerticalAlign::Middle)
            .colors(colors);

        let border = self.get_os_border();

        let mut computed = self.compute_element(
            &LayoutContext {
                height: DimensionContext {
                    dpi: self.dimensions.dpi as f32,
                    pixel_max: self.dimensions.pixel_height as f32,
                    pixel_cell: metrics.cell_size.height as f32,
                },
                width: DimensionContext {
                    dpi: self.dimensions.dpi as f32,
                    pixel_max: self.dimensions.pixel_width as f32,
                    pixel_cell: metrics.cell_size.width as f32,
                },
                bounds: euclid::rect(
                    border.left.get() as f32,
                    0.,
                    self.dimensions.pixel_width as f32 - (border.left + border.right).get() as f32,
                    height,
                ),
                metrics: &metrics,
                gl_state: self.render_state.as_ref().unwrap(),
                // Above the terminal grid, below any modal overlay.
                zindex: 10,
            },
            &row,
        )?;

        // The row sits at the very bottom, inside the padding reserved for it
        // by effective_bottom_padding.
        computed.translate(euclid::vec2(
            0.,
            self.dimensions.pixel_height as f32 - (height + border.bottom.get() as f32),
        ));

        Ok(computed)
    }

    fn build_key(
        &self,
        key: KeyRowKey,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        width: f32,
        gap: f32,
        height: f32,
    ) -> Element {
        let latched = match key {
            KeyRowKey::Modifier(m) => self.key_row_latched.contains(m.modifiers()),
            _ => false,
        };

        let (bg, text) = if latched {
            (self.key_row_latch_bg(), self.key_row_latch_fg())
        } else {
            (self.key_row_key_bg(), self.key_row_key_fg())
        };

        Element::new(font, ElementContent::Text(key.label().to_string()))
            .item_type(UIItemType::KeyRow(key))
            .min_width(Some(Dimension::Pixels(width)))
            .min_height(Some(Dimension::Pixels(height)))
            .vertical_align(VerticalAlign::Middle)
            .float(Float::None)
            .margin(BoxDimension {
                left: Dimension::Pixels(gap / 2.),
                right: Dimension::Pixels(gap / 2.),
                top: Dimension::Pixels(0.),
                bottom: Dimension::Pixels(0.),
            })
            .padding(BoxDimension {
                left: Dimension::Pixels(KEY_PADDING),
                right: Dimension::Pixels(KEY_PADDING),
                top: Dimension::Pixels(2.),
                bottom: Dimension::Pixels(2.),
            })
            .border_corners(Some(Corners {
                top_left: SizedPoly {
                    width: Dimension::Cells(KEY_CORNER_CELLS),
                    height: Dimension::Cells(KEY_CORNER_CELLS),
                    poly: TOP_LEFT_ROUNDED_CORNER,
                },
                top_right: SizedPoly {
                    width: Dimension::Cells(KEY_CORNER_CELLS),
                    height: Dimension::Cells(KEY_CORNER_CELLS),
                    poly: TOP_RIGHT_ROUNDED_CORNER,
                },
                bottom_left: SizedPoly {
                    width: Dimension::Cells(KEY_CORNER_CELLS),
                    height: Dimension::Cells(KEY_CORNER_CELLS),
                    poly: BOTTOM_LEFT_ROUNDED_CORNER,
                },
                bottom_right: SizedPoly {
                    width: Dimension::Cells(KEY_CORNER_CELLS),
                    height: Dimension::Cells(KEY_CORNER_CELLS),
                    poly: BOTTOM_RIGHT_ROUNDED_CORNER,
                },
            }))
            .colors(ElementColors {
                border: Default::default(),
                bg: InheritableColor::Color(bg),
                text: InheritableColor::Color(text),
            })
            .hover_colors(Some(ElementColors {
                border: Default::default(),
                bg: InheritableColor::Color(self.key_row_latch_bg()),
                text: InheritableColor::Color(self.key_row_latch_fg()),
            }))
    }

    pub fn paint_key_row(&self) -> anyhow::Result<Vec<UIItem>> {
        let computed = match self.key_row.as_ref() {
            Some(computed) => computed,
            None => return Ok(vec![]),
        };
        let ui_items = computed.ui_items();
        let gl_state = self.render_state.as_ref().unwrap();
        self.render_element(computed, gl_state, None)?;
        Ok(ui_items)
    }

    /// Handle a tap on the row. Returns true if the display needs repainting.
    pub fn key_row_clicked(&mut self, key: KeyRowKey, context: &dyn WindowOps) -> bool {
        match key {
            KeyRowKey::Modifier(m) => {
                // Toggle rather than set: a user who armed Ctrl by accident
                // needs a way out that is not "press a key and undo it".
                self.key_row_latched ^= m.modifiers();
                true
            }
            KeyRowKey::Key(k) => {
                let modifiers = self.take_key_row_latch();
                let event = KeyEvent {
                    key: k.key_code(),
                    modifiers,
                    leds: KeyboardLedStatus::empty(),
                    repeat_count: 1,
                    key_is_down: true,
                    raw: None,
                };
                self.key_event_impl(event, context);
                true
            }
            KeyRowKey::ToggleSoftKeyboard => {
                toggle_soft_keyboard();
                false
            }
        }
    }

    /// Consume any armed modifiers, returning them.
    ///
    /// Called for every key press, wherever it came from, which is what makes
    /// a latch work for IME-delivered characters.
    pub fn take_key_row_latch(&mut self) -> Modifiers {
        std::mem::replace(&mut self.key_row_latched, Modifiers::NONE)
    }

    fn key_row_colors(&self) -> ElementColors {
        ElementColors {
            border: Default::default(),
            bg: InheritableColor::Color(
                self.config
                    .window_frame
                    .inactive_titlebar_bg
                    .to_linear()
                    .into(),
            ),
            text: InheritableColor::Color(
                self.config
                    .window_frame
                    .inactive_titlebar_fg
                    .to_linear()
                    .into(),
            ),
        }
    }

    /// The background of an unlatched key.
    ///
    /// Derived from the row's own background rather than taken from a config
    /// colour, because every candidate -- `active_titlebar_bg`, `button_bg` --
    /// defaults to the same `#333333` as the row itself, which would leave the
    /// keys invisible and the row looking like a strip of floating labels.
    /// Nudging towards the text colour keeps the relationship intact whatever
    /// the theme, including a light one.
    fn key_row_key_bg(&self) -> LinearRgba {
        let bg = self.config.window_frame.inactive_titlebar_bg.to_linear();
        let fg = self.key_row_key_fg();
        const TOWARDS_TEXT: f32 = 0.12;
        LinearRgba(
            bg.0 + (fg.0 - bg.0) * TOWARDS_TEXT,
            bg.1 + (fg.1 - bg.1) * TOWARDS_TEXT,
            bg.2 + (fg.2 - bg.2) * TOWARDS_TEXT,
            bg.3,
        )
    }

    fn key_row_key_fg(&self) -> LinearRgba {
        self.config.window_frame.active_titlebar_fg.to_linear()
    }

    fn key_row_latch_bg(&self) -> LinearRgba {
        // Reuse the cursor colour so that "armed" reads the same way as
        // "this is where input is going" elsewhere in the terminal.
        self.config
            .resolved_palette
            .cursor_bg
            .map(|c| c.to_linear())
            .unwrap_or_else(|| LinearRgba::with_components(0.5, 0.5, 0.5, 1.0))
    }

    fn key_row_latch_fg(&self) -> LinearRgba {
        self.config
            .resolved_palette
            .cursor_fg
            .map(|c| c.to_linear())
            .unwrap_or_else(|| LinearRgba::with_components(0., 0., 0., 1.0))
    }
}

/// True when the extra-keys row should be drawn.
///
/// On Android there is no other way to press Ctrl, so it defaults on; the
/// config key exists so that a user with a permanently attached Bluetooth
/// keyboard can reclaim the space.
pub fn key_row_enabled(config: &ConfigHandle) -> bool {
    config.android_extra_keys_row
}

fn toggle_soft_keyboard() {
    #[cfg(target_os = "android")]
    {
        if let Some(app) = ::window::os::android::try_android_app() {
            // Ask what the keyboard is actually doing rather than remembering
            // what this button last asked for: the system raises and dismisses
            // it without going through here, and a latch that only counts
            // presses ends up inverted and looks broken for one tap.
            if ::window::os::android::soft_keyboard_visible() {
                app.hide_soft_input(false);
            } else {
                app.show_soft_input(false);
            }
        }
    }
}
