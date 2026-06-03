//! Pure selection model for a virtualized grid.
//!
//! Holds no DOM — the unit-tested heart of selection.
//! [`VirtualTableView`](crate::VirtualTableView) owns a [`GridSelection`],
//! drives it from the keyboard, and reflects it onto the materialized window
//! as `data-selected` attributes that CSS targets (same contract shape as the
//! `data-active-*` cursor).
//!
//! **Configurable** via [`SelectionMode`]: off by default; opt into `Cell`
//! (rectangular cell ranges) or `Row` (whole-row ranges). Selection is the
//! *union* of an optional rectangular range (a shift-extend anchor → the
//! cursor head), a set of individually toggled cells/rows, and a select-all
//! flag — mirroring a spreadsheet's mixed selection.

use std::collections::HashSet;

/// How the table selects, or whether it does at all. `None` is the default —
/// selection is opt-in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionMode {
    /// Selection disabled. `Shift`+arrows / `Space` / `Ctrl-A` do nothing.
    #[default]
    None,
    /// Cell selection — a rectangular `(row, col)` range plus toggled cells.
    Cell,
    /// Row selection — whole rows; the column is ignored.
    Row,
}

/// A logical selection over a grid. Pure — holds no DOM.
///
/// The selected set is the union of: a rectangular range (when a shift-extend
/// `anchor` is set, from `anchor` to `head`), a set of `toggled` cells/rows,
/// and `all`. [`is_selected`](Self::is_selected) answers per cell, mode-aware.
#[derive(Clone, Debug, Default)]
pub struct GridSelection {
    mode: SelectionMode,
    /// Shift-extend anchor `(row, col)`; `None` when no range is active.
    anchor: Option<(usize, usize)>,
    /// The range head — the cursor's position as the range was extended.
    head: (usize, usize),
    /// Individually toggled cells. In `Row` mode the column is normalized to 0.
    toggled: HashSet<(usize, usize)>,
    /// Whole-grid selection (`Ctrl-A`).
    all: bool,
}

impl GridSelection {
    pub fn new(mode: SelectionMode) -> Self {
        Self {
            mode,
            ..Default::default()
        }
    }

    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    /// Change the mode. Changing it clears any active selection — a Cell
    /// rectangle means nothing in Row mode and vice versa.
    pub fn set_mode(&mut self, mode: SelectionMode) {
        self.mode = mode;
        self.clear();
    }

    /// Is anything selected? (Always false when the mode is `None`.)
    pub fn is_active(&self) -> bool {
        self.mode != SelectionMode::None
            && (self.all || !self.toggled.is_empty() || self.anchor.is_some())
    }

    /// Normalize a cell to its selection key — `Row` mode ignores the column.
    fn key(&self, row: usize, col: usize) -> (usize, usize) {
        match self.mode {
            SelectionMode::Row => (row, 0),
            _ => (row, col),
        }
    }

    /// Is `(row, col)` selected? Mode-aware union of range, toggled, and all.
    pub fn is_selected(&self, row: usize, col: usize) -> bool {
        if self.mode == SelectionMode::None {
            return false;
        }
        if self.all {
            return true;
        }
        if self.toggled.contains(&self.key(row, col)) {
            return true;
        }
        if let Some((ar, ac)) = self.anchor {
            let (hr, hc) = self.head;
            let in_rows = row >= ar.min(hr) && row <= ar.max(hr);
            match self.mode {
                SelectionMode::Row => in_rows,
                SelectionMode::Cell => in_rows && col >= ac.min(hc) && col <= ac.max(hc),
                SelectionMode::None => false,
            }
        } else {
            false
        }
    }

    /// Extend the rectangular range: anchor at `from` (the cursor before the
    /// move) if no range is active yet, head at `to` (the cursor after).
    /// No-op when the mode is `None`.
    pub fn extend(&mut self, from: (usize, usize), to: (usize, usize)) {
        if self.mode == SelectionMode::None {
            return;
        }
        if self.anchor.is_none() {
            self.anchor = Some(from);
        }
        self.head = to;
        self.all = false;
    }

    /// Toggle `(row, col)` (or its row, in `Row` mode) in the toggled set.
    /// No-op when the mode is `None`.
    pub fn toggle(&mut self, row: usize, col: usize) {
        if self.mode == SelectionMode::None {
            return;
        }
        let k = self.key(row, col);
        if !self.toggled.remove(&k) {
            self.toggled.insert(k);
        }
        self.all = false;
    }

    /// Commit the active rectangular range into the sticky toggled set as a
    /// unit, then collapse the range — so a `Space` press over a live
    /// `Shift`-range turns the whole rectangle into a persistent selection
    /// (Shift-select, `Space`, move, Shift-select, `Space`, … builds multiple
    /// ranges). If every cell of the range is already toggled it is *removed*
    /// (a true toggle); otherwise the whole range is *added*. Returns `false`
    /// (and does nothing) when no range is active, so the caller can fall back
    /// to toggling the single cursor cell. No-op when the mode is `None`.
    pub fn toggle_range(&mut self) -> bool {
        if self.mode == SelectionMode::None {
            return false;
        }
        let Some((ar, ac)) = self.anchor else {
            return false;
        };
        let (hr, hc) = self.head;
        let (r0, r1) = (ar.min(hr), ar.max(hr));
        let (c0, c1) = match self.mode {
            SelectionMode::Row => (0, 0),
            _ => (ac.min(hc), ac.max(hc)),
        };
        let keys: Vec<(usize, usize)> = (r0..=r1)
            .flat_map(|r| (c0..=c1).map(move |c| (r, c)))
            .map(|(r, c)| self.key(r, c))
            .collect();
        let all_present = keys.iter().all(|k| self.toggled.contains(k));
        if all_present {
            for k in &keys {
                self.toggled.remove(k);
            }
        } else {
            self.toggled.extend(keys);
        }
        self.anchor = None;
        self.all = false;
        true
    }

    /// Select the whole grid (`Ctrl-A`). No-op when the mode is `None`.
    pub fn select_all(&mut self) {
        if self.mode == SelectionMode::None {
            return;
        }
        self.all = true;
        self.anchor = None;
        self.toggled.clear();
    }

    /// Clear everything (`Esc`).
    pub fn clear(&mut self) {
        self.anchor = None;
        self.toggled.clear();
        self.all = false;
    }

    /// Collapse the *transient* selections — an in-progress rectangular range
    /// and a `Ctrl-A` select-all — as a plain (unmodified) cursor move does,
    /// matching every spreadsheet/grid. The explicitly `Space`-toggled set is
    /// kept: it's the keyboard "add to selection" gesture (the TUI stand-in for
    /// `Ctrl`+click) and only `Esc` ([`clear`](Self::clear)) drops it.
    pub fn collapse_transient(&mut self) {
        self.anchor = None;
        self.all = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_mode_selects_nothing() {
        let mut s = GridSelection::new(SelectionMode::None);
        s.extend((0, 0), (5, 5));
        s.toggle(1, 1);
        s.select_all();
        assert!(!s.is_active());
        assert!(!s.is_selected(1, 1));
    }

    #[test]
    fn cell_range_is_rectangular() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.extend((1, 1), (3, 2)); // rows 1..3, cols 1..2
        assert!(s.is_selected(1, 1));
        assert!(s.is_selected(3, 2));
        assert!(s.is_selected(2, 1));
        assert!(!s.is_selected(0, 1), "row above range");
        assert!(!s.is_selected(2, 0), "col left of range");
        assert!(!s.is_selected(4, 2), "row below range");
        // Backwards extend normalizes (anchor may be > head).
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.extend((3, 2), (1, 1));
        assert!(s.is_selected(2, 1));
    }

    #[test]
    fn row_mode_selects_whole_rows_ignoring_column() {
        let mut s = GridSelection::new(SelectionMode::Row);
        s.extend((1, 0), (3, 0)); // rows 1..3, any column
        assert!(s.is_selected(1, 0));
        assert!(s.is_selected(2, 99), "any column in a selected row");
        assert!(!s.is_selected(0, 0));
        assert!(!s.is_selected(4, 0));
    }

    #[test]
    fn toggle_adds_and_removes() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.toggle(2, 2);
        assert!(s.is_selected(2, 2));
        s.toggle(2, 2);
        assert!(!s.is_selected(2, 2));
        assert!(!s.is_active());
    }

    #[test]
    fn row_toggle_keys_on_row_only() {
        let mut s = GridSelection::new(SelectionMode::Row);
        s.toggle(2, 5);
        assert!(s.is_selected(2, 0), "toggling a row selects every column");
        assert!(s.is_selected(2, 9));
    }

    #[test]
    fn union_of_range_and_toggled() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.extend((0, 0), (1, 1));
        s.toggle(5, 5);
        assert!(s.is_selected(0, 0), "range");
        assert!(s.is_selected(5, 5), "toggled");
    }

    #[test]
    fn select_all_and_clear() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.select_all();
        assert!(s.is_selected(99, 99));
        s.clear();
        assert!(!s.is_active());
        assert!(!s.is_selected(0, 0));
    }

    #[test]
    fn collapse_range_keeps_toggled() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.toggle(4, 4);
        s.extend((0, 0), (2, 2));
        s.collapse_transient();
        assert!(!s.is_selected(1, 1), "range dropped");
        assert!(s.is_selected(4, 4), "toggled kept");
    }

    #[test]
    fn collapse_clears_select_all_keeps_toggled() {
        // A plain cursor move collapses the *transient* selections — an
        // in-progress range AND a Ctrl-A select-all — but the explicitly
        // Space-toggled set survives until Esc (the keyboard "add to selection"
        // gesture; collapsing it would make it unusable).
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.toggle(4, 4);
        s.select_all(); // Ctrl-A (note: this clears toggled, like a spreadsheet)
        s.collapse_transient();
        assert!(!s.is_selected(0, 0), "select-all collapsed on a plain move");
        // Re-toggle then collapse: the toggle survives a plain move.
        s.toggle(7, 1);
        s.collapse_transient();
        assert!(s.is_selected(7, 1), "explicit toggles survive a plain move");
        assert!(!s.is_selected(0, 0), "but select-all stays collapsed");
    }

    #[test]
    fn toggle_range_commits_the_rectangle() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.extend((1, 1), (2, 2)); // live range rows 1..2, cols 1..2
        assert!(s.toggle_range(), "there was a range to commit");
        // The range is collapsed, but its cells are now in the sticky set:
        assert!(s.anchor.is_none(), "range collapsed after commit");
        assert!(s.is_selected(1, 1));
        assert!(s.is_selected(2, 2));
        assert!(s.is_selected(1, 2));
        // …so a plain move keeps them (they're toggled now, not a range).
        s.collapse_transient();
        assert!(s.is_selected(1, 1), "committed range survives a plain move");
        // Re-selecting the same rect and toggling again removes it as a unit.
        s.extend((1, 1), (2, 2));
        s.toggle_range();
        assert!(
            !s.is_selected(2, 2),
            "toggling an already-selected range clears it"
        );
    }

    #[test]
    fn toggle_range_builds_multiple_ranges() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.extend((0, 0), (1, 0)); // range A
        s.toggle_range();
        s.extend((5, 1), (6, 2)); // range B (after moving + re-extending)
        s.toggle_range();
        assert!(s.is_selected(0, 0), "range A held");
        assert!(s.is_selected(1, 0), "range A held");
        assert!(s.is_selected(5, 1), "range B held");
        assert!(s.is_selected(6, 2), "range B held");
        assert!(!s.is_selected(3, 0), "gap between ranges is unselected");
    }

    #[test]
    fn toggle_range_without_a_range_is_a_noop() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        assert!(
            !s.toggle_range(),
            "no range → caller falls back to single-cell"
        );
    }

    #[test]
    fn toggle_range_in_row_mode_commits_whole_rows() {
        let mut s = GridSelection::new(SelectionMode::Row);
        s.extend((2, 0), (4, 0));
        s.toggle_range();
        assert!(s.is_selected(2, 9), "row 2, any column");
        assert!(s.is_selected(4, 0));
        assert!(!s.is_selected(5, 0));
    }

    #[test]
    fn set_mode_clears_selection() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.select_all();
        s.set_mode(SelectionMode::Row);
        assert!(!s.is_active());
        assert_eq!(s.mode(), SelectionMode::Row);
    }
}
