use crate::pane::CloseReason;
use crate::{Mux, MuxNotification, Tab, TabId};
use config::GuiPosition;
use std::sync::Arc;

static WIN_ID: ::std::sync::atomic::AtomicUsize = ::std::sync::atomic::AtomicUsize::new(0);
pub type WindowId = usize;

/// The index at which the tab currently at `from_idx` must be re-inserted
/// in order to end up immediately after the tab at `after_idx`, or first in
/// the window if that is `None`.
///
/// The tab is lifted out before being put back, which shifts everything to
/// its right down by one: landing after a tab to our right therefore means
/// taking that tab's index, while landing after a tab to our left means
/// taking the index just past it.
fn idx_following(from_idx: usize, after_idx: Option<usize>) -> usize {
    match after_idx {
        Some(idx) if idx > from_idx => idx,
        Some(idx) => idx + 1,
        None => 0,
    }
}

/// The inverse of `idx_following`: given a move of the tab at `from_idx` to
/// `to_idx`, the index *in the list as it stands now* of the tab that the
/// moved tab will end up following, or `None` if it will be first.
fn idx_preceding_after_move(from_idx: usize, to_idx: usize) -> Option<usize> {
    if to_idx == 0 {
        None
    } else if to_idx > from_idx {
        // Everything between the two shuffles left to fill the gap, so the
        // tab now at `to_idx` ends up on our left.
        Some(to_idx)
    } else {
        Some(to_idx - 1)
    }
}

pub struct Window {
    id: WindowId,
    tabs: Vec<Arc<Tab>>,
    active: usize,
    last_active: Option<TabId>,
    workspace: String,
    title: String,
    initial_position: Option<GuiPosition>,
}

impl Window {
    pub fn new(workspace: Option<String>, initial_position: Option<GuiPosition>) -> Self {
        Self {
            id: WIN_ID.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed),
            tabs: vec![],
            active: 0,
            last_active: None,
            title: String::new(),
            workspace: workspace.unwrap_or_else(|| Mux::get().active_workspace()),
            initial_position,
        }
    }

    pub fn get_initial_position(&self) -> &Option<GuiPosition> {
        &self.initial_position
    }

    pub fn get_workspace(&self) -> &str {
        &self.workspace
    }

    pub fn set_title(&mut self, title: &str) {
        if self.title != title {
            self.title = title.to_string();
            Mux::try_get().map(|mux| {
                mux.notify(MuxNotification::WindowTitleChanged {
                    window_id: self.id,
                    title: title.to_string(),
                })
            });
        }
    }

    pub fn get_title(&self) -> &str {
        &self.title
    }

    pub fn set_workspace(&mut self, workspace: &str) {
        if workspace == self.workspace {
            return;
        }
        self.workspace = workspace.to_string();
        Mux::get().notify(MuxNotification::WindowWorkspaceChanged(self.id));
    }

    pub fn window_id(&self) -> WindowId {
        self.id
    }

    fn check_that_tab_isnt_already_in_window(&self, tab: &Arc<Tab>) {
        for t in &self.tabs {
            assert_ne!(t.tab_id(), tab.tab_id(), "tab already added to this window");
        }
    }

    fn invalidate(&self) {
        let mux = Mux::get();
        mux.notify(MuxNotification::WindowInvalidated(self.id));
    }

    pub fn insert(&mut self, index: usize, tab: &Arc<Tab>) {
        self.check_that_tab_isnt_already_in_window(tab);
        self.tabs.insert(index, Arc::clone(tab));
        self.invalidate();
    }

    pub fn push(&mut self, tab: &Arc<Tab>) {
        self.check_that_tab_isnt_already_in_window(tab);
        self.tabs.push(Arc::clone(tab));
        self.invalidate();
    }

    /// Move the tab at `from_idx` so that it sits at `to_idx`, sliding the
    /// tabs in between along to make room. Whichever tab was active stays
    /// active; only its index changes, so this deliberately assigns
    /// `self.active` directly rather than going through
    /// `set_active_without_saving`, which would tell the active pane it had
    /// lost the focus.
    ///
    /// Returns false, having done nothing, if either index is out of range
    /// or the tab is already where it was asked to go.
    pub fn move_tab(&mut self, from_idx: usize, to_idx: usize) -> bool {
        let len = self.tabs.len();
        if from_idx >= len || to_idx >= len || from_idx == to_idx {
            return false;
        }

        let active = self.get_active().map(|tab| tab.tab_id());
        let tab = self.tabs.remove(from_idx);
        self.tabs.insert(to_idx, tab);
        if let Some(idx) = active.and_then(|id| self.idx_by_id(id)) {
            self.active = idx;
        }
        self.invalidate();
        true
    }

    /// Move `tab_id` so that it immediately follows `after_tab_id`, or to
    /// the front of the window if that is `None`.
    /// Returns false if either tab isn't in this window, or if the tab is
    /// already in that position.
    pub fn move_tab_after(&mut self, tab_id: TabId, after_tab_id: Option<TabId>) -> bool {
        let from_idx = match self.idx_by_id(tab_id) {
            Some(idx) => idx,
            None => return false,
        };
        if after_tab_id == Some(tab_id) {
            return false;
        }
        let after_idx = match after_tab_id {
            Some(after_tab_id) => match self.idx_by_id(after_tab_id) {
                Some(idx) => Some(idx),
                None => return false,
            },
            None => None,
        };
        self.move_tab(from_idx, idx_following(from_idx, after_idx))
    }

    /// Which tab would the tab at `from_idx` come to sit after, were it moved
    /// to `to_idx`? `None` means it would be the first tab in the window.
    /// This is the form in which a move is described to a remote mux, whose
    /// indices need not line up with ours; see codec::MoveTab.
    pub fn tab_preceding_after_move(&self, from_idx: usize, to_idx: usize) -> Option<TabId> {
        let idx = idx_preceding_after_move(from_idx, to_idx)?;
        self.tabs.get(idx).map(|tab| tab.tab_id())
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn get_by_idx(&self, idx: usize) -> Option<&Arc<Tab>> {
        self.tabs.get(idx)
    }

    pub fn can_close_without_prompting(&self) -> bool {
        for tab in &self.tabs {
            if !tab.can_close_without_prompting(CloseReason::Window) {
                return false;
            }
        }
        true
    }

    pub fn idx_by_id(&self, id: TabId) -> Option<usize> {
        for (idx, t) in self.tabs.iter().enumerate() {
            if t.tab_id() == id {
                return Some(idx);
            }
        }
        None
    }

    fn fixup_active_tab_after_removal(&mut self, active: Option<Arc<Tab>>) {
        let len = self.tabs.len();
        if let Some(active) = active {
            for (idx, tab) in self.tabs.iter().enumerate() {
                if tab.tab_id() == active.tab_id() {
                    self.set_active_without_saving(idx);
                    return;
                }
            }
        }

        if len > 0 && self.active >= len {
            self.set_active_without_saving(len - 1);
        } else {
            self.invalidate();
        }
    }

    pub fn remove_by_idx(&mut self, idx: usize) -> Arc<Tab> {
        self.invalidate();
        let active = self.get_active().map(Arc::clone);
        self.do_remove_idx(idx, active)
    }

    pub fn remove_by_id(&mut self, id: TabId) {
        let active = self.get_active().map(Arc::clone);
        if let Some(idx) = self.idx_by_id(id) {
            self.do_remove_idx(idx, active);
        }
    }

    fn do_remove_idx(&mut self, idx: usize, active: Option<Arc<Tab>>) -> Arc<Tab> {
        if let (Some(active), Some(removing)) = (&active, self.tabs.get(idx)) {
            if active.tab_id() == removing.tab_id()
                && config::configuration().switch_to_last_active_tab_when_closing_tab
            {
                // If we are removing the active tab, switch back to
                // the previously active tab
                if let Some(last_active) = self.get_last_active_idx() {
                    self.set_active_without_saving(last_active);
                }
            }
        }
        let tab = self.tabs.remove(idx);
        self.fixup_active_tab_after_removal(active);
        tab
    }

    pub fn get_active(&self) -> Option<&Arc<Tab>> {
        self.get_by_idx(self.active)
    }

    #[inline]
    pub fn get_active_idx(&self) -> usize {
        self.active
    }

    pub fn save_last_active(&mut self) {
        self.last_active = self.get_by_idx(self.active).map(|tab| tab.tab_id());
    }

    #[inline]
    pub fn get_last_active_idx(&self) -> Option<usize> {
        if let Some(tab_id) = self.last_active {
            self.idx_by_id(tab_id)
        } else {
            None
        }
    }

    /// If `idx` is different from the current active tab,
    /// save the current tabid and then make `idx` the active
    /// tab position.
    pub fn save_and_then_set_active(&mut self, idx: usize) {
        if idx == self.get_active_idx() {
            return;
        }
        self.save_last_active();
        self.set_active_without_saving(idx);
    }

    /// Make `idx` the active tab position.
    /// The saved tab id is not changed.
    pub fn set_active_without_saving(&mut self, idx: usize) {
        assert!(idx < self.tabs.len());
        if self.active != idx {
            if let Some(tab) = self.tabs.get(self.active) {
                if let Some(pane) = tab.get_active_pane() {
                    pane.focus_changed(false);
                }
            }
        }
        self.active = idx;
        self.invalidate();
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Tab>> {
        self.tabs.iter()
    }

    pub fn prune_dead_tabs(&mut self, live_tab_ids: &[TabId]) {
        let mut invalidated = false;
        let dead: Vec<TabId> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                if tab.prune_dead_panes() {
                    invalidated = true;
                }
                if tab.is_dead() {
                    Some(tab.tab_id())
                } else {
                    None
                }
            })
            .collect();

        for tab_id in dead {
            log::trace!("Window::prune_dead_tabs: tab_id {} is dead", tab_id);
            self.remove_by_id(tab_id);
            invalidated = true;
        }

        let dead: Vec<TabId> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                if live_tab_ids
                    .iter()
                    .find(|&&id| id == tab.tab_id())
                    .is_none()
                {
                    Some(tab.tab_id())
                } else {
                    None
                }
            })
            .collect();
        for tab_id in dead {
            log::trace!("Window::prune_dead_tabs: (live) tab_id {} is dead", tab_id);
            self.remove_by_id(tab_id);
        }

        if invalidated {
            self.invalidate();
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Re-inserting a tab that has been lifted out of the list is one of
    /// those places where being off by one is easy and quiet, so pin the
    /// arithmetic down here rather than discovering it as tabs that land
    /// one place to the left of where they were dropped.
    #[test]
    fn idx_following_accounts_for_the_lifted_tab() {
        // Tabs [a b c d]; moving `a` (0) after each of the others.
        assert_eq!(idx_following(0, Some(1)), 1); // [b a c d]
        assert_eq!(idx_following(0, Some(3)), 3); // [b c d a]

        // Moving `d` (3) after each of the others.
        assert_eq!(idx_following(3, Some(0)), 1); // [a d b c]
        assert_eq!(idx_following(3, Some(2)), 3); // unchanged

        // Moving to the front is just index 0, wherever we started.
        assert_eq!(idx_following(0, None), 0);
        assert_eq!(idx_following(2, None), 0);

        // Staying put: `b` (1) is already immediately after `a` (0).
        assert_eq!(idx_following(1, Some(0)), 1);
    }

    /// Describing a move by naming a neighbour is only useful if the two
    /// directions agree, so check that a move to `to_idx` and the neighbour
    /// we report for it are the same move.
    #[test]
    fn idx_following_and_idx_preceding_are_inverses() {
        for len in 1..6 {
            for from_idx in 0..len {
                for to_idx in 0..len {
                    let after_idx = idx_preceding_after_move(from_idx, to_idx);
                    assert_eq!(
                        idx_following(from_idx, after_idx),
                        to_idx,
                        "moving {from_idx} -> {to_idx} of {len} was described \
                         as following {after_idx:?}"
                    );
                }
            }
        }
    }

    /// And the description has to survive the trip: what the mover means by
    /// "after tab X" must be what the other end does with it, even though
    /// they are working from different lists.
    #[test]
    fn a_described_move_reproduces_the_same_order() {
        // Ours: [a b c d]. Theirs has an extra tab we haven't heard about,
        // and lacks one of ours, so the indices don't line up at all.
        let ours = ["a", "b", "c", "d"];
        let theirs = ["z", "a", "b", "d"];

        for from_idx in 0..ours.len() {
            for to_idx in 0..ours.len() {
                let mut expected: Vec<&str> = ours.to_vec();
                let moved = expected.remove(from_idx);
                expected.insert(
                    idx_following(from_idx, idx_preceding_after_move(from_idx, to_idx)),
                    moved,
                );

                // What we'd put on the wire: the name of the tab we now follow.
                let after = idx_preceding_after_move(from_idx, to_idx).map(|idx| ours[idx]);

                // What the other end does with it.
                let mut got: Vec<&str> = theirs.to_vec();
                let their_from = match got.iter().position(|t| *t == moved) {
                    Some(idx) => idx,
                    // Not a tab they have; nothing to check.
                    None => continue,
                };
                let their_after = match after {
                    Some(after) => match got.iter().position(|t| *t == after) {
                        Some(idx) => Some(idx),
                        // They don't have the tab we named, so they leave
                        // things alone rather than guess.
                        None => continue,
                    },
                    None => None,
                };
                let tab = got.remove(their_from);
                got.insert(idx_following(their_from, their_after), tab);

                // The tabs they share with us must end up in the order we
                // put them in, whatever else is interleaved.
                let ours_only: Vec<&str> = got
                    .iter()
                    .copied()
                    .filter(|t| expected.contains(t))
                    .collect();
                let theirs_only: Vec<&str> = expected
                    .iter()
                    .copied()
                    .filter(|t| got.contains(t))
                    .collect();
                assert_eq!(
                    ours_only, theirs_only,
                    "moving {moved} ({from_idx} -> {to_idx}) after {after:?}: \
                     we have {expected:?}, they have {got:?}"
                );
            }
        }
    }
}
