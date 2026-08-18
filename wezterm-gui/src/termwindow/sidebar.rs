//! The SSH host sidebar.
//!
//! Rust-rendered inside the terminal's own GPU surface, from the same box model
//! as the tab bar and the extra-keys row. It is not a `DrawerLayout` or a Compose
//! drawer wrapped around the `GameActivity` surface: that would introduce surface
//! z-order, gesture dispatch and terminal-size synchronization problems, none of
//! which a Rust-drawn panel has. Text entry is the exception and goes to native
//! dialogs; see `crate::dialog`.
//!
//! ## Overlay and pinned
//!
//! On a phone in portrait the sidebar is an overlay: it covers part of the
//! terminal, and opening or closing it does not resize the grid. That is the
//! common case and it is deliberately the one that avoids the resize path
//! entirely, because `TermWindow::dimensions` is not reliably the size of the
//! surface -- while the client is attached to a remote pane it briefly holds the
//! size the GUI would *like*, and an earlier attempt to place the key row from it
//! computed a top edge of 3828px on a 2235px surface.
//!
//! Pinned mode reduces the terminal's usable width and is therefore the same
//! class of computation, so it waits for an explicit viewport rectangle. The
//! state exists here; nothing produces it yet.
//!
//! ## Scrolling and clipping
//!
//! The host list pans vertically inside a fixed viewport, by the same mechanism
//! as the key row's panning region: the list is laid out in more room than it
//! has, translated, and clipped -- in both rendering and hit testing. Without the
//! hit-test clip a row scrolled out of sight still answers taps where the header
//! is drawn.
//!
//! ## Gestures
//!
//! An open overlay is not only its own rectangle. Taps on the dimmed area outside
//! it must close it rather than focus a pane behind the dimming, and a drag there
//! must not scroll the terminal. Both are published as regions: the drawer, and a
//! full-surface scrim at a lower priority. See `crate::termwindow::gesture`.

use crate::hosts::{ConfiguredDomain, HostProfile, HostRepository, KeyEntry};
use crate::termwindow::box_model::{
    BoxDimension, ComputedElement, Corners, DisplayType, Element, ElementColors, ElementContent,
    Float, InheritableColor, LayoutContext, SizedPoly, VerticalAlign,
};
use crate::termwindow::render::corners::{
    BOTTOM_LEFT_ROUNDED_CORNER, BOTTOM_RIGHT_ROUNDED_CORNER, TOP_LEFT_ROUNDED_CORNER,
    TOP_RIGHT_ROUNDED_CORNER,
};
use crate::termwindow::{dp, TermWindow, TermWindowNotif, UIItem, UIItemType};
use crate::utilsprites::RenderMetrics;
use config::{Dimension, DimensionContext};
use mux::domain::Domain;
use mux::Mux;
use window::color::LinearRgba;
use window::{RectF, WindowOps};

/// Whether the sidebar is shut, overlaid or pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarState {
    Closed,
    /// Drawn over the terminal. Does not resize the grid.
    Overlay,
    /// Reduces the terminal's usable width. Not yet reachable; see the module
    /// comment.
    Pinned,
}

impl SidebarState {
    pub fn is_open(self) -> bool {
        !matches!(self, Self::Closed)
    }
}

/// What a tap on the sidebar means.
///
/// Profile ids rather than indices: the list is rebuilt from the repository
/// whenever it changes, and an index captured in a cached element tree would
/// point at whatever moved into that slot after a delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarItem {
    /// The dimmed area outside an open overlay. Tapping it closes the sidebar.
    Scrim,
    /// Connect to a stored profile.
    Connect(String),
    /// Open the editor for a stored profile.
    Edit(String),
    /// Delete a stored profile.
    Delete(String),
    /// Connect to a domain declared in `wezterm.lua`.
    ConfiguredDomain(String),
    Add,
    /// Import a private key into the keychain.
    AddKey,
    /// Forget a key, and detach it from every host that used it.
    DeleteKey(String),
    Export,
    Reset,
    Close,
    /// Pin the sidebar, reducing the terminal's usable width.
    Pin,
    /// Return a pinned sidebar to being an overlay.
    Unpin,
    /// Something drawn but not tappable. It still needs an item so that a tap
    /// on it is swallowed by the drawer rather than reaching the terminal
    /// behind.
    Inert,
}

/// Overlay width, as a fraction of the surface. The plan's 80%: wide enough for
/// a host name and an address, narrow enough that the terminal behind is visibly
/// still there.
const OVERLAY_WIDTH_FRACTION: f32 = 0.8;

/// The widest the drawer gets, in dp. Beyond this it stops being a drawer.
const MAX_WIDTH_DP: f32 = 360.;

/// Pinned width, in dp. Within the 300--360dp the plan calls for.
const PINNED_WIDTH_DP: f32 = 320.;

/// The narrowest terminal worth leaving beside a pinned panel, in dp.
///
/// 280dp is about 40 columns at a default font size, which is the point below
/// which a shell prompt starts wrapping on its own.
const MIN_TERMINAL_WIDTH_DP: f32 = 280.;

/// A row in the host list, in dp. The same 48dp target as the key row's
/// preferred size: a mis-tap here connects somewhere or deletes something.
const ROW_HEIGHT_DP: f32 = 56.;

/// A row's inner padding, in dp.
const ROW_PADDING_DP: f32 = 8.;

/// The small buttons on a row, in dp. Still at the 44dp accessibility floor.
const ICON_DP: f32 = 44.;

/// Corner radius, in dp.
const CORNER_DP: f32 = 6.;

/// Room to lay the list out in, in pixels.
///
/// The box model drops content that does not fit the space remaining to it, so a
/// row laid out below the bottom of the drawer would come up empty and stay empty
/// once scrolled into view. The list is laid out in a space taller than any list
/// needs, then translated and clipped to the real viewport.
const LAYOUT_HEADROOM: f32 = 100_000.;

/// The sidebar's state, cached layout, and its view of the host list.
pub struct Sidebar {
    pub state: SidebarState,
    /// How far the host list has been scrolled down, in pixels.
    scroll: f32,
    /// The laid-out sidebar. Taken whenever anything it depends on changes --
    /// including the glyph atlas, which if recreated leaves a cached tree
    /// rendering fragments of other glyphs.
    layout: Option<SidebarLayout>,
    /// The stored profiles.
    ///
    /// Loaded on first open rather than at startup: a desktop build never opens
    /// the sidebar and has no business reading the file.
    repo: Option<HostRepository>,
    /// SSH domains from `wezterm.lua`, shown read-only.
    configured: Vec<ConfiguredDomain>,
    /// Something to tell the user: a rejected save, a failed load.
    notice: Option<String>,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            state: SidebarState::Closed,
            scroll: 0.,
            layout: None,
            repo: None,
            configured: vec![],
            notice: None,
        }
    }
}

impl Sidebar {
    pub fn is_open(&self) -> bool {
        self.state.is_open()
    }

    /// Discard the cached element tree.
    pub fn invalidate(&mut self) {
        self.layout.take();
    }

    /// Forget the loaded profiles, so the next open re-reads them.
    pub fn reload(&mut self) {
        self.repo = None;
        self.invalidate();
    }

    pub fn set_notice(&mut self, notice: Option<String>) {
        self.notice = notice;
        self.invalidate();
    }

    pub fn profiles(&self) -> &[HostProfile] {
        self.repo.as_ref().map(|r| r.profiles()).unwrap_or(&[])
    }

    pub fn profile(&self, id: &str) -> Option<&HostProfile> {
        self.repo.as_ref().and_then(|repo| repo.get(id))
    }

    pub fn keys(&self) -> &[KeyEntry] {
        self.repo.as_ref().map(|repo| repo.keys()).unwrap_or(&[])
    }

    /// The repository, loading it if this is the first time it is needed.
    ///
    /// A load failure is reported as a notice and leaves an empty repository
    /// *without* a path, so that nothing can then save over a file it could not
    /// read.
    pub fn repository_mut(&mut self) -> Option<&mut HostRepository> {
        if self.repo.is_none() {
            match HostRepository::load() {
                Ok(repo) => self.repo = Some(repo),
                Err(err) => {
                    log::error!("could not load the host list: {err:#}");
                    self.notice = Some(format!("Could not read the saved hosts: {err:#}"));
                    return None;
                }
            }
        }
        self.repo.as_mut()
    }
}

/// A laid-out sidebar.
///
/// Separate elements rather than one tree, because the list has to be laid out in
/// more room than the drawer has and then confined to its viewport, which a single
/// tree cannot express. Same reasoning as `KeyRowLayout`.
pub struct SidebarLayout {
    /// The dimmed area outside the drawer. Absent in pinned mode, where there is
    /// nothing to dim.
    scrim: Option<ComputedElement>,
    /// The drawer's background, header and footer.
    frame: ComputedElement,
    /// The scrollable host list, already offset and clipped.
    list: ComputedElement,
    /// The rectangle the list is confined to, in window pixels.
    viewport: RectF,
    /// The measured height of the list.
    list_height: f32,
    /// The offset this layout was built with, clamped to what fits.
    scroll: f32,
}

impl TermWindow {
    /// The sidebar's width in pixels, or zero when it is closed.
    pub fn sidebar_pixel_width(&self) -> f32 {
        sidebar_width(
            self.sidebar.state,
            self.dimensions.pixel_width as f32,
            self.dimensions.dpi as f64,
        )
    }

    /// True when pinning would leave a usable terminal beside the panel.
    ///
    /// Pinned mode is for landscape and for tablets. On a phone in portrait the
    /// panel and a terminal cannot both have room, and offering the choice would
    /// mean offering a way to make the terminal unusable.
    pub fn can_pin_sidebar(&self) -> bool {
        let dpi = self.dimensions.dpi as f64;
        // `dimensions` rather than `surface_dimensions`: the two now agree, and
        // this is the space the panel is drawn in. See the `viewport` module for
        // what keeps them in step.
        can_pin(self.dimensions.pixel_width as f32, dpi)
    }

    /// Open, close, or swap between overlay and pinned.
    pub fn set_sidebar_state(&mut self, state: SidebarState) {
        if self.sidebar.state == state {
            return;
        }
        let was_pinned = self.sidebar.state == SidebarState::Pinned;
        self.sidebar.state = state;
        self.sidebar.invalidate();
        if state.is_open() {
            // Re-read on every open. The file is small, and a stale list after
            // an edit made in another Activity of the same process is worse
            // than the read.
            //
            // `reload` only drops the cached copy; the read itself happens on
            // the next request for the repository. Making that request here is
            // what turns "forget" into "re-read": the painting path reaches for
            // `profiles()`, which cannot load anything, so without this an open
            // sidebar shows an empty list whatever the file says -- and a file
            // it could not parse reports nothing either. Clear any previous
            // notice first, so a failure that has since been fixed does not
            // linger.
            self.sidebar.reload();
            self.sidebar.notice = None;
            self.sidebar.repository_mut();
            self.sidebar.configured = crate::hosts::configured_ssh_domains(&self.config);
        } else {
            self.sidebar.notice = None;
        }
        // Opening changes gesture routing for the whole surface, not just for
        // the drawer: see `publish_gesture_regions`.
        self.publish_gesture_regions();

        // Only pinning takes width away from the terminal, so only crossing that
        // boundary needs the grid and the pty resized. An overlay deliberately
        // does not, which is what keeps the common case off the
        // window-geometry path entirely.
        if was_pinned != (state == SidebarState::Pinned) {
            if let Some(window) = self.window.as_ref().cloned() {
                let dimensions = self.dimensions;
                self.apply_dimensions(&dimensions, None, &window);
            }
        }

        if let Some(window) = self.window.as_ref() {
            window.invalidate();
        }
    }

    pub fn toggle_sidebar(&mut self) {
        let next = if self.sidebar.is_open() {
            SidebarState::Closed
        } else {
            SidebarState::Overlay
        };
        self.set_sidebar_state(next);
    }

    /// How far the list can be scrolled. Zero until it has been laid out once.
    pub fn sidebar_max_scroll(&self) -> f32 {
        match self.sidebar.layout.as_ref() {
            Some(layout) => (layout.list_height - layout.viewport.height()).max(0.),
            None => 0.,
        }
    }

    /// Scroll the host list by `delta` pixels. True when it moved.
    pub fn scroll_sidebar(&mut self, delta: f32) -> bool {
        let max = self.sidebar_max_scroll();
        let next = (self.sidebar.scroll + delta).clamp(0., max);
        let step = next - self.sidebar.scroll;
        if step.abs() < f32::EPSILON {
            return false;
        }
        self.sidebar.scroll = next;

        // Slide the laid-out rows rather than rebuilding, for the same reason as
        // the key row: motion events outpace repaints, and a rebuild per event
        // would both reshape every label and stall, because the next event would
        // read a max scroll of zero from the layout the previous one discarded.
        if let Some(layout) = self.sidebar.layout.as_mut() {
            layout.list.translate(euclid::vec2(0., -step));
            // `translate` carries a clip with its content, but this one is a
            // fixed window the content moves inside.
            layout.list.clip = Some(layout.viewport);
            layout.scroll = next;
        }
        true
    }

    /// The rectangle the drawer occupies, in window pixels.
    fn sidebar_rect(&self) -> RectF {
        let border = self.get_os_border();
        let width = self.sidebar_pixel_width();
        let top = if self.show_tab_bar {
            self.tab_bar_pixel_height().unwrap_or(0.) + border.top.get() as f32
        } else {
            border.top.get() as f32
        };
        let bottom = self.dimensions.pixel_height as f32
            - (self.key_row_pixel_height().unwrap_or(0.) + border.bottom.get() as f32);

        euclid::rect(border.left.get() as f32, top, width, (bottom - top).max(0.))
    }

    pub fn build_sidebar(&self) -> anyhow::Result<SidebarLayout> {
        let font = self.fonts.title_font()?;
        let metrics = RenderMetrics::with_font_metrics(&font.metrics());
        let dpi = self.dimensions.dpi as f64;
        let drawer = self.sidebar_rect();

        let scrim = match self.sidebar.state {
            // Only an overlay has anything to dim. In pinned mode the terminal
            // beside it is live and must keep its gestures.
            SidebarState::Overlay => Some(self.build_sidebar_scrim(&font, &metrics)?),
            _ => None,
        };

        let frame = self.compute_sidebar_element(
            &metrics,
            &self.build_sidebar_frame(&font, dpi, &drawer),
            euclid::rect(drawer.min_x(), 0., drawer.width(), drawer.height()),
            drawer.max_x(),
            drawer.height(),
        )?;

        let header_height = self.sidebar_header_height(dpi);
        let footer_height = self.sidebar_footer_height(dpi);
        let viewport = euclid::rect(
            drawer.min_x(),
            drawer.min_y() + header_height,
            drawer.width(),
            (drawer.height() - header_height - footer_height).max(0.),
        );

        let mut list = self.compute_sidebar_element(
            &metrics,
            &self.build_sidebar_list(&font, dpi, drawer.width()),
            euclid::rect(drawer.min_x(), 0., drawer.width(), LAYOUT_HEADROOM),
            drawer.max_x(),
            LAYOUT_HEADROOM,
        )?;
        let list_height = list_extent(&list);

        let mut frame = frame;
        frame.translate(euclid::vec2(0., drawer.min_y()));

        let scroll = self
            .sidebar
            .scroll
            .clamp(0., (list_height - viewport.height()).max(0.));
        list.translate(euclid::vec2(0., viewport.min_y() - scroll));
        // After the translation, so the clip stays fixed in the window.
        list.set_clip(viewport);

        Ok(SidebarLayout {
            scrim,
            frame,
            list,
            viewport,
            list_height,
            scroll,
        })
    }

    /// The dimmed area outside the drawer.
    ///
    /// A full-surface element rather than the three strips left over around the
    /// drawer: the drawer is drawn after it and covers its own rectangle, and one
    /// element is one hit region, which is what "tap anywhere else to close"
    /// wants.
    fn build_sidebar_scrim(
        &self,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        metrics: &RenderMetrics,
    ) -> anyhow::Result<ComputedElement> {
        let width = self.dimensions.pixel_width as f32;
        let height = self.dimensions.pixel_height as f32;
        self.compute_sidebar_element(
            metrics,
            &Element::new(font, ElementContent::Children(vec![]))
                .display(DisplayType::Block)
                .item_type(UIItemType::Sidebar(SidebarItem::Scrim))
                .min_width(Some(Dimension::Pixels(width)))
                .min_height(Some(Dimension::Pixels(height)))
                .colors(ElementColors {
                    border: Default::default(),
                    bg: InheritableColor::Color(self.sidebar_scrim_color()),
                    text: InheritableColor::Color(self.sidebar_text_color()),
                }),
            euclid::rect(0., 0., width, height),
            width,
            height,
        )
    }

    fn compute_sidebar_element(
        &self,
        metrics: &RenderMetrics,
        element: &Element,
        bounds: RectF,
        pixel_max_x: f32,
        pixel_max_y: f32,
    ) -> anyhow::Result<ComputedElement> {
        self.compute_element(
            &LayoutContext {
                height: DimensionContext {
                    dpi: self.dimensions.dpi as f32,
                    pixel_max: pixel_max_y,
                    pixel_cell: metrics.cell_size.height as f32,
                },
                width: DimensionContext {
                    dpi: self.dimensions.dpi as f32,
                    pixel_max: pixel_max_x,
                    pixel_cell: metrics.cell_size.width as f32,
                },
                bounds,
                metrics,
                gl_state: self.render_state.as_ref().unwrap(),
                // Above the terminal, the tab bar and the key row, below a modal.
                zindex: 20,
            },
            element,
        )
    }

    fn sidebar_notice_height(&self, dpi: f64) -> f32 {
        if self.sidebar.notice.is_some() {
            dp(ROW_HEIGHT_DP, dpi)
        } else {
            0.
        }
    }

    /// The title row plus any notice, above the scrolling list.
    fn sidebar_header_height(&self, dpi: f64) -> f32 {
        dp(ROW_HEIGHT_DP, dpi) + self.sidebar_notice_height(dpi)
    }

    /// The two action rows below the scrolling list.
    fn sidebar_footer_height(&self, dpi: f64) -> f32 {
        2. * dp(ROW_HEIGHT_DP, dpi)
    }

    /// The drawer's background, its title, any notice, and the footer actions.
    fn build_sidebar_frame(
        &self,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        dpi: f64,
        drawer: &RectF,
    ) -> Element {
        let mut children = vec![self.sidebar_label(font, dpi, "Hosts", drawer.width(), true)];

        if let Some(notice) = &self.sidebar.notice {
            children.push(self.sidebar_notice_element(font, dpi, notice, drawer.width()));
        }

        // The footer is pushed to the bottom by a spacer of the height the list
        // viewport occupies. Giving the footer `VerticalAlign::Bottom` would
        // align it within its own row rather than within the drawer.
        //
        // Derived from the same header and footer heights that place the list's
        // viewport, so the spacer cannot disagree with it about where the list
        // ends and push the footer off the bottom of the drawer.
        let spacer_height =
            (drawer.height() - self.sidebar_header_height(dpi) - self.sidebar_footer_height(dpi))
                .max(0.);
        children.push(
            Element::new(font, ElementContent::Children(vec![]))
                .display(DisplayType::Block)
                .min_width(Some(Dimension::Pixels(drawer.width())))
                .min_height(Some(Dimension::Pixels(spacer_height))),
        );

        let mut top_actions: Vec<(&str, SidebarItem)> = vec![("+  Add host", SidebarItem::Add)];
        // Pinning is only offered where it leaves a terminal worth having beside
        // the panel; on a phone in portrait there is no such width.
        if self.can_pin_sidebar() {
            top_actions.push(if self.sidebar.state == SidebarState::Pinned {
                ("Unpin", SidebarItem::Unpin)
            } else {
                ("Pin", SidebarItem::Pin)
            });
        }
        children.push(self.sidebar_action_row(font, dpi, drawer.width(), &top_actions));
        children.push(self.sidebar_action_row(
            font,
            dpi,
            drawer.width(),
            &[
                ("Export", SidebarItem::Export),
                ("Reset", SidebarItem::Reset),
                ("Close", SidebarItem::Close),
            ],
        ));

        Element::new(font, ElementContent::Children(children))
            .display(DisplayType::Block)
            .min_width(Some(Dimension::Pixels(drawer.width())))
            .min_height(Some(Dimension::Pixels(drawer.height())))
            // The drawer's own background is opaque, so the terminal does not
            // read through the host names.
            .colors(ElementColors {
                border: Default::default(),
                bg: InheritableColor::Color(self.sidebar_bg_color()),
                text: InheritableColor::Color(self.sidebar_text_color()),
            })
    }

    /// The scrollable part: stored profiles, then the config file's domains.
    fn build_sidebar_list(
        &self,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        dpi: f64,
        width: f32,
    ) -> Element {
        let mut children = vec![];

        if self.sidebar.profiles().is_empty() {
            children.push(self.sidebar_label(font, dpi, "No saved hosts yet.", width, false));
        }

        for profile in self.sidebar.profiles() {
            children.push(self.sidebar_host_row(font, dpi, width, profile));
        }

        if !self.sidebar.configured.is_empty() {
            children.push(self.sidebar_label(font, dpi, "From wezterm.lua", width, true));
            for domain in &self.sidebar.configured {
                children.push(self.sidebar_configured_row(font, dpi, width, domain));
            }
        }

        // Keys last: they are a thing you set up once and then choose from in
        // the host editor, not something to scroll past on the way to a host.
        children.push(self.sidebar_label(font, dpi, "Keys", width, true));
        if self.sidebar.keys().is_empty() {
            children.push(self.sidebar_label(font, dpi, "No keys yet.", width, false));
        }
        for key in self.sidebar.keys() {
            children.push(self.sidebar_key_row(font, dpi, width, key));
        }
        children.push(self.sidebar_row(
            font,
            dpi,
            width,
            vec![self.sidebar_row_label(font, "+  Add key", width)],
            SidebarItem::AddKey,
        ));

        Element::new(font, ElementContent::Children(children))
            .display(DisplayType::Block)
            .min_width(Some(Dimension::Pixels(width)))
    }

    /// One stored profile: tap the row to connect, with edit and delete at the
    /// right.
    fn sidebar_host_row(
        &self,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        dpi: f64,
        width: f32,
        profile: &HostProfile,
    ) -> Element {
        let label = format!(
            "{}   {}@{}:{}",
            profile.display_name, profile.username, profile.host, profile.port
        );

        // Delete first so that it lands rightmost: floats are placed from the
        // right edge inwards in the order they appear.
        let children = vec![
            self.sidebar_icon(font, dpi, "x", SidebarItem::Delete(profile.id.clone())),
            self.sidebar_icon(font, dpi, "...", SidebarItem::Edit(profile.id.clone())),
            self.sidebar_row_label(font, &label, width - 2. * dp(ICON_DP, dpi)),
        ];

        self.sidebar_row(
            font,
            dpi,
            width,
            children,
            SidebarItem::Connect(profile.id.clone()),
        )
    }

    /// A domain from the config file. Read-only in this release: it belongs to
    /// `wezterm.lua`, and the app has no business rewriting Lua.
    fn sidebar_configured_row(
        &self,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        dpi: f64,
        width: f32,
        domain: &ConfiguredDomain,
    ) -> Element {
        let label = match &domain.username {
            Some(user) => format!("{}   {}@{}", domain.name, user, domain.remote_address),
            None => format!("{}   {}", domain.name, domain.remote_address),
        };
        // Saying which flavour it is matters: the multiplexing one needs a
        // wezterm binary on the far end and fails against a plain sshd, which is
        // otherwise a baffling failure.
        let label = if domain.multiplexed {
            format!("{label}   [mux]")
        } else {
            label
        };

        let children = vec![self.sidebar_row_label(font, &label, width)];
        self.sidebar_row(
            font,
            dpi,
            width,
            children,
            SidebarItem::ConfiguredDomain(domain.name.clone()),
        )
    }

    /// One key in the keychain, with a delete button. Not tappable otherwise:
    /// a key is chosen from the host editor, not from here.
    fn sidebar_key_row(
        &self,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        dpi: f64,
        width: f32,
        key: &KeyEntry,
    ) -> Element {
        let used_by = self
            .sidebar
            .profiles()
            .iter()
            .filter(|profile| profile.key_id.as_deref() == Some(key.id.as_str()))
            .count();
        // Saying how many hosts use it is what makes the delete button safe to
        // press deliberately: the confirmation can then name a consequence.
        let label = match used_by {
            0 => format!("{}   unused", key.name),
            1 => format!("{}   1 host", key.name),
            n => format!("{}   {n} hosts", key.name),
        };

        let children = vec![
            self.sidebar_icon(font, dpi, "x", SidebarItem::DeleteKey(key.id.clone())),
            self.sidebar_row_label(font, &label, width - dp(ICON_DP, dpi)),
        ];
        self.sidebar_row(font, dpi, width, children, SidebarItem::Inert)
    }

    fn sidebar_row(
        &self,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        dpi: f64,
        width: f32,
        children: Vec<Element>,
        item: SidebarItem,
    ) -> Element {
        Element::new(font, ElementContent::Children(children))
            .display(DisplayType::Block)
            .item_type(UIItemType::Sidebar(item))
            .min_width(Some(Dimension::Pixels(width)))
            .min_height(Some(Dimension::Pixels(dp(ROW_HEIGHT_DP, dpi))))
            // No vertical_align: a row is stacked against its neighbours in the
            // list, and asking to be centred would move it down by half the
            // list's height, taking every following row with it.
            .colors(ElementColors {
                border: Default::default(),
                bg: InheritableColor::Color(self.sidebar_row_color()),
                text: InheritableColor::Color(self.sidebar_text_color()),
            })
            .hover_colors(Some(ElementColors {
                border: Default::default(),
                bg: InheritableColor::Color(self.sidebar_row_pressed_color()),
                text: InheritableColor::Color(self.sidebar_text_color()),
            }))
            .padding(BoxDimension {
                left: Dimension::Pixels(dp(ROW_PADDING_DP, dpi)),
                right: Dimension::Pixels(dp(ROW_PADDING_DP, dpi)),
                top: Dimension::Pixels(0.),
                bottom: Dimension::Pixels(0.),
            })
            .margin(BoxDimension {
                left: Dimension::Pixels(0.),
                right: Dimension::Pixels(0.),
                top: Dimension::Pixels(1.),
                bottom: Dimension::Pixels(1.),
            })
            .border_corners(Some(rounded(dp(CORNER_DP, dpi))))
    }

    fn sidebar_row_label(
        &self,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        text: &str,
        width: f32,
    ) -> Element {
        Element::new(font, ElementContent::Text(text.to_string()))
            // Left to its natural height so that `Middle` has room to centre it
            // in the row above. Giving it the row's height instead would make it
            // exactly as tall as its parent, so centring became a no-op and the
            // text sat against the top edge.
            .vertical_align(VerticalAlign::Middle)
            .float(Float::None)
            .max_width(Some(Dimension::Pixels(width.max(0.))))
    }

    /// A small square button at the right of a row.
    fn sidebar_icon(
        &self,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        dpi: f64,
        text: &str,
        item: SidebarItem,
    ) -> Element {
        Element::new(font, ElementContent::Text(text.to_string()))
            .item_type(UIItemType::Sidebar(item))
            .float(Float::Right)
            // Middle centres this button within the taller row; center_content
            // centres the glyph within the button.
            .vertical_align(VerticalAlign::Middle)
            .center_content(true)
            .min_width(Some(Dimension::Pixels(dp(ICON_DP, dpi))))
            .min_height(Some(Dimension::Pixels(dp(ICON_DP, dpi))))
            .colors(ElementColors {
                border: Default::default(),
                bg: InheritableColor::Color(self.sidebar_row_color()),
                text: InheritableColor::Color(self.sidebar_text_color()),
            })
            .hover_colors(Some(ElementColors {
                border: Default::default(),
                bg: InheritableColor::Color(self.sidebar_row_pressed_color()),
                text: InheritableColor::Color(self.sidebar_text_color()),
            }))
            .border_corners(Some(rounded(dp(CORNER_DP, dpi))))
    }

    fn sidebar_label(
        &self,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        dpi: f64,
        text: &str,
        width: f32,
        heading: bool,
    ) -> Element {
        let color = if heading {
            self.sidebar_text_color()
        } else {
            self.sidebar_dim_text_color()
        };
        Element::new(font, ElementContent::Text(text.to_string()))
            .display(DisplayType::Block)
            // Inert rather than no item at all: a tap on the header must be
            // swallowed by the drawer, not fall through to the terminal behind.
            .item_type(UIItemType::Sidebar(SidebarItem::Inert))
            .min_width(Some(Dimension::Pixels(width)))
            .min_height(Some(Dimension::Pixels(dp(ROW_HEIGHT_DP, dpi))))
            .center_content_vertically(true)
            .padding(BoxDimension {
                left: Dimension::Pixels(dp(ROW_PADDING_DP, dpi)),
                right: Dimension::Pixels(dp(ROW_PADDING_DP, dpi)),
                top: Dimension::Pixels(0.),
                bottom: Dimension::Pixels(0.),
            })
            .colors(ElementColors {
                border: Default::default(),
                bg: InheritableColor::Color(self.sidebar_bg_color()),
                text: InheritableColor::Color(color),
            })
    }

    fn sidebar_notice_element(
        &self,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        dpi: f64,
        notice: &str,
        width: f32,
    ) -> Element {
        Element::new(font, ElementContent::Text(notice.to_string()))
            .display(DisplayType::Block)
            .item_type(UIItemType::Sidebar(SidebarItem::Inert))
            .min_width(Some(Dimension::Pixels(width)))
            .min_height(Some(Dimension::Pixels(dp(ROW_HEIGHT_DP, dpi))))
            .center_content_vertically(true)
            .padding(BoxDimension {
                left: Dimension::Pixels(dp(ROW_PADDING_DP, dpi)),
                right: Dimension::Pixels(dp(ROW_PADDING_DP, dpi)),
                top: Dimension::Pixels(0.),
                bottom: Dimension::Pixels(0.),
            })
            .colors(ElementColors {
                border: Default::default(),
                bg: InheritableColor::Color(self.sidebar_notice_color()),
                text: InheritableColor::Color(self.sidebar_text_color()),
            })
    }

    fn sidebar_action_row(
        &self,
        font: &std::rc::Rc<wezterm_font::LoadedFont>,
        dpi: f64,
        width: f32,
        actions: &[(&str, SidebarItem)],
    ) -> Element {
        let share = width / actions.len() as f32;
        let children = actions
            .iter()
            .map(|(label, item)| {
                Element::new(font, ElementContent::Text(label.to_string()))
                    .item_type(UIItemType::Sidebar(item.clone()))
                    .float(Float::None)
                    .center_content(true)
                    .min_width(Some(Dimension::Pixels(share)))
                    .min_height(Some(Dimension::Pixels(dp(ROW_HEIGHT_DP, dpi))))
                    .colors(ElementColors {
                        border: Default::default(),
                        bg: InheritableColor::Color(self.sidebar_row_color()),
                        text: InheritableColor::Color(self.sidebar_text_color()),
                    })
                    .hover_colors(Some(ElementColors {
                        border: Default::default(),
                        bg: InheritableColor::Color(self.sidebar_row_pressed_color()),
                        text: InheritableColor::Color(self.sidebar_text_color()),
                    }))
                    .border_corners(Some(rounded(dp(CORNER_DP, dpi))))
            })
            .collect();

        Element::new(font, ElementContent::Children(children))
            .display(DisplayType::Block)
            .min_width(Some(Dimension::Pixels(width)))
            .min_height(Some(Dimension::Pixels(dp(ROW_HEIGHT_DP, dpi))))
    }

    pub fn paint_sidebar(&self) -> anyhow::Result<Vec<UIItem>> {
        let layout = match self.sidebar.layout.as_ref() {
            Some(layout) => layout,
            None => return Ok(vec![]),
        };
        let gl_state = self.render_state.as_ref().unwrap();

        let mut items = vec![];
        if let Some(scrim) = &layout.scrim {
            self.render_element(scrim, gl_state, None)?;
            items.append(&mut scrim.ui_items());
        }
        self.render_element(&layout.list, gl_state, None)?;
        self.render_element(&layout.frame, gl_state, None)?;

        // The frame's items go after the list's, because a hit test takes the
        // last match: the header and footer must win over a row that has been
        // scrolled behind them. The list's rectangles are clipped to the
        // viewport too, so this is the cheap half of a belt-and-braces pair.
        items.append(&mut layout.list.ui_items());
        items.append(&mut layout.frame.ui_items());
        Ok(items)
    }

    /// Build the sidebar if it is open and not already laid out, then draw it.
    pub fn paint_sidebar_if_open(&mut self) -> anyhow::Result<()> {
        if !self.sidebar.is_open() {
            return Ok(());
        }
        if self.sidebar.layout.is_none() {
            let layout = self.build_sidebar()?;
            self.sidebar.scroll = layout.scroll;
            self.sidebar.layout = Some(layout);
        }
        let mut items = self.paint_sidebar()?;
        self.ui_items.append(&mut items);
        Ok(())
    }

    fn sidebar_bg_color(&self) -> LinearRgba {
        self.config.window_frame.inactive_titlebar_bg.to_linear()
    }

    fn sidebar_text_color(&self) -> LinearRgba {
        self.config.window_frame.active_titlebar_fg.to_linear()
    }

    fn sidebar_dim_text_color(&self) -> LinearRgba {
        let bg = self.sidebar_bg_color();
        let fg = self.sidebar_text_color();
        mix(bg, fg, 0.6)
    }

    fn sidebar_row_color(&self) -> LinearRgba {
        mix(self.sidebar_bg_color(), self.sidebar_text_color(), 0.12)
    }

    fn sidebar_row_pressed_color(&self) -> LinearRgba {
        mix(self.sidebar_bg_color(), self.sidebar_text_color(), 0.28)
    }

    fn sidebar_notice_color(&self) -> LinearRgba {
        // The cursor colour, as the key row uses for an armed modifier: the
        // theme has already chosen something that stands out against the
        // background.
        mix(
            self.sidebar_bg_color(),
            self.config
                .resolved_palette
                .cursor_bg
                .map(|c| c.to_linear())
                .unwrap_or_else(|| LinearRgba::with_components(0.5, 0.5, 0.5, 1.)),
            0.5,
        )
    }

    /// The dimming over the terminal.
    ///
    /// Translucent black rather than a tint of the frame colour, so that it reads
    /// as "this is behind something" under a light theme as well as a dark one.
    fn sidebar_scrim_color(&self) -> LinearRgba {
        LinearRgba::with_components(0., 0., 0., 0.55)
    }
}

fn mix(from: LinearRgba, to: LinearRgba, amount: f32) -> LinearRgba {
    LinearRgba(
        from.0 + (to.0 - from.0) * amount,
        from.1 + (to.1 - from.1) * amount,
        from.2 + (to.2 - from.2) * amount,
        from.3,
    )
}

fn rounded(radius: f32) -> Corners {
    Corners {
        top_left: SizedPoly {
            width: Dimension::Pixels(radius),
            height: Dimension::Pixels(radius),
            poly: TOP_LEFT_ROUNDED_CORNER,
        },
        top_right: SizedPoly {
            width: Dimension::Pixels(radius),
            height: Dimension::Pixels(radius),
            poly: TOP_RIGHT_ROUNDED_CORNER,
        },
        bottom_left: SizedPoly {
            width: Dimension::Pixels(radius),
            height: Dimension::Pixels(radius),
            poly: BOTTOM_LEFT_ROUNDED_CORNER,
        },
        bottom_right: SizedPoly {
            width: Dimension::Pixels(radius),
            height: Dimension::Pixels(radius),
            poly: BOTTOM_RIGHT_ROUNDED_CORNER,
        },
    }
}

/// The height the list's rows really occupy, measured rather than counted.
fn list_extent(list: &ComputedElement) -> f32 {
    use crate::termwindow::box_model::ComputedElementContent;
    match &list.content {
        ComputedElementContent::Children(rows) => match (rows.first(), rows.last()) {
            (Some(first), Some(last)) => (last.bounds.max_y() - first.bounds.min_y()).max(0.),
            _ => 0.,
        },
        _ => 0.,
    }
}

impl TermWindow {
    /// A tap on the sidebar.
    ///
    /// Only the press edge is acted on. A release would fire a second time, and
    /// for `Delete` and `Connect` doing the thing twice is not harmless.
    pub fn mouse_event_sidebar(
        &mut self,
        item: SidebarItem,
        event: window::MouseEvent,
        context: &dyn WindowOps,
    ) {
        use window::{MouseEventKind, MousePress};
        if !matches!(event.kind, MouseEventKind::Press(MousePress::Left)) {
            return;
        }

        match item {
            SidebarItem::Scrim | SidebarItem::Close => {
                self.set_sidebar_state(SidebarState::Closed);
            }
            // Drawn, but not a control. Swallowing the tap is the whole job:
            // without an item here it would reach the terminal behind the
            // drawer and move the cursor or focus another pane.
            SidebarItem::Inert => {}
            SidebarItem::Connect(id) => {
                let Some(profile) = self.sidebar.profile(&id).cloned() else {
                    return;
                };
                self.connect_to_profile(&profile);
                // Close on connect: the new tab is what the user asked for, and
                // leaving the drawer over it means a second tap to see it.
                self.set_sidebar_state(SidebarState::Closed);
            }
            SidebarItem::ConfiguredDomain(name) => {
                self.spawn_tab(&config::keyassignment::SpawnTabDomain::DomainName(name));
                self.set_sidebar_state(SidebarState::Closed);
            }
            SidebarItem::Add => self.edit_host_profile(None),
            SidebarItem::AddKey => self.add_key(),
            SidebarItem::DeleteKey(id) => self.delete_key(&id),
            SidebarItem::Edit(id) => self.edit_host_profile(Some(id)),
            SidebarItem::Delete(id) => self.delete_host_profile(&id),
            SidebarItem::Pin => self.set_sidebar_state(SidebarState::Pinned),
            SidebarItem::Unpin => self.set_sidebar_state(SidebarState::Overlay),
            SidebarItem::Export => self.export_host_profiles(),
            SidebarItem::Reset => self.reset_host_profiles(),
        }

        self.sidebar.invalidate();
        context.invalidate();
    }

    /// Register the profile's domain and open a tab in it.
    ///
    /// The spawn goes through the ordinary machinery rather than calling
    /// `Domain::spawn` directly, so that a `RemoteSshDomain` shows its progress
    /// and any failure inside the pane it is creating -- which is where a user
    /// looking at a connection that did not come up will be looking.
    fn connect_to_profile(&mut self, profile: &HostProfile) {
        let key_path = self
            .sidebar
            .repo
            .as_ref()
            .and_then(|repo| repo.key_path_for(profile));

        let domain = match crate::hosts::ensure_domain(profile, key_path.as_deref()) {
            Ok(domain) => domain,
            Err(err) => {
                log::error!("could not prepare {}: {err:#}", profile.display_name);
                self.sidebar
                    .set_notice(Some(format!("Could not connect: {err:#}")));
                return;
            }
        };

        if profile.multiplexing {
            self.attach_to_profile(profile, domain);
        } else {
            self.spawn_tab(&config::keyassignment::SpawnTabDomain::DomainName(
                profile.domain_name(),
            ));
        }
    }

    /// Adopt whatever is already running on the far end, and only start
    /// something if there was nothing there.
    ///
    /// This is the difference the multiplexing option is *for*. Spawning
    /// unconditionally would connect to a server holding the user's panes and
    /// then put a brand new empty one in front of them, which looks exactly like
    /// the state having been lost -- the panes are still there, just not the ones
    /// being shown.
    fn attach_to_profile(&mut self, profile: &HostProfile, domain: std::sync::Arc<dyn Domain>) {
        let mux_window_id = self.mux_window_id;
        let name = profile.display_name.clone();
        let Some(window) = self.window.clone() else {
            return;
        };

        promise::spawn::spawn(async move {
            if let Err(err) = domain.attach(Some(mux_window_id)).await {
                log::error!("could not attach to {name}: {err:#}");
                window.notify(TermWindowNotif::SidebarNotice(format!(
                    "Could not reach the session on {name}: {err:#}. \
                     Keeping the session running needs a matching wezterm-mux-server \
                     installed on that host."
                )));
                return;
            }

            let adopted = Mux::get()
                .iter_panes()
                .iter()
                .any(|pane| pane.domain_id() == domain.domain_id());
            if !adopted {
                // Nothing was running, so this is a first connection rather than
                // a reattach and there is something to start.
                window.notify(TermWindowNotif::SpawnTabInDomain(domain.domain_id()));
            }
        })
        .detach();
    }

    /// Add a profile, or edit one.
    ///
    /// The form runs asynchronously in a native dialog, so the result comes back
    /// through a window notification rather than from here; see
    /// `TermWindowNotif::HostProfileEdited`.
    fn edit_host_profile(&mut self, id: Option<String>) {
        let existing = id.and_then(|id| self.sidebar.profile(&id).cloned());
        let Some(window) = self.window.clone() else {
            return;
        };
        // Copied rather than borrowed: the form outlives this call, and the
        // keychain it offers is the one that existed when it was opened.
        let keys = self.sidebar.keys().to_vec();

        promise::spawn::spawn(async move {
            match crate::hosts::edit_interactively(existing.as_ref(), &keys).await {
                Ok(Some(profile)) => {
                    window.notify(TermWindowNotif::HostProfileEdited {
                        profile,
                        is_new: existing.is_none(),
                    });
                }
                // Cancelled: nothing to do, and nothing to report.
                Ok(None) => {}
                Err(err) => {
                    log::error!("the host editor failed: {err:#}");
                    window.notify(TermWindowNotif::SidebarNotice(format!(
                        "Could not edit the host: {err:#}"
                    )));
                }
            }
        })
        .detach();
    }

    /// Import a private key into the keychain.
    ///
    /// The dialog only collects; the write happens back here, where the
    /// repository that mints the key's id is reachable.
    fn add_key(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };

        promise::spawn::spawn(async move {
            match crate::hosts::import_key_interactively().await {
                Ok(Some(imported)) => {
                    window.notify(TermWindowNotif::KeyImported(imported));
                }
                Ok(None) => {}
                Err(err) => {
                    log::error!("the key import failed: {err:#}");
                    window.notify(TermWindowNotif::SidebarNotice(format!(
                        "Could not import the key: {err:#}"
                    )));
                }
            }
        })
        .detach();
    }

    /// Store a key that was just pasted.
    pub fn key_imported(&mut self, imported: crate::hosts::ImportedKey) {
        let Some(repo) = self.sidebar.repository_mut() else {
            return;
        };
        // `imported` is consumed here and not logged: it carries the key.
        let outcome = repo.add_key(&imported.name, &imported.pem);
        drop(imported);
        match outcome {
            Ok(entry) => {
                self.sidebar
                    .set_notice(Some(format!("Added {}", entry.name)));
            }
            Err(err) => {
                log::error!("could not store the key: {err:#}");
                self.sidebar
                    .set_notice(Some(format!("Could not store the key: {err:#}")));
            }
        }
        self.sidebar.invalidate();
    }

    /// Forget a key.
    ///
    /// Confirmed, because it detaches the key from every host that used it and
    /// there is no undo: the material is gone and only the user has another copy.
    fn delete_key(&mut self, id: &str) {
        let Some(key) = self.sidebar.keys().iter().find(|key| key.id == id).cloned() else {
            return;
        };
        let used_by = self
            .sidebar
            .profiles()
            .iter()
            .filter(|profile| profile.key_id.as_deref() == Some(id.as_ref()))
            .count();
        let Some(window) = self.window.clone() else {
            return;
        };

        promise::spawn::spawn(async move {
            let mut spec = crate::dialog::DialogSpec::new("Delete key")
                .submit_label("Delete")
                .grave(true);
            spec = spec.message(&match used_by {
                0 => format!(
                    "Delete the key \"{}\"? This app's only copy of it is removed.",
                    key.name
                ),
                1 => format!(
                    "Delete the key \"{}\"? One host uses it and will be left with \
                     no key. This app's only copy of it is removed.",
                    key.name
                ),
                n => format!(
                    "Delete the key \"{}\"? {n} hosts use it and will be left with \
                     no key. This app's only copy of it is removed.",
                    key.name
                ),
            });

            match crate::dialog::confirm(&spec).await {
                Ok(true) => window.notify(TermWindowNotif::KeyDeleted(key.id)),
                Ok(false) => {}
                Err(err) => log::error!("the delete confirmation failed: {err:#}"),
            }
        })
        .detach();
    }

    /// Carry out a confirmed key deletion.
    pub fn key_deleted(&mut self, id: String) {
        let Some(repo) = self.sidebar.repository_mut() else {
            return;
        };
        if let Err(err) = repo.remove_key(&id) {
            log::error!("could not delete the key: {err:#}");
            self.sidebar
                .set_notice(Some(format!("Could not delete the key: {err:#}")));
        }
        self.sidebar.invalidate();
    }

    /// Store an edited profile.
    ///
    /// `add` and `update` are deliberately different calls: only `update` spends
    /// a generation, and a generation is what leaves a dead domain behind for the
    /// life of the process.
    pub fn host_profile_edited(&mut self, profile: HostProfile, is_new: bool) {
        let Some(repo) = self.sidebar.repository_mut() else {
            return;
        };
        let result = if is_new {
            repo.add(profile)
        } else {
            repo.update(profile)
        };
        match result {
            Ok(()) => self.sidebar.set_notice(None),
            Err(err) => self
                .sidebar
                .set_notice(Some(format!("Could not save: {err:#}"))),
        }
        if let Some(window) = self.window.as_ref() {
            window.invalidate();
        }
    }

    fn delete_host_profile(&mut self, id: &str) {
        let Some(repo) = self.sidebar.repository_mut() else {
            return;
        };
        match repo.remove(id) {
            Ok(()) => self.sidebar.set_notice(None),
            Err(err) => self
                .sidebar
                .set_notice(Some(format!("Could not delete: {err:#}"))),
        }
    }

    /// Hand the profile list to whatever the user keeps things in.
    ///
    /// The stored file is unreachable on a release build -- app-private storage,
    /// and `run-as` refused for a package that is not debuggable -- so this is
    /// the only way anything ever gets out. It carries profiles only.
    fn export_host_profiles(&mut self) {
        let document = match self.sidebar.repository_mut() {
            Some(repo) => match repo.export_document() {
                Ok(document) => document,
                Err(err) => {
                    self.sidebar
                        .set_notice(Some(format!("Could not export: {err:#}")));
                    return;
                }
            },
            None => return,
        };

        // A share intent needs no storage permission and no file picker. Where
        // there is nothing to share to, fall back to the clipboard, which at
        // least gets the list off the device.
        match crate::hosts::share_text(&document) {
            Ok(()) => self.sidebar.set_notice(Some("Exported.".to_string())),
            Err(err) => {
                log::warn!("could not share the host list: {err:#}");
                if let Some(window) = self.window.as_ref() {
                    window.set_clipboard(window::Clipboard::Clipboard, document);
                }
                self.sidebar
                    .set_notice(Some("Copied the host list to the clipboard.".to_string()));
            }
        }
    }

    /// Delete the stored list.
    ///
    /// Confirmed in a dialog, because there is no undo and no other copy: the
    /// file cannot be recovered from anywhere on a release build.
    fn reset_host_profiles(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let count = self.sidebar.profiles().len();

        promise::spawn::spawn(async move {
            let spec = crate::dialog::DialogSpec::new("Delete all hosts?")
                .message(&format!(
                    "This removes {count} saved host(s). There is no undo, and the \
                     stored file cannot be recovered from the device."
                ))
                .submit_label("Delete all")
                .grave(true);
            match crate::dialog::confirm(&spec).await {
                Ok(true) => window.notify(TermWindowNotif::HostProfilesReset),
                Ok(false) => {}
                Err(err) => log::error!("could not confirm the reset: {err:#}"),
            }
        })
        .detach();
    }

    pub fn host_profiles_reset(&mut self) {
        if let Some(repo) = self.sidebar.repository_mut() {
            if let Err(err) = repo.reset() {
                self.sidebar
                    .set_notice(Some(format!("Could not reset: {err:#}")));
                return;
            }
        }
        self.sidebar
            .set_notice(Some("All hosts deleted.".to_string()));
        if let Some(window) = self.window.as_ref() {
            window.invalidate();
        }
    }
}

/// True when pinning leaves a terminal worth having beside the panel.
fn can_pin(surface_width: f32, dpi: f64) -> bool {
    surface_width - dp(PINNED_WIDTH_DP, dpi) >= dp(MIN_TERMINAL_WIDTH_DP, dpi)
}

/// The drawer's width for a state and a surface.
///
/// A free function so that the arithmetic can be tested without a window: it is
/// the part with a trade-off in it, and 80% of a tablet in landscape is not a
/// drawer.
fn sidebar_width(state: SidebarState, surface_width: f32, dpi: f64) -> f32 {
    match state {
        SidebarState::Closed => 0.,
        SidebarState::Overlay => {
            (surface_width * OVERLAY_WIDTH_FRACTION).min(dp(MAX_WIDTH_DP, dpi))
        }
        // Never more than half the screen. Pinned mode takes its width *from*
        // the terminal, and a terminal narrower than the panel beside it is not
        // a terminal.
        SidebarState::Pinned => dp(PINNED_WIDTH_DP, dpi).min(surface_width * 0.5),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The reference device: 1080px wide at density 2.75, which wezterm sees as
    /// 198dpi.
    const DPI: f64 = 440. * 72. / 160.;
    const PHONE_WIDTH: f32 = 1080.;

    #[test]
    fn a_closed_sidebar_has_no_width() {
        assert_eq!(sidebar_width(SidebarState::Closed, PHONE_WIDTH, DPI), 0.);
        assert!(!SidebarState::Closed.is_open());
        assert!(SidebarState::Overlay.is_open());
        assert!(SidebarState::Pinned.is_open());
    }

    #[test]
    fn an_overlay_takes_most_of_a_phone_but_not_all_of_it() {
        let width = sidebar_width(SidebarState::Overlay, PHONE_WIDTH, DPI);
        // 80% of the screen, so the terminal behind is visibly still there.
        assert_eq!(width, PHONE_WIDTH * 0.8);
        assert!(width < PHONE_WIDTH);
    }

    #[test]
    fn an_overlay_stops_growing_on_a_wide_screen() {
        // 80% of a tablet in landscape is not a drawer.
        let width = sidebar_width(SidebarState::Overlay, 2400., DPI);
        assert_eq!(width, dp(MAX_WIDTH_DP, DPI));
        assert!(width < 2400. * 0.8);
    }

    #[test]
    fn pinned_never_takes_more_than_half_the_screen() {
        // Pinned mode takes its width from the terminal, so on a narrow screen
        // the cap matters: 320dp is 880px, most of a 1080px phone.
        assert_eq!(
            sidebar_width(SidebarState::Pinned, PHONE_WIDTH, DPI),
            PHONE_WIDTH / 2.
        );
        // On a wide screen it is the nominal width.
        assert_eq!(
            sidebar_width(SidebarState::Pinned, 2400., DPI),
            dp(PINNED_WIDTH_DP, DPI)
        );
    }

    #[test]
    fn pinning_is_offered_only_where_a_terminal_fits_beside_it() {
        // A phone in portrait, 392dp: the panel and a usable terminal cannot both
        // have room, so the choice is not offered rather than offered and
        // regretted.
        assert!(!can_pin(PHONE_WIDTH, DPI));
        // The same phone rotated, 873dp.
        assert!(can_pin(2400., DPI));

        // The threshold is 320dp of panel plus 280dp of terminal, which lands on
        // 600dp -- the same figure Android itself uses to mean "a large screen",
        // arrived at from the other direction.
        assert!(can_pin(dp(600., DPI), DPI));
        assert!(!can_pin(dp(599., DPI), DPI));
    }

    #[test]
    fn a_row_is_a_comfortable_tap_target() {
        // A mis-tap in this list connects somewhere or deletes something, so the
        // rows and the small buttons on them both clear the 44dp floor.
        assert!(ROW_HEIGHT_DP >= 44.);
        assert!(ICON_DP >= 44.);
    }
}
