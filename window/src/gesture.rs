//! The gesture region registry.
//!
//! The touch layer has to know which part of the surface a drag belongs to.
//! Its first widget -- the extra-keys row -- was special-cased by teaching the
//! layer the row's height, and long press stayed unconditional, so holding a
//! finger on a strip of buttons began selecting terminal text behind them. A
//! second widget would have meant a second special case and a third would have
//! meant a third.
//!
//! Instead the GUI publishes the regions that claim gestures, and the touch
//! layer routes on that list. A region says where it is, what it claims, and
//! how it ranks against an overlapping region; anything it does not claim falls
//! through to the terminal.
//!
//! Positions are deliberately *not* published. A region declares an anchor edge
//! and its extent, and the backend places it against the surface it owns,
//! because the GUI's own window size is briefly the size it would *like* --
//! large enough for a remote pane's row and column count -- rather than the size
//! touch coordinates arrive in. Placing the extra-keys row from the GUI's
//! `dimensions` produced a top edge of 3828px on a 2235px surface, and the row
//! silently ignored every drag.
//!
//! Taps are not in `GestureClaims`. A tap is already delivered as a synthesised
//! mouse press and release, and the GUI resolves it against the `UIItem` list it
//! built while rendering, which is a hit test against the real geometry rather
//! than against a declared extent. Adding a tap claim here would duplicate that
//! with a coarser answer.

use bitflags::bitflags;

/// Which surface edge a region is fastened to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnchorEdge {
    Top,
    Bottom,
    Left,
    Right,
}

impl AnchorEdge {
    /// True when the region's `thickness` is measured vertically.
    pub fn is_vertical(self) -> bool {
        matches!(self, Self::Top | Self::Bottom)
    }
}

// What a region wants to be told about.
//
// A region that claims neither drag axis still suppresses long press if it
// omits `LONG_PRESS`, which is the whole point for a row of buttons.
bitflags! {
    #[derive(Default)]
    pub struct GestureClaims: u8 {
        /// Sideways drags belong to this region.
        const DRAG_HORIZONTAL = 1<<0;
        /// Vertical drags belong to this region.
        const DRAG_VERTICAL = 1<<1;
        /// The region handles long press itself. Omitting this suppresses the
        /// long press rather than passing it to the terminal, because a region
        /// is drawn *over* the terminal: a selection starting underneath a
        /// button is never what the user meant.
        const LONG_PRESS = 1<<2;
    }
}

impl GestureClaims {
    pub fn claims_drag(self) -> bool {
        self.intersects(Self::DRAG_HORIZONTAL | Self::DRAG_VERTICAL)
    }
}

/// Which widget a region belongs to, so that the GUI can tell its own regions
/// apart when a drag comes back to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GestureRegionId {
    /// The extra-keys row along the bottom of the window.
    KeyRow,
    /// The host sidebar, whether overlaid or pinned.
    Sidebar,
    /// The dimmed area outside an open overlay sidebar. It exists so that a
    /// drag there does not scroll the terminal underneath the dimming.
    SidebarScrim,
}

/// A region of the surface that claims some gestures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GestureRegion {
    pub id: GestureRegionId,
    pub anchor: AnchorEdge,
    /// Extent along the anchored axis, in pixels: a height for `Top`/`Bottom`
    /// and a width for `Left`/`Right`. Larger than the surface is fine and is
    /// how a full-surface region is expressed; the backend clamps it.
    pub thickness: f32,
    /// Inset from the start of the cross axis: from the left edge for a
    /// `Top`/`Bottom` anchor, from the top edge for `Left`/`Right`.
    pub cross_start: f32,
    /// Inset from the end of the cross axis. This is what lets the sidebar say
    /// that it begins below the tab bar and stops above the key row.
    pub cross_end: f32,
    /// Higher wins where regions overlap. An open drawer outranks the scrim
    /// behind it, and both outrank the key row it covers.
    pub priority: i32,
    pub claims: GestureClaims,
}

impl GestureRegion {
    /// A region spanning the whole of its anchored edge.
    pub fn edge(id: GestureRegionId, anchor: AnchorEdge, thickness: f32) -> Self {
        Self {
            id,
            anchor,
            thickness,
            cross_start: 0.,
            cross_end: 0.,
            priority: 0,
            claims: GestureClaims::empty(),
        }
    }

    /// A region covering the entire surface. `thickness` is left at infinity
    /// and clamped by the backend, so this needs no knowledge of the surface.
    pub fn whole_surface(id: GestureRegionId) -> Self {
        Self::edge(id, AnchorEdge::Left, f32::INFINITY)
    }

    pub fn claims(mut self, claims: GestureClaims) -> Self {
        self.claims = claims;
        self
    }

    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    pub fn cross_insets(mut self, start: f32, end: f32) -> Self {
        self.cross_start = start;
        self.cross_end = end;
        self
    }

    /// True when a point in surface coordinates falls inside this region, given
    /// the size of the surface.
    pub fn contains(&self, x: f64, y: f64, surface_width: f64, surface_height: f64) -> bool {
        if surface_width <= 0. || surface_height <= 0. || !(self.thickness > 0.) {
            return false;
        }

        let (along, along_extent, cross, cross_extent) = match self.anchor {
            AnchorEdge::Top | AnchorEdge::Bottom => (y, surface_height, x, surface_width),
            AnchorEdge::Left | AnchorEdge::Right => (x, surface_width, y, surface_height),
        };

        let thickness = (self.thickness as f64).min(along_extent);
        let in_along = match self.anchor {
            AnchorEdge::Top | AnchorEdge::Left => along < thickness,
            AnchorEdge::Bottom | AnchorEdge::Right => along >= along_extent - thickness,
        };
        if !in_along {
            return false;
        }

        let cross_min = self.cross_start as f64;
        let cross_max = cross_extent - self.cross_end as f64;
        cross >= cross_min && cross < cross_max
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const W: f64 = 1080.;
    const H: f64 = 2235.;

    fn key_row() -> GestureRegion {
        GestureRegion::edge(GestureRegionId::KeyRow, AnchorEdge::Bottom, 132.)
    }

    #[test]
    fn bottom_anchored_region_tracks_the_bottom_edge() {
        let row = key_row();
        // Inside the band.
        assert!(row.contains(10., H - 1., W, H));
        assert!(row.contains(10., H - 132., W, H));
        // One pixel above it is the terminal.
        assert!(!row.contains(10., H - 133., W, H));
        // And nothing lands in it near the top of the screen. This is the case
        // that regressed when the row was placed from the GUI's window size,
        // which was 3960px against a 2235px surface.
        assert!(!row.contains(10., 0., W, H));
    }

    #[test]
    fn a_region_taller_than_the_surface_is_clamped() {
        let scrim = GestureRegion::whole_surface(GestureRegionId::SidebarScrim);
        assert!(scrim.contains(0., 0., W, H));
        assert!(scrim.contains(W - 1., H - 1., W, H));
        assert!(!scrim.contains(W, 0., W, H));
    }

    #[test]
    fn cross_insets_bound_the_other_axis() {
        // A left-anchored drawer that starts below a 60px tab bar and stops
        // above a 132px key row.
        let sidebar = GestureRegion::edge(GestureRegionId::Sidebar, AnchorEdge::Left, 800.)
            .cross_insets(60., 132.);
        assert!(sidebar.contains(10., 60., W, H));
        assert!(sidebar.contains(799., H - 133., W, H));
        // Above the top inset, and inside the key row, both fall through.
        assert!(!sidebar.contains(10., 59., W, H));
        assert!(!sidebar.contains(10., H - 132., W, H));
        // And past its width.
        assert!(!sidebar.contains(800., 100., W, H));
    }

    #[test]
    fn a_zero_extent_region_contains_nothing() {
        let row = GestureRegion::edge(GestureRegionId::KeyRow, AnchorEdge::Bottom, 0.);
        assert!(!row.contains(10., H - 1., W, H));
        // Nor does any region before the surface size is known.
        assert!(!key_row().contains(0., 0., 0., 0.));
    }

    #[test]
    fn claims_drag_covers_either_axis() {
        assert!(GestureClaims::DRAG_HORIZONTAL.claims_drag());
        assert!(GestureClaims::DRAG_VERTICAL.claims_drag());
        assert!(!GestureClaims::LONG_PRESS.claims_drag());
        assert!(!GestureClaims::empty().claims_drag());
    }
}
