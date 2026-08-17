//! The extra-keys row.
//!
//! A terminal needs Ctrl, Alt, Esc, Tab and arrows. An Android soft keyboard
//! supplies none of them: it delivers IME text commits, which carry no modifier
//! information at all. Without a way to press Ctrl there is no Ctrl-C, and the
//! terminal is unusable for anything beyond reading.
//!
//! So wezterm draws its own row. It is built from the same box model as the
//! fancy tab bar rather than being Android-native UI, which means it renders
//! through the existing glyph cache and needs no new plumbing between Kotlin
//! and Rust.
//!
//! ## Why the row is split in two
//!
//! At the accessibility targets this row holds itself to, the full set does not
//! fit on a phone:
//!
//! ```text
//! 10 keys x 44 dp = 440 dp   >   392 dp usable on the reference device
//! 10 keys x 48 dp = 480 dp   >   392 dp usable
//! ```
//!
//! The options were to shrink the targets below the 44dp floor, to scroll the
//! whole row, or to pin the keys that must never move and scroll the rest. Only
//! the third keeps both the floor and direct reachability of the arrows, so the
//! row is a pinned cluster plus a panning remainder. The boundary between them
//! is the *measured* right edge of the pinned cluster, never a figure derived
//! from the nominal target: `min_width` is only a floor, so a key whose label
//! needs more grows past it, and arithmetic that assumes otherwise insists the
//! row fits while the last key sits off the screen.
//!
//! The panning region is clipped to its own rectangle, in both rendering and hit
//! testing. Without the clip a key panned to the left of the boundary has
//! nowhere to go and draws over the pinned keys; without the hit-test clip it
//! answers taps in a pinned key's rectangle, which is a `CTRL` that
//! intermittently types `ESC`.
//!
//! ## Modifiers
//!
//! `CTRL`, `ALT` and `SHIFT` cycle off -> armed -> locked -> off, every
//! transition by the same tap. Armed applies to the next key and then releases;
//! locked stays until tapped off. Modifiers that only ever stay on are a footgun
//! on a touch screen -- an accidental `CTRL` silently corrupts everything typed
//! afterwards -- and a one-shot latch alone cannot express "hold Ctrl while I
//! use the arrows".
//!
//! Cycling was chosen over a long press or a double tap for lock because the
//! touch layer has no double-tap recognition, and adding it would delay the
//! dispatch of every tap by the double-tap interval, which is a real latency
//! regression on the most common interaction in the row; and because long press
//! in the row is already spoken for by the gesture registry.
//!
//! The state is one set per window rather than a map per pane, and it is cleared
//! when the active pane changes. There is then no map, so no entry lifetime and
//! no state accumulating against panes that have closed; clearing on a focus
//! change is the safer default under the same mis-tap reasoning; and it matches
//! how an attached physical keyboard behaves.
//!
//! The row is laid out in the window's bottom padding; see
//! `effective_bottom_padding` in `resize.rs`, which is what reserves the space
//! so that the terminal grid does not draw underneath it.

use crate::termwindow::box_model::{
    BoxDimension, ComputedElement, ComputedElementContent, Corners, DisplayType, Element,
    ElementColors, ElementContent, Float, InheritableColor, LayoutContext, SizedPoly,
    VerticalAlign,
};
use crate::termwindow::render::corners::{
    BOTTOM_LEFT_ROUNDED_CORNER, BOTTOM_RIGHT_ROUNDED_CORNER, TOP_LEFT_ROUNDED_CORNER,
    TOP_RIGHT_ROUNDED_CORNER,
};
use crate::termwindow::{TermWindow, UIItem, UIItemType};
use crate::utilsprites::RenderMetrics;
use config::{ConfigHandle, Dimension, DimensionContext};
use window::color::LinearRgba;
use window::{KeyCode, KeyEvent, KeyboardLedStatus, Modifiers, RectF, WindowOps};

/// One key in the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyRowKey {
    /// A cycling modifier: off, armed, locked.
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

    /// True for the keys that earn the larger of the two tap targets: an
    /// accidental arrow or modifier costs more than an accidental `TAB`, and
    /// they are the keys pressed repeatedly.
    fn prefers_larger_target(self) -> bool {
        match self {
            Self::Modifier(_) => true,
            Self::Key(
                KeyRowPlain::Left | KeyRowPlain::Down | KeyRowPlain::Up | KeyRowPlain::Right,
            ) => true,
            Self::Key(_) | Self::ToggleSoftKeyboard => false,
        }
    }
}

/// The keys that are always visible and never pan.
///
/// `CTRL` because there is no other way to type a control character, and the
/// arrows because they are used in bursts and hunting for them behind a pan
/// would be intolerable.
pub const PINNED_KEYS: &[KeyRowKey] = &[
    KeyRowKey::Modifier(KeyRowModifier::Ctrl),
    KeyRowKey::Key(KeyRowPlain::Left),
    KeyRowKey::Key(KeyRowPlain::Up),
    KeyRowKey::Key(KeyRowPlain::Down),
    KeyRowKey::Key(KeyRowPlain::Right),
];

/// The keys that pan sideways when they do not fit.
///
/// `PgUp` and `PgDn` used to be here and are gone: touch scrolling covers their
/// principal use, and they displaced more important controls.
pub const SCROLLING_KEYS: &[KeyRowKey] = &[
    KeyRowKey::Key(KeyRowPlain::Escape),
    KeyRowKey::Modifier(KeyRowModifier::Alt),
    KeyRowKey::Modifier(KeyRowModifier::Shift),
    KeyRowKey::Key(KeyRowPlain::Tab),
    KeyRowKey::ToggleSoftKeyboard,
];

/// The accessibility floor for a tap target, in dp.
const KEY_MIN_DP: f32 = 44.;

/// The preferred tap target for the arrows and modifiers, in dp.
const KEY_PREFERRED_DP: f32 = 48.;

/// The gap between two keys, in dp. Small, because horizontal room is the
/// scarce resource, but non-zero so that the keys read as separate targets.
const KEY_GAP_DP: f32 = 3.;

/// Breathing room above and below the keys, in dp.
const ROW_PADDING_DP: f32 = 3.;

/// The radius of a key's rounded corners, in dp.
const KEY_CORNER_DP: f32 = 6.;

/// Room to lay the panning keys out in, in pixels.
///
/// The box model drops a label that does not fit the width remaining to it, so a
/// key laid out beyond the right edge of the window would render as an empty box
/// and stay empty once panned into view. Laying the panning region out in a
/// space wider than any row could need avoids that; it is then translated and
/// clipped to the real viewport.
const LAYOUT_HEADROOM: f32 = 100_000.;

/// One modifier's position in the off -> armed -> locked cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierState {
    Off,
    /// Applies to the next key, then releases.
    Armed,
    /// Applies until tapped off.
    Locked,
}

/// The row's modifier state: one set per window.
///
/// Three states need two bits per modifier, so this is two masks rather than the
/// single `Modifiers` it replaced. It is still one value on `TermWindow`, which
/// is the part that matters -- see the module comment on ownership.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyRowModifiers {
    armed: Modifiers,
    locked: Modifiers,
}

impl KeyRowModifiers {
    pub fn state(&self, m: KeyRowModifier) -> ModifierState {
        let bit = m.modifiers();
        if self.locked.contains(bit) {
            ModifierState::Locked
        } else if self.armed.contains(bit) {
            ModifierState::Armed
        } else {
            ModifierState::Off
        }
    }

    /// Advance one modifier by a tap.
    pub fn cycle(&mut self, m: KeyRowModifier) {
        let bit = m.modifiers();
        match self.state(m) {
            ModifierState::Off => self.armed |= bit,
            ModifierState::Armed => {
                self.armed -= bit;
                self.locked |= bit;
            }
            ModifierState::Locked => self.locked -= bit,
        }
    }

    /// The modifiers to fold into a key press. Armed modifiers are consumed;
    /// locked ones persist.
    pub fn consume_for_key_press(&mut self) -> Modifiers {
        let armed = std::mem::replace(&mut self.armed, Modifiers::NONE);
        armed | self.locked
    }

    /// True when consuming a key press would change what the row looks like.
    pub fn has_armed(&self) -> bool {
        !self.armed.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.armed.is_empty() && self.locked.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// A laid-out row.
///
/// Three separate computed elements rather than one tree, because the panning
/// keys have to be laid out in more room than the window has (see
/// `LAYOUT_HEADROOM`) and then confined to the viewport, which a single tree
/// cannot express.
pub struct KeyRowLayout {
    /// The strip behind the keys, spanning the row.
    background: ComputedElement,
    /// The keys that never pan.
    pinned: ComputedElement,
    /// The keys that pan, already offset by the scroll position and clipped.
    scrolling: ComputedElement,
    /// The rectangle the panning keys are confined to, in window pixels.
    viewport: RectF,
    /// The measured width of the panning keys, which is what decides whether
    /// there is anything to pan.
    scrolling_width: f32,
    /// The offset this layout was built with, already clamped to what the
    /// measured widths allow. The caller adopts it, so that a window that grew
    /// wider does not leave a stale pan to be worked off before the row
    /// responds to a drag.
    pub scroll: f32,
}

impl TermWindow {
    /// The height the extra-keys row occupies, or zero when it is disabled.
    pub fn key_row_pixel_height(&self) -> anyhow::Result<f32> {
        Self::key_row_pixel_height_impl(&self.config, &self.fonts, self.dimensions.dpi as f64)
    }

    /// The row is tall enough for a preferred-size key plus padding, and never
    /// shorter than the title font needs, so a large `font_size` cannot clip its
    /// own labels.
    ///
    /// This is 48dp against the 34dp the row used to be, which is screen taken
    /// from the terminal. That is the cost of meeting the tap-target floor and
    /// it is accepted deliberately.
    pub fn key_row_pixel_height_impl(
        config: &ConfigHandle,
        fontconfig: &wezterm_font::FontConfiguration,
        dpi: f64,
    ) -> anyhow::Result<f32> {
        if !key_row_enabled(config) {
            return Ok(0.);
        }
        let font = fontconfig.title_font()?;
        let metrics = RenderMetrics::with_font_metrics(&font.metrics());
        let padding = 2. * dp(ROW_PADDING_DP, dpi);
        Ok((dp(KEY_PREFERRED_DP, dpi) + padding)
            .max(metrics.cell_size.height as f32 + padding)
            .ceil())
    }

    /// How far the panning region can travel before its last key reaches the
    /// right edge. Zero until the row has been laid out once, and whenever
    /// everything already fits.
    pub fn key_row_max_scroll(&self) -> f32 {
        match self.key_row.as_ref() {
            Some(row) => (row.scrolling_width - row.viewport.width()).max(0.),
            None => 0.,
        }
    }

    /// Pan the panning region by `delta` pixels, clamped to its extent. Returns
    /// true when it moved and the display needs repainting.
    pub fn scroll_key_row(&mut self, delta: f32) -> bool {
        let max = self.key_row_max_scroll();
        let next = (self.key_row_scroll + delta).clamp(0., max);
        let step = next - self.key_row_scroll;
        if step.abs() < f32::EPSILON {
            return false;
        }
        self.key_row_scroll = next;

        // Slide the keys that are already laid out rather than discarding the
        // layout. A drag delivers motion events faster than the window repaints,
        // so rebuilding here would relayout and reshape every label for each few
        // pixels of travel -- and, because the rebuild only happens at the next
        // paint, the pan would stall: each event would read a max scroll of zero
        // from the absent layout and decline to move.
        if let Some(row) = self.key_row.as_mut() {
            row.scrolling.translate(euclid::vec2(-step, 0.));
            // `translate` carries a clip along with its content, but this one is
            // a fixed window in which the content moves, so put it back.
            row.scrolling.clip = Some(row.viewport);
            row.scroll = next;
        }
        true
    }

    pub fn build_key_row(&self) -> anyhow::Result<KeyRowLayout> {
        let height = self.key_row_pixel_height()?;
        let font = self.fonts.title_font()?;
        let metrics = RenderMetrics::with_font_metrics(&font.metrics());
        let dpi = self.dimensions.dpi as f64;
        let border = self.get_os_border();

        let left = border.left.get() as f32;
        let right = self.dimensions.pixel_width as f32 - border.right.get() as f32;
        // The row sits at the very bottom, inside the padding reserved for it by
        // effective_bottom_padding.
        let top = self.dimensions.pixel_height as f32 - (height + border.bottom.get() as f32);

        let background = self.compute_key_row_element(
            &metrics,
            &Element::new(&font, ElementContent::Children(vec![]))
                .display(DisplayType::Block)
                .min_width(Some(Dimension::Pixels(right - left)))
                .min_height(Some(Dimension::Pixels(height)))
                .colors(self.key_row_colors()),
            euclid::rect(left, 0., right - left, height),
            right,
        )?;

        let pinned = self.compute_key_row_element(
            &metrics,
            &self.build_key_group(PINNED_KEYS, &font, dpi, height),
            euclid::rect(left, 0., right - left, height),
            right,
        )?;

        // The measured right edge of the pinned cluster, gap included, is the
        // boundary. Nothing may derive it from KEY_PREFERRED_DP: a key whose
        // label needs more room than that grows past it.
        let boundary = group_extent(&pinned).map(|extent| extent.1).unwrap_or(left);
        let viewport = euclid::rect(boundary, top, (right - boundary).max(0.), height);

        let mut scrolling = self.compute_key_row_element(
            &metrics,
            &self.build_key_group(SCROLLING_KEYS, &font, dpi, height),
            euclid::rect(boundary, 0., LAYOUT_HEADROOM, height),
            boundary + LAYOUT_HEADROOM,
        )?;
        let scrolling_width = group_extent(&scrolling)
            .map(|(min_x, max_x)| max_x - min_x)
            .unwrap_or(0.);

        let mut background = background;
        let mut pinned = pinned;
        background.translate(euclid::vec2(0., top));
        pinned.translate(euclid::vec2(0., top));
        // The pan is applied to the laid-out geometry rather than to a leading
        // margin, so that each key's hit rectangle travels with the key it
        // belongs to. Shifting a margin instead leaves the first key's tap
        // target behind at its unpanned position.
        let scroll = self
            .key_row_scroll
            .clamp(0., (scrolling_width - viewport.width()).max(0.));
        scrolling.translate(euclid::vec2(-scroll, top));
        // After the translation, so that the clip stays fixed in the window.
        scrolling.set_clip(viewport);

        Ok(KeyRowLayout {
            background,
            pinned,
            scrolling,
            viewport,
            scrolling_width,
            scroll,
        })
    }

    fn compute_key_row_element(
        &self,
        metrics: &RenderMetrics,
        element: &Element,
        bounds: RectF,
        pixel_max: f32,
    ) -> anyhow::Result<ComputedElement> {
        self.compute_element(
            &LayoutContext {
                height: DimensionContext {
                    dpi: self.dimensions.dpi as f32,
                    pixel_max: self.dimensions.pixel_height as f32,
                    pixel_cell: metrics.cell_size.height as f32,
                },
                width: DimensionContext {
                    dpi: self.dimensions.dpi as f32,
                    pixel_max,
                    pixel_cell: metrics.cell_size.width as f32,
                },
                bounds,
                metrics,
                gl_state: self.render_state.as_ref().unwrap(),
                // Above the terminal grid, below any modal overlay.
                zindex: 10,
            },
            element,
        )
    }

    fn build_key_group(
        &self,
        keys: &[KeyRowKey],
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        dpi: f64,
        height: f32,
    ) -> Element {
        let children = keys
            .iter()
            .map(|key| self.build_key(*key, font, dpi, height))
            .collect::<Vec<_>>();

        Element::new(font, ElementContent::Children(children))
            .display(DisplayType::Block)
            .min_height(Some(Dimension::Pixels(height)))
            .vertical_align(VerticalAlign::Middle)
    }

    fn build_key(
        &self,
        key: KeyRowKey,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        dpi: f64,
        height: f32,
    ) -> Element {
        let target = if key.prefers_larger_target() {
            KEY_PREFERRED_DP
        } else {
            KEY_MIN_DP
        };
        let width = dp(target, dpi);
        let gap = dp(KEY_GAP_DP, dpi);
        let corner = dp(KEY_CORNER_DP, dpi);

        let (bg, text) = match key {
            KeyRowKey::Modifier(m) => match self.key_row_modifiers.state(m) {
                ModifierState::Off => (self.key_row_key_bg(), self.key_row_key_fg()),
                ModifierState::Armed => (self.key_row_armed_bg(), self.key_row_armed_fg()),
                ModifierState::Locked => (self.key_row_locked_bg(), self.key_row_locked_fg()),
            },
            _ => (self.key_row_key_bg(), self.key_row_key_fg()),
        };

        Element::new(font, ElementContent::Text(key.label().to_string()))
            .item_type(UIItemType::KeyRow(key))
            .min_width(Some(Dimension::Pixels(width)))
            .min_height(Some(Dimension::Pixels(dp(target, dpi).min(height))))
            .vertical_align(VerticalAlign::Middle)
            .float(Float::None)
            .margin(BoxDimension {
                left: Dimension::Pixels(gap / 2.),
                right: Dimension::Pixels(gap / 2.),
                top: Dimension::Pixels(0.),
                bottom: Dimension::Pixels(0.),
            })
            .padding(BoxDimension {
                left: Dimension::Pixels(1.),
                right: Dimension::Pixels(1.),
                top: Dimension::Pixels(0.),
                bottom: Dimension::Pixels(0.),
            })
            .border_corners(Some(Corners {
                top_left: SizedPoly {
                    width: Dimension::Pixels(corner),
                    height: Dimension::Pixels(corner),
                    poly: TOP_LEFT_ROUNDED_CORNER,
                },
                top_right: SizedPoly {
                    width: Dimension::Pixels(corner),
                    height: Dimension::Pixels(corner),
                    poly: TOP_RIGHT_ROUNDED_CORNER,
                },
                bottom_left: SizedPoly {
                    width: Dimension::Pixels(corner),
                    height: Dimension::Pixels(corner),
                    poly: BOTTOM_LEFT_ROUNDED_CORNER,
                },
                bottom_right: SizedPoly {
                    width: Dimension::Pixels(corner),
                    height: Dimension::Pixels(corner),
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
                bg: InheritableColor::Color(self.key_row_pressed_bg()),
                text: InheritableColor::Color(text),
            }))
    }

    pub fn paint_key_row(&self) -> anyhow::Result<Vec<UIItem>> {
        let row = match self.key_row.as_ref() {
            Some(row) => row,
            None => return Ok(vec![]),
        };
        let gl_state = self.render_state.as_ref().unwrap();

        self.render_element(&row.background, gl_state, None)?;
        self.render_element(&row.scrolling, gl_state, None)?;
        self.render_element(&row.pinned, gl_state, None)?;

        // A hit test takes the *last* match in the list, so the pinned keys go
        // last: they must win any overlap with a key that has panned under them.
        // The panning keys' rectangles are clipped to the viewport as well, so
        // this is belt and braces rather than the only defence -- but it is the
        // cheap half, and the failure it prevents is intermittent and baffling.
        let mut items = row.scrolling.ui_items();
        items.append(&mut row.pinned.ui_items());
        Ok(items)
    }

    /// Handle a tap on the row. Returns true if the row's own appearance changed
    /// and it needs rebuilding.
    pub fn key_row_clicked(&mut self, key: KeyRowKey, context: &dyn WindowOps) -> bool {
        match key {
            KeyRowKey::Modifier(m) => {
                self.key_row_modifiers.cycle(m);
                self.key_row_modifier_pane = self
                    .get_active_pane_or_overlay()
                    .map(|pane| pane.pane_id())
                    .filter(|_| !self.key_row_modifiers.is_empty());
                true
            }
            KeyRowKey::Key(k) => {
                // The modifiers are not folded in here: `key_event_impl` does it
                // for every key press whatever its origin, and going through the
                // same path keeps the row's keys behaving exactly like an IME
                // commit or a physical key.
                let event = KeyEvent {
                    key: k.key_code(),
                    modifiers: Modifiers::NONE,
                    leds: KeyboardLedStatus::empty(),
                    repeat_count: 1,
                    key_is_down: true,
                    raw: None,
                };
                self.key_event_impl(event, context);
                false
            }
            KeyRowKey::ToggleSoftKeyboard => {
                toggle_soft_keyboard();
                false
            }
        }
    }

    /// Fold the row's modifiers into a key press, consuming the armed ones.
    ///
    /// Called for every key press, wherever it came from, which is what makes
    /// this work for characters delivered by an IME with no modifier
    /// information at all.
    pub fn take_key_row_modifiers(&mut self) -> Modifiers {
        let modifiers = self.key_row_modifiers.consume_for_key_press();
        if self.key_row_modifiers.is_empty() {
            self.key_row_modifier_pane = None;
        }
        modifiers
    }

    /// Drop the row's modifier state if the active pane has changed since it was
    /// armed. Returns true when something was cleared.
    ///
    /// This is checked rather than hooked. The active pane changes from a tap,
    /// from split navigation, from a tab switch and from a pane closing, and a
    /// hook at each is a hook that the next one added forgets. Checking at the
    /// two points that matter -- where the state is consumed and where it is
    /// drawn -- cannot be bypassed by a new way of changing panes.
    ///
    /// A pane id is enough to stand for the tab as well: switching tabs
    /// necessarily changes which pane is active.
    pub fn expire_key_row_modifiers(&mut self) -> bool {
        if self.key_row_modifiers.is_empty() {
            return false;
        }
        let current = self.get_active_pane_or_overlay().map(|pane| pane.pane_id());
        if current == self.key_row_modifier_pane {
            return false;
        }
        self.key_row_modifiers.clear();
        self.key_row_modifier_pane = None;
        true
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

    /// The background of a key that is off.
    ///
    /// Derived from the row's own background rather than taken from a config
    /// colour, because every candidate -- `active_titlebar_bg`, `button_bg` --
    /// defaults to the same `#333333` as the row itself, which would leave the
    /// keys invisible and the row looking like a strip of floating labels.
    /// Nudging towards the text colour keeps the relationship intact whatever
    /// the theme, including a light one.
    fn key_row_key_bg(&self) -> LinearRgba {
        self.key_row_bg_towards_text(0.12)
    }

    fn key_row_key_fg(&self) -> LinearRgba {
        self.config.window_frame.active_titlebar_fg.to_linear()
    }

    /// Pressed feedback: the same key, further towards its own text colour.
    ///
    /// Deliberately not the armed colour, which the row used to use: flashing a
    /// key as "armed" while it is merely being pressed makes an ordinary key
    /// look like it latched.
    fn key_row_pressed_bg(&self) -> LinearRgba {
        self.key_row_bg_towards_text(0.28)
    }

    fn key_row_bg_towards_text(&self, amount: f32) -> LinearRgba {
        let bg = self.config.window_frame.inactive_titlebar_bg.to_linear();
        let fg = self.key_row_key_fg();
        LinearRgba(
            bg.0 + (fg.0 - bg.0) * amount,
            bg.1 + (fg.1 - bg.1) * amount,
            bg.2 + (fg.2 - bg.2) * amount,
            bg.3,
        )
    }

    /// Armed: a tint, reusing the cursor colour so that "this applies to the
    /// next key" reads the same way as "this is where input is going" does
    /// elsewhere in the terminal.
    fn key_row_armed_bg(&self) -> LinearRgba {
        self.config
            .resolved_palette
            .cursor_bg
            .map(|c| c.to_linear())
            .unwrap_or_else(|| LinearRgba::with_components(0.5, 0.5, 0.5, 1.0))
    }

    fn key_row_armed_fg(&self) -> LinearRgba {
        self.config
            .resolved_palette
            .cursor_fg
            .map(|c| c.to_linear())
            .unwrap_or_else(|| LinearRgba::with_components(0., 0., 0., 1.0))
    }

    /// Locked: the key inverted.
    ///
    /// Armed and locked have to be distinguishable from each other and from off,
    /// which one "active" colour cannot do. Inversion is unmistakably different
    /// from both without introducing a fourth colour that some theme will make
    /// invisible: it is the row's own two colours, swapped.
    fn key_row_locked_bg(&self) -> LinearRgba {
        self.key_row_key_fg()
    }

    fn key_row_locked_fg(&self) -> LinearRgba {
        self.config.window_frame.inactive_titlebar_bg.to_linear()
    }
}

/// The horizontal span a group of keys actually occupies, measured from the
/// laid-out elements.
fn group_extent(group: &ComputedElement) -> Option<(f32, f32)> {
    match &group.content {
        ComputedElementContent::Children(keys) => {
            let first = keys.first()?;
            let last = keys.last()?;
            Some((first.bounds.min_x(), last.bounds.max_x()))
        }
        _ => None,
    }
}

/// Convert device-independent pixels to device pixels.
///
/// On Android the dpi wezterm sees is the screen's density scaled by 72/160, so
/// that a configured `font_size` in points behaves as `sp` does in every other
/// app; see `default_dpi` in the Android connection. One point in that space is
/// therefore exactly one dp, and the points-to-pixels conversion is the dp
/// conversion.
fn dp(value: f32, dpi: f64) -> f32 {
    value * dpi as f32 / 72.
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn a_modifier_cycles_off_armed_locked_off() {
        let mut mods = KeyRowModifiers::default();
        let ctrl = KeyRowModifier::Ctrl;

        assert_eq!(mods.state(ctrl), ModifierState::Off);
        mods.cycle(ctrl);
        assert_eq!(mods.state(ctrl), ModifierState::Armed);
        mods.cycle(ctrl);
        assert_eq!(mods.state(ctrl), ModifierState::Locked);
        mods.cycle(ctrl);
        assert_eq!(mods.state(ctrl), ModifierState::Off);
        assert!(mods.is_empty());
    }

    #[test]
    fn armed_applies_once_and_locked_applies_until_tapped_off() {
        let mut mods = KeyRowModifiers::default();
        mods.cycle(KeyRowModifier::Ctrl);

        assert!(mods.has_armed());
        assert_eq!(mods.consume_for_key_press(), Modifiers::CTRL);
        // Released by the key it modified.
        assert_eq!(mods.state(KeyRowModifier::Ctrl), ModifierState::Off);
        assert_eq!(mods.consume_for_key_press(), Modifiers::NONE);

        mods.cycle(KeyRowModifier::Ctrl);
        mods.cycle(KeyRowModifier::Ctrl);
        assert_eq!(mods.state(KeyRowModifier::Ctrl), ModifierState::Locked);
        assert!(!mods.has_armed());
        assert_eq!(mods.consume_for_key_press(), Modifiers::CTRL);
        assert_eq!(mods.consume_for_key_press(), Modifiers::CTRL);
        assert_eq!(mods.state(KeyRowModifier::Ctrl), ModifierState::Locked);

        mods.cycle(KeyRowModifier::Ctrl);
        assert_eq!(mods.consume_for_key_press(), Modifiers::NONE);
    }

    #[test]
    fn modifiers_combine_and_are_independent() {
        let mut mods = KeyRowModifiers::default();
        // Ctrl locked, Alt armed.
        mods.cycle(KeyRowModifier::Ctrl);
        mods.cycle(KeyRowModifier::Ctrl);
        mods.cycle(KeyRowModifier::Alt);

        assert_eq!(
            mods.consume_for_key_press(),
            Modifiers::CTRL | Modifiers::ALT
        );
        // Alt released, Ctrl held.
        assert_eq!(mods.state(KeyRowModifier::Alt), ModifierState::Off);
        assert_eq!(mods.state(KeyRowModifier::Ctrl), ModifierState::Locked);
        assert_eq!(mods.consume_for_key_press(), Modifiers::CTRL);
    }

    #[test]
    fn a_locked_modifier_still_reports_nothing_armed() {
        let mut mods = KeyRowModifiers::default();
        mods.cycle(KeyRowModifier::Shift);
        mods.cycle(KeyRowModifier::Shift);
        // has_armed drives whether a key press has to redraw the row. A locked
        // modifier does not change appearance when a key is pressed, so
        // reporting true here would rebuild the row on every keystroke.
        assert!(!mods.has_armed());
        assert!(!mods.is_empty());
    }

    #[test]
    fn clear_drops_both_armed_and_locked() {
        let mut mods = KeyRowModifiers::default();
        mods.cycle(KeyRowModifier::Ctrl);
        mods.cycle(KeyRowModifier::Alt);
        mods.cycle(KeyRowModifier::Alt);
        mods.clear();
        assert!(mods.is_empty());
        assert_eq!(mods.consume_for_key_press(), Modifiers::NONE);
    }

    #[test]
    fn the_pinned_cluster_holds_the_keys_that_must_not_move() {
        // CTRL because there is no other way to type a control character, and
        // the arrows because they are used in bursts.
        assert!(PINNED_KEYS.contains(&KeyRowKey::Modifier(KeyRowModifier::Ctrl)));
        for arrow in [
            KeyRowPlain::Left,
            KeyRowPlain::Down,
            KeyRowPlain::Up,
            KeyRowPlain::Right,
        ] {
            assert!(PINNED_KEYS.contains(&KeyRowKey::Key(arrow)));
        }
        // And no key appears in both groups.
        for key in PINNED_KEYS {
            assert!(!SCROLLING_KEYS.contains(key));
        }
    }

    #[test]
    fn dp_converts_against_androids_scaled_dpi() {
        // The reference device: 440dpi at density 2.75, which wezterm sees as
        // 440 * 72/160 = 198dpi.
        let dpi = 440. * 72. / 160.;
        assert_eq!(dp(1., dpi), 2.75);
        assert_eq!(dp(48., dpi), 132.);
        // A row of ten preferred-size keys does not fit the 1080px screen,
        // which is the whole reason the row is split.
        assert!(10. * dp(KEY_PREFERRED_DP, dpi) > 1080.);
        // The pinned cluster does.
        assert!(
            PINNED_KEYS.len() as f32 * (dp(KEY_PREFERRED_DP, dpi) + dp(KEY_GAP_DP, dpi)) < 1080.
        );
    }
}
