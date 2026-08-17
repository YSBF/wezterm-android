//! The GUI half of the gesture region registry.
//!
//! `window::gesture` defines what a region is and why it is declared as an
//! anchor and an extent rather than as a rectangle. This module decides which
//! regions exist for the current state of the window and what to do with a drag
//! that comes back from one.
//!
//! Everything here is a no-op on a desktop backend, where `set_gesture_regions`
//! has an empty default implementation and no `RegionDrag` is ever produced.

use crate::termwindow::TermWindow;
use window::gesture::{AnchorEdge, GestureClaims, GestureRegion, GestureRegionId};
use window::{Window, WindowOps};

impl TermWindow {
    /// Recompute the regions that claim touch gestures and hand them to the
    /// backend.
    ///
    /// Call this whenever something that owns a region changes size or
    /// visibility. It is cheap -- a couple of small structs -- so it is not
    /// worth caching against the risk of a stale registry, which shows up as
    /// gestures going to the wrong widget.
    pub fn publish_gesture_regions(&self) {
        let window = match self.window.as_ref() {
            Some(window) => window,
            None => return,
        };

        let mut regions = vec![];

        let key_row_height = self.key_row_pixel_height().unwrap_or(0.);
        if key_row_height > 0. {
            // The row is a strip of buttons. It claims drags on both axes even
            // though only the sideways component pans it, because a vertical
            // drag that started on a button scrolling the terminal behind the
            // row is not what anyone means; and it declines long press, which
            // otherwise begins selecting the terminal text underneath.
            regions.push(
                GestureRegion::edge(GestureRegionId::KeyRow, AnchorEdge::Bottom, key_row_height)
                    .claims(GestureClaims::DRAG_HORIZONTAL | GestureClaims::DRAG_VERTICAL)
                    .priority(PRIORITY_KEY_ROW),
            );
        }

        window.set_gesture_regions(regions);
    }

    /// A drag that a region claimed, in raw pixels since the previous motion
    /// event.
    pub fn region_drag_impl(
        &mut self,
        region: GestureRegionId,
        dx: f32,
        _dy: f32,
        window: &Window,
    ) {
        match region {
            GestureRegionId::KeyRow => {
                // Dragging the row left brings the trailing keys into view, so
                // the scroll offset moves opposite to the finger. The row is
                // slid in place rather than rebuilt; see `scroll_key_row`.
                if self.scroll_key_row(-dx) {
                    window.invalidate();
                }
            }
            GestureRegionId::Sidebar | GestureRegionId::SidebarScrim => {}
        }
    }
}

/// Where the key row ranks. An overlay drawn on top of it must be given a
/// higher number so that it wins the overlap.
pub const PRIORITY_KEY_ROW: i32 = 10;
