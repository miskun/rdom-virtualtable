//! Column operations on [`VirtualTableView`] — sort, reorder, hide/show, and
//! the header sort indicator. A child module of `virtual_table`, so it reaches
//! the view's private fields while keeping the impl off the core view file.

use rdom_tui::TuiDom;

use super::VirtualTableView;
use crate::model::{SortDir, VirtualTable};

impl VirtualTableView {
    /// Current sort `(column, direction)`, or `None` if unsorted.
    pub fn sort_state(&self) -> Option<(usize, SortDir)> {
        self.inner.borrow().sort_state()
    }

    /// Set the sort-direction glyph suffixes appended to the sorted header
    /// text — default `(" ▲", " ▼")`. Include any leading separator yourself
    /// (e.g. `" ^"`). Use **narrow (width-1) glyphs** if your terminal renders
    /// East-Asian *ambiguous-width* characters (`▲`/`▼`, `↑`/`↓`) double-width,
    /// which otherwise shifts header columns after the sorted one by one cell;
    /// `("", "")` disables the glyph (keeping only the `data-sort` attribute).
    /// Takes effect on the next [`sort`](Self::sort) / re-render.
    pub fn set_sort_glyphs(&self, ascending: impl Into<String>, descending: impl Into<String>) {
        *self.sort_glyphs.borrow_mut() = (ascending.into(), descending.into());
    }

    /// Sort by `col` in `dir`, re-materialize the visible window in the new
    /// order, and mark the header (`data-sort="asc|desc"`, which the default
    /// sheet turns into a `▲`/`▼` glyph). The cursor keeps its position; the
    /// **selection is cleared** — it's keyed by row index, which now points at
    /// different data after the reorder. Pass a custom comparator by calling
    /// [`VirtualTable::sort_by_with`] via [`with`](Self::with) then
    /// [`refresh`](Self::refresh) yourself.
    pub fn sort(&self, dom: &mut TuiDom, col: usize, dir: SortDir) {
        self.inner.borrow_mut().sort_by(col, dir);
        self.selection.borrow_mut().clear();
        // Mark the header (and append the glyph to its text) *before* refresh,
        // so `size_columns` measures the glyph and the column is wide enough.
        self.apply_sort_indicator(dom);
        self.refresh(dom);
    }

    /// Toggle the sort on `col`: ascending the first time, then flipping
    /// asc⇄desc on each subsequent call. Ideal for a header-click handler.
    pub fn toggle_sort(&self, dom: &mut TuiDom, col: usize) {
        let dir = match self.sort_state() {
            Some((c, d)) if c == col => d.flipped(),
            _ => SortDir::Ascending,
        };
        self.sort(dom, col, dir);
    }

    /// Move the column at `from` to index `to`: permutes the model (header +
    /// every row's cell), re-syncs the headers + sort glyph, and
    /// re-materializes the visible window in the new order. The **cursor
    /// follows** its column; the **selection is cleared** (a structural change,
    /// like sort). No-op for out-of-range or equal indices.
    pub fn move_column(&self, dom: &mut TuiDom, from: usize, to: usize) {
        let (rows, cols) = self.with(|t| (t.row_count(), t.columns().len()));
        if from >= cols || to >= cols || from == to {
            return;
        }
        self.with(|t| t.move_column(from, to));
        let cur = self.cursor.get();
        let new_col = VirtualTable::remapped_index(from, to, cur.col());
        self.cursor.set(cur.at(cur.row(), new_col, rows, cols));
        self.selection.borrow_mut().clear();
        // Headers persist across `show_window`, so re-sync their labels/glyph
        // to the new order *before* refresh (so `size_columns` measures right).
        self.apply_sort_indicator(dom);
        self.refresh(dom);
    }

    /// Hide or show the column at `col`. A hidden column gets `data-vt-hidden`
    /// on its header `<th>` and every body cell (the default sheet maps that to
    /// `display: none`), the cursor skips it on horizontal navigation, and the
    /// flag follows the column through reordering. No-op for out-of-range `col`
    /// at the DOM level (the model still records it).
    pub fn set_column_hidden(&self, dom: &mut TuiDom, col: usize, hidden: bool) {
        self.inner.borrow_mut().set_column_hidden(col, hidden);
        if let Some(&th) = self.header_cells.borrow().get(col) {
            super::set_flag(dom, th, "data-vt-hidden", hidden);
        }
        // Re-materialize so the body cells pick up (or drop) the attribute.
        self.refresh(dom);
    }

    /// Reflect the model's sort state onto the headers: `data-sort="asc|desc"`
    /// on the sorted `<th>` (the CSS contract), removed from the rest, and a
    /// `▲`/`▼` glyph appended to the sorted header's **text**. Headers persist
    /// across `show_window` (built once at `mount`), so this only runs on sort.
    ///
    /// The glyph is rendered as header text rather than the cleaner
    /// `th[data-sort]::after` CSS pseudo-element because the substrate's
    /// `size_columns` measures only text-node width (and runs before cascade),
    /// so an `::after` glyph would be clipped by the auto-computed column width.
    /// See `STATE.md` (substrate-friction backlog).
    pub(super) fn apply_sort_indicator(&self, dom: &mut TuiDom) {
        let state = self.inner.borrow().sort_state();
        let headers = self.header_cells.borrow();
        let model = self.inner.borrow();
        let glyphs = self.sort_glyphs.borrow();
        for (c, &th) in headers.iter().enumerate() {
            let label = model.columns().get(c).map_or("", |col| col.header.as_str());
            let (attr, glyph) = match state {
                Some((sc, SortDir::Ascending)) if sc == c => (Some("asc"), glyphs.0.as_str()),
                Some((sc, SortDir::Descending)) if sc == c => (Some("desc"), glyphs.1.as_str()),
                _ => (None, ""),
            };
            match attr {
                Some(v) => {
                    let _ = dom.set_attribute(th, "data-sort", v);
                }
                None => {
                    let _ = dom.remove_attribute(th, "data-sort");
                }
            }
            // Reset the header text to `label (+ glyph)`; the first child of a
            // header `<th>` is its text node (built that way in `mount`).
            if let Some(text_id) = dom.node(th).child_nodes().next().map(|n| n.id()) {
                let _ = dom
                    .node_mut(text_id)
                    .set_node_value(&format!("{label}{glyph}"));
            }
        }
    }
}
