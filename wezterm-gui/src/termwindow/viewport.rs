//! The terminal viewport rectangle.
//!
//! `TermWindow::dimensions` is not reliably the size of the surface. It is the
//! size the GUI would *like* the window to be, and the two diverge whenever
//! something asks for a size the window will not take: attaching to a 50-row
//! remote pane sets it to roughly 3960px tall against a real surface of 2235px,
//! and logcat shows the window declining:
//!
//! ```text
//! cannot resize window to match RowsAndCols { rows: 50, cols: 111 }
//! because window_state is FULL_SCREEN
//! ```
//!
//! Everything the GUI draws is positioned in a coordinate space whose origin is
//! `dimensions`, so while it disagrees with the surface the whole frame is
//! offset -- and anything computed from it and cached across that window is
//! simply wrong. An earlier attempt to place the extra-keys row's touch band from
//! `dimensions` produced a top edge of 3828px on that 2235px surface, and the row
//! silently ignored every drag.
//!
//! Two things follow, and they are the same idea from both ends:
//!
//! * a window that *cannot* resize does not get a `dimensions` it will never
//!   have. On Android that is not a failure to be worked around; it is how the
//!   platform works, and the surface the backend reports is the only truth about
//!   it. `surface_dimensions` holds that, written only by the backend.
//!
//! * grid recalculation and pty resize take an explicit rectangle rather than
//!   reaching for the window size. That rectangle is also where a pinned sidebar
//!   subtracts its width, which is the one place terminal content is excluded
//!   from part of a surface it would otherwise fill.

use crate::termwindow::sidebar::SidebarState;
use crate::termwindow::TermWindow;
use ::window::Dimensions;

/// The part of the surface that the window's own content may occupy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalViewport {
    /// Pixels excluded at the left, currently only by a pinned sidebar.
    pub left_inset: usize,
    /// Width available after `left_inset`.
    pub width: usize,
    pub height: usize,
    pub dpi: usize,
}

impl TerminalViewport {
    /// The viewport as a `Dimensions`, for the layout arithmetic that still
    /// speaks in those terms.
    pub fn as_dimensions(&self) -> Dimensions {
        Dimensions {
            pixel_width: self.width,
            pixel_height: self.height,
            dpi: self.dpi,
        }
    }
}

impl TermWindow {
    /// The rectangle terminal content lives in.
    ///
    /// This is the surface the backend last reported, less anything terminal
    /// content must not occupy. Borders, the tab bar, the key row and the
    /// configured padding are *not* subtracted here: each is computed where it is
    /// used, and folding them in as well would leave two places to disagree about
    /// the same number.
    pub fn terminal_viewport(&self) -> TerminalViewport {
        let surface = self.surface_dimensions;
        // A pinned sidebar is the only thing that takes width away from the
        // terminal rather than being drawn over it.
        let left_inset = match self.sidebar.state {
            SidebarState::Pinned => self.sidebar_pixel_width() as usize,
            SidebarState::Closed | SidebarState::Overlay => 0,
        }
        .min(surface.pixel_width);

        TerminalViewport {
            left_inset,
            width: surface.pixel_width - left_inset,
            height: surface.pixel_height,
            dpi: surface.dpi,
        }
    }

    /// Record the size the backend says the surface is.
    ///
    /// The only writer. Anything else assigning this would reintroduce exactly
    /// the divergence the module comment describes.
    pub fn note_surface_dimensions(&mut self, dimensions: Dimensions) {
        self.surface_dimensions = dimensions;
    }

    /// The dimensions to lay the window out against, given a requested size.
    ///
    /// When the window can resize, that is the request: it will become true, and
    /// the speculative resize that follows is what makes it so. When it cannot,
    /// the request is discarded in favour of the surface, because laying out
    /// against a size the window will never have puts the grid, the key row and
    /// the sidebar somewhere the user cannot see -- and offsets everything drawn,
    /// since the render projection is centred on this value too.
    pub fn dimensions_for_layout(&self, requested: &Dimensions) -> Dimensions {
        let chosen = dimensions_for_layout(
            *requested,
            self.surface_dimensions,
            self.window_state.can_resize(),
        );
        if chosen != *requested {
            log::debug!(
                "the window cannot resize to {requested:?} in state {:?}; laying out against \
                 the surface it has, {chosen:?}",
                self.window_state,
            );
        }
        chosen
    }
}

/// Which size to lay out against, given a request and the surface.
///
/// A free function so the decision can be tested without a window.
pub(crate) fn dimensions_for_layout(
    requested: Dimensions,
    surface: Dimensions,
    can_resize: bool,
) -> Dimensions {
    if can_resize {
        requested
    } else {
        surface
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn surface() -> Dimensions {
        Dimensions {
            pixel_width: 1080,
            pixel_height: 2235,
            dpi: 198,
        }
    }

    fn viewport(left_inset: usize) -> TerminalViewport {
        let surface = surface();
        TerminalViewport {
            left_inset,
            width: surface.pixel_width - left_inset,
            height: surface.pixel_height,
            dpi: surface.dpi,
        }
    }

    #[test]
    fn a_closed_sidebar_leaves_the_whole_surface() {
        let view = viewport(0);
        assert_eq!(view.width, 1080);
        assert_eq!(view.as_dimensions(), surface());
    }

    #[test]
    fn a_resizable_window_gets_the_size_it_asked_for() {
        let requested = Dimensions {
            pixel_width: 1080,
            pixel_height: 3960,
            dpi: 198,
        };
        assert_eq!(dimensions_for_layout(requested, surface(), true), requested);
    }

    #[test]
    fn a_window_that_cannot_resize_keeps_the_surface_it_has() {
        // Attaching to a 50-row remote pane asks for roughly 3960px against a
        // real surface of 2235px. Accepting the request would offset everything
        // drawn -- the projection is centred on this value -- and put the key row
        // at a top edge of 3828px, where it silently ignored every drag.
        let requested = Dimensions {
            pixel_width: 1080,
            pixel_height: 3960,
            dpi: 198,
        };
        assert_eq!(
            dimensions_for_layout(requested, surface(), false),
            surface()
        );
    }

    #[test]
    fn a_request_that_matches_the_surface_is_not_a_special_case() {
        assert_eq!(
            dimensions_for_layout(surface(), surface(), false),
            surface()
        );
    }

    #[test]
    fn a_pinned_sidebar_takes_width_off_the_terminal() {
        let view = viewport(540);
        assert_eq!(view.width, 540);
        assert_eq!(view.height, 2235);
        // The height is untouched: a sidebar is a column, and taking height from
        // the grid as well would leave a gap under it.
        assert_eq!(view.as_dimensions().pixel_height, surface().pixel_height);
    }
}
