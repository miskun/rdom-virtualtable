//! Pure selection model for a virtualized grid.
//!
//! Holds no DOM — the unit-tested heart of selection.
//! [`VirtualTableView`](crate::VirtualTableView) owns a [`GridSelection`],
//! drives it from the keyboard/mouse, and reflects it onto the materialized
//! window as `data-selected` attributes that CSS targets (same contract shape
//! as the `data-active-*` cursor).
//!
//! **Identity, not position (`SPEC_DATA_SOURCE.md` §8).** The *durable*
//! selection — the individually toggled cells and the `Ctrl-A` predicate — is
//! keyed by [`RowKey`], so a selected row stays selected as it scrolls,
//! re-sorts, or is live-updated under it. Only the *transient* shift-extend
//! rectangle is positional (anchor → head in the current view); it collapses on
//! a plain cursor move, so it never needs to survive a re-sort.
//!
//! Because identity lives in the row keys, [`is_selected`](GridSelection::is_selected)
//! takes the row's [`RowKey`] alongside its current index: the index answers the
//! transient range, the key answers the durable set + predicate. The View
//! resolves index → key from its model and exposes the positional
//! [`is_cell_selected`](crate::VirtualTableView::is_cell_selected) helper.
//!
//! **Configurable** via [`SelectionMode`]: off by default; opt into `Cell`
//! (rectangular cell ranges) or `Row` (whole-row ranges). Selection is the
//! *union* of the transient rectangle, the durable toggled set, and the
//! `all`/`except` predicate — mirroring a spreadsheet's mixed selection.

use std::collections::HashSet;

use crate::data::RowKey;

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

/// A durable selection key: a row's identity plus a column. In `Row` mode the
/// column is normalized to 0 (the whole row is one unit).
type CellKey = (RowKey, usize);

/// A logical selection over a grid. Pure — holds no DOM.
///
/// The selected set is the union of: a *transient* rectangular range (when a
/// shift-extend `anchor` is set, from `anchor` to `head`, positional in the
/// current view), a durable set of identity-keyed `explicit` cells, and the
/// `all`/`except` predicate (`Ctrl-A`). [`is_selected`](Self::is_selected)
/// answers per cell, mode-aware.
#[derive(Clone, Debug, Default)]
pub struct GridSelection {
    mode: SelectionMode,
    /// Shift-extend anchor `(row, col)` — *positional*, in the current view;
    /// `None` when no range is active.
    anchor: Option<(usize, usize)>,
    /// The range head — the cursor's position as the range was extended.
    head: (usize, usize),
    /// Individually toggled cells, keyed by **identity** so they survive
    /// scroll / re-sort / live updates. In `Row` mode the column is 0.
    explicit: HashSet<CellKey>,
    /// Predicate select-all (`Ctrl-A`): everything matching the current view,
    /// minus `except`. The only sane "select all" over a windowed 100k set.
    all: bool,
    /// Cells deselected while the `all` predicate is active.
    except: HashSet<CellKey>,
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
            && (self.all || !self.explicit.is_empty() || self.anchor.is_some())
    }

    /// Is the `all` predicate (`Ctrl-A`) active? Bulk actions consult this plus
    /// [`except`](Self::except) and ask the source to enumerate server-side
    /// (`SPEC_DATA_SOURCE.md` §8).
    pub fn is_all(&self) -> bool {
        self.all
    }

    /// The durable, explicitly toggled cells (identity-keyed). Empty in
    /// predicate (`all`) mode unless cells were toggled outside it.
    pub fn explicit(&self) -> &HashSet<CellKey> {
        &self.explicit
    }

    /// The cells excepted from the `all` predicate (deselected under `Ctrl-A`).
    pub fn except(&self) -> &HashSet<CellKey> {
        &self.except
    }

    /// Normalize a key to its selection identity — `Row` mode ignores the
    /// column.
    fn id_key(&self, key: &RowKey, col: usize) -> CellKey {
        match self.mode {
            SelectionMode::Row => (key.clone(), 0),
            _ => (key.clone(), col),
        }
    }

    /// Is the cell at view index `row` (column `col`), whose row identity is
    /// `key`, selected? The index answers the transient range; the key answers
    /// the durable set + predicate. Mode-aware.
    pub fn is_selected(&self, row: usize, col: usize, key: &RowKey) -> bool {
        if self.mode == SelectionMode::None {
            return false;
        }
        let k = self.id_key(key, col);
        if self.all && !self.except.contains(&k) {
            return true;
        }
        if self.explicit.contains(&k) {
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

    /// Extend the transient rectangular range: anchor at `from` (the cursor
    /// before the move) if no range is active yet, head at `to` (the cursor
    /// after). Starting a fresh range gesture drops a live `Ctrl-A` predicate.
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
        self.except.clear();
    }

    /// Toggle the cell `(key, col)` (or its row, in `Row` mode) in the durable
    /// selection. Under the `all` predicate the toggle flips membership via the
    /// `except` set (so deselecting one cell of a select-all keeps the rest);
    /// otherwise it flips the `explicit` set. No-op when the mode is `None`.
    pub fn toggle(&mut self, key: RowKey, col: usize) {
        if self.mode == SelectionMode::None {
            return;
        }
        let k = self.id_key(&key, col);
        if self.all {
            if !self.except.remove(&k) {
                self.except.insert(k);
            }
        } else if !self.explicit.remove(&k) {
            self.explicit.insert(k);
        }
    }

    /// Commit the active rectangular range into the durable `explicit` set as a
    /// unit, then collapse the range — so a `Space` press over a live
    /// `Shift`-range turns the whole rectangle into a persistent selection
    /// (Shift-select, `Space`, move, Shift-select, `Space`, … builds multiple
    /// ranges). `key_at` resolves a view row index to its [`RowKey`]; rows it
    /// can't resolve (off the loaded window) are skipped. If every resolvable
    /// cell of the range is already in `explicit` it is *removed* (a true
    /// toggle); otherwise the whole range is *added*. Returns `false` (and does
    /// nothing) when no range is active, so the caller can fall back to toggling
    /// the single cursor cell. No-op when the mode is `None`.
    pub fn toggle_range(&mut self, key_at: impl Fn(usize) -> Option<RowKey>) -> bool {
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
        let mut keys: Vec<CellKey> = Vec::new();
        for r in r0..=r1 {
            if let Some(rk) = key_at(r) {
                for c in c0..=c1 {
                    keys.push(self.id_key(&rk, c));
                }
            }
        }
        let all_present = !keys.is_empty() && keys.iter().all(|k| self.explicit.contains(k));
        if all_present {
            for k in &keys {
                self.explicit.remove(k);
            }
        } else {
            self.explicit.extend(keys);
        }
        self.anchor = None;
        true
    }

    /// Select the whole view (`Ctrl-A`) — the `all` predicate. No-op when the
    /// mode is `None`.
    pub fn select_all(&mut self) {
        if self.mode == SelectionMode::None {
            return;
        }
        self.all = true;
        self.except.clear();
        self.anchor = None;
        self.explicit.clear();
    }

    /// Clear everything (`Esc`).
    pub fn clear(&mut self) {
        self.anchor = None;
        self.explicit.clear();
        self.all = false;
        self.except.clear();
    }

    /// Collapse the *transient* selections — an in-progress rectangular range
    /// and a `Ctrl-A` predicate — as a plain (unmodified) cursor move does,
    /// matching every spreadsheet/grid. The explicitly toggled `explicit` set is
    /// kept: it's the keyboard "add to selection" gesture (the TUI stand-in for
    /// `Ctrl`+click) and only `Esc` ([`clear`](Self::clear)) drops it.
    pub fn collapse_transient(&mut self) {
        self.anchor = None;
        self.all = false;
        self.except.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic row key for a view index — mirrors the in-memory filler's
    /// per-row synthetic key, so tests can speak "the row at index N".
    fn rk(i: usize) -> RowKey {
        RowKey::from(i.to_string())
    }

    #[test]
    fn none_mode_selects_nothing() {
        let mut s = GridSelection::new(SelectionMode::None);
        s.extend((0, 0), (5, 5));
        s.toggle(rk(1), 1);
        s.select_all();
        assert!(!s.is_active());
        assert!(!s.is_selected(1, 1, &rk(1)));
    }

    #[test]
    fn cell_range_is_rectangular() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.extend((1, 1), (3, 2)); // rows 1..3, cols 1..2
        assert!(s.is_selected(1, 1, &rk(1)));
        assert!(s.is_selected(3, 2, &rk(3)));
        assert!(s.is_selected(2, 1, &rk(2)));
        assert!(!s.is_selected(0, 1, &rk(0)), "row above range");
        assert!(!s.is_selected(2, 0, &rk(2)), "col left of range");
        assert!(!s.is_selected(4, 2, &rk(4)), "row below range");
        // Backwards extend normalizes (anchor may be > head).
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.extend((3, 2), (1, 1));
        assert!(s.is_selected(2, 1, &rk(2)));
    }

    #[test]
    fn row_mode_selects_whole_rows_ignoring_column() {
        let mut s = GridSelection::new(SelectionMode::Row);
        s.extend((1, 0), (3, 0)); // rows 1..3, any column
        assert!(s.is_selected(1, 0, &rk(1)));
        assert!(s.is_selected(2, 99, &rk(2)), "any column in a selected row");
        assert!(!s.is_selected(0, 0, &rk(0)));
        assert!(!s.is_selected(4, 0, &rk(4)));
    }

    #[test]
    fn toggle_adds_and_removes() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.toggle(rk(2), 2);
        assert!(s.is_selected(2, 2, &rk(2)));
        s.toggle(rk(2), 2);
        assert!(!s.is_selected(2, 2, &rk(2)));
        assert!(!s.is_active());
    }

    #[test]
    fn explicit_selection_is_identity_keyed_survives_reindex() {
        // The durable toggle is keyed by RowKey, not position: the same key
        // stays selected even if its view index changes under a re-sort, and a
        // *different* key at the old index is not selected.
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.toggle(rk(37), 1);
        assert!(s.is_selected(5, 1, &rk(37)), "key 37 selected at index 5");
        assert!(
            s.is_selected(0, 1, &rk(37)),
            "…and still selected after it moved to index 0"
        );
        assert!(
            !s.is_selected(5, 1, &rk(99)),
            "a different key now at index 5 is not selected"
        );
    }

    #[test]
    fn row_toggle_keys_on_row_only() {
        let mut s = GridSelection::new(SelectionMode::Row);
        s.toggle(rk(2), 5);
        assert!(
            s.is_selected(2, 0, &rk(2)),
            "toggling a row selects every column"
        );
        assert!(s.is_selected(2, 9, &rk(2)));
    }

    #[test]
    fn union_of_range_and_toggled() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.extend((0, 0), (1, 1));
        s.toggle(rk(5), 5);
        assert!(s.is_selected(0, 0, &rk(0)), "range");
        assert!(s.is_selected(5, 5, &rk(5)), "toggled");
    }

    #[test]
    fn select_all_predicate_and_clear() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.select_all();
        assert!(s.is_all());
        assert!(
            s.is_selected(99, 99, &rk(99)),
            "anything matches the predicate"
        );
        s.clear();
        assert!(!s.is_active());
        assert!(!s.is_all());
        assert!(!s.is_selected(0, 0, &rk(0)));
    }

    #[test]
    fn toggle_under_select_all_excepts_one_keeps_rest() {
        // Ctrl-A then deselect one cell: the predicate stays on, the deselected
        // cell goes to `except`, the rest remain selected.
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.select_all();
        s.toggle(rk(3), 1); // deselect (3,1)
        assert!(s.is_all(), "predicate still active");
        assert!(
            !s.is_selected(3, 1, &rk(3)),
            "the excepted cell is deselected"
        );
        assert!(
            s.is_selected(3, 0, &rk(3)),
            "other column of the same row stays"
        );
        assert!(s.is_selected(8, 2, &rk(8)), "an unrelated cell stays");
        assert_eq!(s.except().len(), 1);
        // Toggling it back removes it from except.
        s.toggle(rk(3), 1);
        assert!(s.is_selected(3, 1, &rk(3)));
        assert!(s.except().is_empty());
    }

    #[test]
    fn collapse_range_keeps_toggled() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.toggle(rk(4), 4);
        s.extend((0, 0), (2, 2));
        s.collapse_transient();
        assert!(!s.is_selected(1, 1, &rk(1)), "range dropped");
        assert!(s.is_selected(4, 4, &rk(4)), "toggled kept");
    }

    #[test]
    fn collapse_clears_select_all_keeps_toggled() {
        // A plain cursor move collapses the *transient* selections — an
        // in-progress range AND a Ctrl-A predicate — but the explicitly
        // toggled set survives until Esc.
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.toggle(rk(4), 4);
        s.select_all(); // Ctrl-A (clears explicit, like a spreadsheet)
        s.collapse_transient();
        assert!(!s.is_all(), "predicate collapsed on a plain move");
        assert!(!s.is_selected(0, 0, &rk(0)));
        // Re-toggle then collapse: the toggle survives a plain move.
        s.toggle(rk(7), 1);
        s.collapse_transient();
        assert!(
            s.is_selected(7, 1, &rk(7)),
            "explicit toggles survive a plain move"
        );
    }

    #[test]
    fn toggle_range_commits_the_rectangle() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.extend((1, 1), (2, 2)); // live range rows 1..2, cols 1..2
        assert!(
            s.toggle_range(|r| Some(rk(r))),
            "there was a range to commit"
        );
        // The range is collapsed, but its cells are now in the durable set:
        assert!(s.anchor.is_none(), "range collapsed after commit");
        assert!(s.is_selected(1, 1, &rk(1)));
        assert!(s.is_selected(2, 2, &rk(2)));
        assert!(s.is_selected(1, 2, &rk(1)));
        // …so a plain move keeps them (they're explicit now, not a range).
        s.collapse_transient();
        assert!(
            s.is_selected(1, 1, &rk(1)),
            "committed range survives a plain move"
        );
        // Re-selecting the same rect and toggling again removes it as a unit.
        s.extend((1, 1), (2, 2));
        s.toggle_range(|r| Some(rk(r)));
        assert!(
            !s.is_selected(2, 2, &rk(2)),
            "toggling an already-selected range clears it"
        );
    }

    #[test]
    fn toggle_range_builds_multiple_ranges() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.extend((0, 0), (1, 0)); // range A
        s.toggle_range(|r| Some(rk(r)));
        s.extend((5, 1), (6, 2)); // range B (after moving + re-extending)
        s.toggle_range(|r| Some(rk(r)));
        assert!(s.is_selected(0, 0, &rk(0)), "range A held");
        assert!(s.is_selected(1, 0, &rk(1)), "range A held");
        assert!(s.is_selected(5, 1, &rk(5)), "range B held");
        assert!(s.is_selected(6, 2, &rk(6)), "range B held");
        assert!(
            !s.is_selected(3, 0, &rk(3)),
            "gap between ranges is unselected"
        );
    }

    #[test]
    fn toggle_range_without_a_range_is_a_noop() {
        let mut s = GridSelection::new(SelectionMode::Cell);
        assert!(
            !s.toggle_range(|r| Some(rk(r))),
            "no range → caller falls back to single-cell"
        );
    }

    #[test]
    fn toggle_range_skips_unresolvable_rows() {
        // A row index the resolver can't map (off the loaded window) is skipped,
        // not panicked or selected.
        let mut s = GridSelection::new(SelectionMode::Cell);
        s.extend((0, 0), (2, 0));
        s.toggle_range(|r| if r == 1 { None } else { Some(rk(r)) });
        assert!(s.is_selected(0, 0, &rk(0)));
        assert!(!s.is_selected(1, 0, &rk(1)), "unresolved row skipped");
        assert!(s.is_selected(2, 0, &rk(2)));
    }

    #[test]
    fn toggle_range_in_row_mode_commits_whole_rows() {
        let mut s = GridSelection::new(SelectionMode::Row);
        s.extend((2, 0), (4, 0));
        s.toggle_range(|r| Some(rk(r)));
        assert!(s.is_selected(2, 9, &rk(2)), "row 2, any column");
        assert!(s.is_selected(4, 0, &rk(4)));
        assert!(!s.is_selected(5, 0, &rk(5)));
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
