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

        let border = self.get_os_border();
        window.set_gesture_regions(regions_for(&RegionMetrics {
            key_row_height: self.key_row_pixel_height().unwrap_or(0.),
            sidebar_width: self.sidebar_pixel_width(),
            tab_bar_height: if self.show_tab_bar {
                self.tab_bar_pixel_height().unwrap_or(0.)
            } else {
                0.
            },
            border_top: border.top.get() as f32,
            border_bottom: border.bottom.get() as f32,
        }));
    }

    /// A drag that a region claimed, in raw pixels since the previous motion
    /// event.
    pub fn region_drag_impl(&mut self, region: GestureRegionId, dx: f32, dy: f32, window: &Window) {
        match region {
            GestureRegionId::KeyRow => {
                // Dragging the row left brings the trailing keys into view, so
                // the scroll offset moves opposite to the finger. The row is
                // slid in place rather than rebuilt; see `scroll_key_row`.
                if self.scroll_key_row(-dx) {
                    window.invalidate();
                }
            }
            GestureRegionId::Sidebar => {
                // Same sign convention: dragging the list up brings later hosts
                // into view.
                if self.scroll_sidebar(-dy) {
                    window.invalidate();
                }
            }
            // Claimed only so that the terminal underneath the dimming does not
            // scroll. Tapping the scrim closes the sidebar, and that arrives as
            // an ordinary tap resolved against the scrim's own UIItem.
            GestureRegionId::SidebarScrim => {}
        }
    }
}

/// Where the key row ranks. An overlay drawn on top of it must be given a
/// higher number so that it wins the overlap.
pub const PRIORITY_KEY_ROW: i32 = 10;

/// The dimmed area outside an open overlay sidebar, above the key row it covers.
pub const PRIORITY_SIDEBAR_SCRIM: i32 = 20;

/// The drawer itself, above its own scrim.
pub const PRIORITY_SIDEBAR: i32 = 30;

/// What the region set depends on.
///
/// Gathered into a struct so that the region set can be built -- and tested --
/// without a window, a font or a GL context.
pub(crate) struct RegionMetrics {
    pub key_row_height: f32,
    /// Zero when the sidebar is closed.
    pub sidebar_width: f32,
    pub tab_bar_height: f32,
    pub border_top: f32,
    pub border_bottom: f32,
}

/// The regions that claim gestures, for a given window layout.
pub(crate) fn regions_for(metrics: &RegionMetrics) -> Vec<GestureRegion> {
    let mut regions = vec![];

    if metrics.key_row_height > 0. {
        // The row is a strip of buttons. It claims drags on both axes even
        // though only the sideways component pans it, because a vertical drag
        // that started on a button scrolling the terminal behind the row is not
        // what anyone means; and it declines long press, which otherwise begins
        // selecting the terminal text underneath.
        regions.push(
            GestureRegion::edge(
                GestureRegionId::KeyRow,
                AnchorEdge::Bottom,
                metrics.key_row_height,
            )
            .claims(GestureClaims::DRAG_HORIZONTAL | GestureClaims::DRAG_VERTICAL)
            .priority(PRIORITY_KEY_ROW),
        );
    }

    if metrics.sidebar_width > 0. {
        // An open sidebar is not only its own rectangle. A drag on the dimmed
        // area outside it must not scroll the terminal it is dimming, so the
        // whole surface is claimed at a lower priority than the drawer itself.
        // The drawer then wins its own rectangle, including the part of the key
        // row it covers.
        regions.push(
            GestureRegion::whole_surface(GestureRegionId::SidebarScrim)
                .claims(GestureClaims::DRAG_HORIZONTAL | GestureClaims::DRAG_VERTICAL)
                .priority(PRIORITY_SIDEBAR_SCRIM),
        );

        // The drawer begins below the tab bar and stops above the key row, which
        // is exactly why a region declares cross-axis insets: it is
        // left-anchored, so its extent along its own axis is a width and the
        // vertical bounds have to be stated separately.
        regions.push(
            GestureRegion::edge(
                GestureRegionId::Sidebar,
                AnchorEdge::Left,
                metrics.sidebar_width,
            )
            .cross_insets(
                metrics.tab_bar_height + metrics.border_top,
                metrics.key_row_height + metrics.border_bottom,
            )
            // Both axes for the same reason as the key row: a drag that began on
            // the host list belongs to the list. Long press is declined, so
            // holding a finger on a host name does not begin selecting the
            // terminal text behind the drawer.
            .claims(GestureClaims::DRAG_HORIZONTAL | GestureClaims::DRAG_VERTICAL)
            .priority(PRIORITY_SIDEBAR),
        );
    }

    regions
}

#[cfg(test)]
mod test {
    use super::*;

    /// The reference device: a 48dp key row and a 60px tab bar at density 2.75.
    fn phone() -> RegionMetrics {
        RegionMetrics {
            key_row_height: 132.,
            sidebar_width: 0.,
            tab_bar_height: 60.,
            border_top: 0.,
            border_bottom: 0.,
        }
    }

    fn find(regions: &[GestureRegion], id: GestureRegionId) -> Option<&GestureRegion> {
        regions.iter().find(|region| region.id == id)
    }

    #[test]
    fn a_closed_sidebar_publishes_only_the_key_row() {
        let regions = regions_for(&phone());
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].id, GestureRegionId::KeyRow);
        // No long press: it would begin selecting the terminal text behind a
        // strip of buttons, which is the defect the registry exists to fix.
        assert!(!regions[0].claims.contains(GestureClaims::LONG_PRESS));
        assert!(regions[0].claims.claims_drag());
    }

    #[test]
    fn no_key_row_means_no_region_for_it() {
        let regions = regions_for(&RegionMetrics {
            key_row_height: 0.,
            ..phone()
        });
        assert!(regions.is_empty());
    }

    #[test]
    fn an_open_sidebar_claims_the_whole_surface_as_well_as_itself() {
        let regions = regions_for(&RegionMetrics {
            sidebar_width: 800.,
            ..phone()
        });

        let drawer = find(&regions, GestureRegionId::Sidebar).unwrap();
        let scrim = find(&regions, GestureRegionId::SidebarScrim).unwrap();
        let row = find(&regions, GestureRegionId::KeyRow).unwrap();

        // The drawer outranks its own scrim, and both outrank the key row they
        // are drawn over.
        assert!(drawer.priority > scrim.priority);
        assert!(scrim.priority > row.priority);

        // The drawer sits below the tab bar and above the key row.
        assert_eq!(drawer.anchor, AnchorEdge::Left);
        assert_eq!(drawer.thickness, 800.);
        assert_eq!(drawer.cross_start, 60.);
        assert_eq!(drawer.cross_end, 132.);

        // The scrim is the whole surface, however large that turns out to be.
        assert!(scrim.contains(0., 0., 1080., 2235.));
        assert!(scrim.contains(1079., 2234., 1080., 2235.));
    }

    #[test]
    fn the_drawer_wins_its_own_rectangle_including_the_key_row_it_covers() {
        let mut regions = regions_for(&RegionMetrics {
            sidebar_width: 800.,
            ..phone()
        });
        // The touch layer resolves by descending priority.
        regions.sort_by(|a, b| b.priority.cmp(&a.priority));

        let at = |x: f64, y: f64| {
            regions
                .iter()
                .find(|region| region.contains(x, y, 1080., 2235.))
                .map(|region| region.id)
        };

        // Inside the drawer.
        assert_eq!(at(100., 500.), Some(GestureRegionId::Sidebar));
        // Beside it, over the terminal: the scrim, so the terminal does not
        // scroll under the dimming.
        assert_eq!(at(900., 500.), Some(GestureRegionId::SidebarScrim));
        // The part of the key row the drawer covers belongs to the drawer, and
        // the part beside it to the scrim rather than to the row.
        assert_eq!(at(100., 2230.), Some(GestureRegionId::SidebarScrim));
        assert_eq!(at(900., 2230.), Some(GestureRegionId::SidebarScrim));
        // Over the tab bar, which the drawer stops below.
        assert_eq!(at(100., 10.), Some(GestureRegionId::SidebarScrim));
    }

    #[test]
    fn borders_are_taken_out_of_the_drawers_vertical_extent() {
        let regions = regions_for(&RegionMetrics {
            sidebar_width: 800.,
            border_top: 4.,
            border_bottom: 6.,
            ..phone()
        });
        let drawer = find(&regions, GestureRegionId::Sidebar).unwrap();
        assert_eq!(drawer.cross_start, 64.);
        assert_eq!(drawer.cross_end, 138.);
    }
}
