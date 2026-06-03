//! Virtualized table built on native `<table>` elements.
//!
//! Unlike the chart components (which paint onto a `<canvas>`), the table
//! is a real DOM subtree: `<table>` → `<thead>`/`<tbody>` → `<tr>` →
//! `<th>`/`<td>`. rdom-tui's table builtin aligns columns across rows, so
//! this component only has to materialize the right rows.
//!
//! **Virtualization:** the data lives in the model; only a *window* of
//! rows is ever materialized into the `<tbody>`. A consumer with 100k
//! rows builds at most `count` `<tr>` nodes. Call
//! [`show_window`](VirtualTableView::show_window) with the slice to
//! display; recompute the slice from a scroll offset with
//! [`VirtualTable::window_for`].
//!
//! **Native scrollbar (opt-in):**
//! [`enable_scrollbar`](VirtualTableView::enable_scrollbar) makes the
//! `<tbody>` a vertical scroll container and brackets the window with
//! spacer rows so the scroll thumb reflects the *total* row count; a
//! `scroll` listener re-windows on wheel/drag. Without it, the consumer
//! drives [`show_window`](VirtualTableView::show_window) explicitly.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rdom_tui::runtime::builtins::table::size_columns;
use rdom_tui::{
    Color, ListenerOptions, NodeId, Overflow, Size, Stylesheet, TuiAccessors, TuiAccessorsMut,
    TuiDom, TuiNodeMutExt, TuiStyle,
};

use crate::grid_cursor::{GridCursor, Nav, nav_for_key};
use crate::selection::{GridSelection, SelectionMode};

/// A table column: a header label and an optional fixed width (otherwise
/// the column auto-sizes to its widest cell).
#[derive(Clone, Debug)]
pub struct Column {
    pub header: String,
    pub width: Option<u16>,
}

impl Column {
    pub fn new(header: impl Into<String>) -> Self {
        Self {
            header: header.into(),
            width: None,
        }
    }

    pub fn with_width(mut self, width: u16) -> Self {
        self.width = Some(width);
        self
    }
}

/// Sort direction for [`VirtualTable::sort_by`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    Ascending,
    Descending,
}

impl SortDir {
    /// The opposite direction — handy for toggling a header.
    pub fn flipped(self) -> Self {
        match self {
            SortDir::Ascending => SortDir::Descending,
            SortDir::Descending => SortDir::Ascending,
        }
    }
}

/// Default cell comparator: numeric when *both* cells parse as numbers
/// (so `"2" < "10"`), lexicographic otherwise. Override per-sort with
/// [`VirtualTable::sort_by_with`].
fn default_cell_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.cmp(b),
    }
}

/// The cell text at `col`, or `""` if the row is short.
fn cell_at(row: &[String], col: usize) -> &str {
    row.get(col).map(String::as_str).unwrap_or("")
}

/// The table model: columns + row data. Holds no DOM state.
pub struct VirtualTable {
    columns: Vec<Column>,
    rows: Vec<Vec<String>>,
    /// Current sort `(column, direction)`, or `None` if unsorted.
    sort: Option<(usize, SortDir)>,
}

impl VirtualTable {
    pub fn new(columns: Vec<Column>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
            sort: None,
        }
    }

    pub fn set_rows(&mut self, rows: Vec<Vec<String>>) {
        self.rows = rows;
    }

    pub fn push_row(&mut self, row: Vec<String>) {
        self.rows.push(row);
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// Read-only view of the row data (in current sort order).
    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    /// Current sort `(column, direction)`, or `None` if unsorted.
    pub fn sort_state(&self) -> Option<(usize, SortDir)> {
        self.sort
    }

    /// Sort the rows by `col` using the [`default_cell_cmp`] comparator
    /// (numeric-aware, else lexicographic). Stable: equal keys keep their
    /// prior order. Records the sort so [`sort_state`](Self::sort_state)
    /// reflects it. Out-of-range `col` sorts by the empty string (a no-op
    /// ordering) and still records the state.
    pub fn sort_by(&mut self, col: usize, dir: SortDir) {
        self.sort_by_with(col, dir, default_cell_cmp);
    }

    /// Like [`sort_by`](Self::sort_by) but with a custom cell comparator —
    /// the sort hook. `cmp(a, b)` compares the two cells' text in column
    /// `col`; `dir` reverses it for descending.
    pub fn sort_by_with(
        &mut self,
        col: usize,
        dir: SortDir,
        cmp: impl Fn(&str, &str) -> std::cmp::Ordering,
    ) {
        self.rows.sort_by(|a, b| {
            let ord = cmp(cell_at(a, col), cell_at(b, col));
            match dir {
                SortDir::Ascending => ord,
                SortDir::Descending => ord.reverse(),
            }
        });
        self.sort = Some((col, dir));
    }

    /// Move the column at `from` to index `to`, permuting the header and
    /// **every row's cell** by the same amount so the model stays consistent.
    /// The recorded sort column (if any) is remapped so the sort follows its
    /// column. No-op for out-of-range or equal indices.
    pub fn move_column(&mut self, from: usize, to: usize) {
        let n = self.columns.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let col = self.columns.remove(from);
        self.columns.insert(to, col);
        for row in &mut self.rows {
            if from < row.len() {
                let cell = row.remove(from);
                row.insert(to.min(row.len()), cell);
            }
        }
        if let Some((c, dir)) = self.sort {
            self.sort = Some((Self::remapped_index(from, to, c), dir));
        }
    }

    /// Where index `i` lands after [`move_column(from, to)`](Self::move_column)
    /// — pure, so the view can remap the cursor column the same way. `from`
    /// maps to `to`; indices between shift by one toward the vacated slot;
    /// everything outside `[from, to]` is unchanged.
    pub fn remapped_index(from: usize, to: usize, i: usize) -> usize {
        if i == from {
            to
        } else if from < to {
            if i > from && i <= to { i - 1 } else { i }
        } else if i >= to && i < from {
            i + 1
        } else {
            i
        }
    }

    /// Compute the row window to materialize: `(start, count)` for a
    /// viewport that can show `viewport_rows` data rows, scrolled so the
    /// top visible row is `scroll_y`. Pure — the unit of testing for the
    /// virtualization math.
    pub fn window_for(viewport_rows: u16, scroll_y: usize, total: usize) -> (usize, usize) {
        let start = scroll_y.min(total);
        let count = (viewport_rows as usize).min(total - start);
        (start, count)
    }
}

/// A shareable handle that owns a [`VirtualTable`] and materializes a
/// window of it as a `<table>` subtree in a `TuiDom`.
#[derive(Clone)]
pub struct VirtualTableView {
    inner: Rc<RefCell<VirtualTable>>,
    table: Rc<Cell<Option<NodeId>>>,
    tbody: Rc<Cell<Option<NodeId>>>,
    mounted_rows: Rc<RefCell<Vec<NodeId>>>,
    /// Cell (`<td>`) node ids per materialized row, in column order. Parallel
    /// to `mounted_rows`; lets highlight run without re-walking the tree.
    mounted_cells: Rc<RefCell<Vec<Vec<NodeId>>>>,
    /// Header (`<th>`) node ids in column order, recorded at `mount`.
    header_cells: Rc<RefCell<Vec<NodeId>>>,
    /// Logical keyboard cursor over the full dataset.
    cursor: Rc<Cell<GridCursor>>,
    /// Selection state (configurable mode; off by default).
    selection: Rc<RefCell<GridSelection>>,
    /// Visible data-row count — drives scroll-follow and the page step.
    viewport_rows: Rc<Cell<u16>>,
    /// Logical row that materialized pool row 0 currently represents.
    window_start: Rc<Cell<usize>>,
    /// Whether navigation/highlight is engaged. Pure-virtualization
    /// consumers (no cursor) leave this `false`, so `show_window` never
    /// writes `data-active-*` attributes behind their back.
    nav_active: Rc<Cell<bool>>,
    /// Sort-direction glyph suffixes `(ascending, descending)` appended to the
    /// sorted header text. Default `(" ▲", " ▼")`. Configurable because `▲`/`▼`
    /// are East-Asian *ambiguous-width*: a terminal set to render ambiguous
    /// glyphs double-width shifts later header columns by one — set narrow
    /// glyphs (`" ^"` / `" v"`, `" ↑"` / `" ↓"`) or `""` to avoid it.
    sort_glyphs: Rc<RefCell<(String, String)>>,
    /// Whether the native vertical scrollbar is engaged (`enable_scrollbar`).
    /// When set, `show_window` brackets the row window with spacer `<tr>`s so
    /// the `<tbody>`'s scroll extent reflects the *total* row count.
    scroll_mode: Rc<Cell<bool>>,
    /// The spacer `<tr>` ids (top + bottom) currently in `<tbody>`, dropped
    /// alongside the row window on the next `show_window`.
    spacers: Rc<RefCell<Vec<NodeId>>>,
}

impl VirtualTableView {
    pub fn new(table: VirtualTable) -> Self {
        Self {
            inner: Rc::new(RefCell::new(table)),
            table: Rc::new(Cell::new(None)),
            tbody: Rc::new(Cell::new(None)),
            mounted_rows: Rc::new(RefCell::new(Vec::new())),
            mounted_cells: Rc::new(RefCell::new(Vec::new())),
            header_cells: Rc::new(RefCell::new(Vec::new())),
            cursor: Rc::new(Cell::new(GridCursor::new())),
            selection: Rc::new(RefCell::new(GridSelection::new(SelectionMode::None))),
            viewport_rows: Rc::new(Cell::new(0)),
            window_start: Rc::new(Cell::new(0)),
            nav_active: Rc::new(Cell::new(false)),
            sort_glyphs: Rc::new(RefCell::new((" \u{25B2}".into(), " \u{25BC}".into()))),
            scroll_mode: Rc::new(Cell::new(false)),
            spacers: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Build `<table><thead>…</thead><tbody></tbody></table>`, remember
    /// the `<table>` and `<tbody>` ids, and return the `<table>` id. The
    /// `<tbody>` starts empty — call [`show_window`](Self::show_window).
    pub fn mount(&self, dom: &mut TuiDom) -> NodeId {
        let table = dom.create_element("table");
        let thead = dom.create_element("thead");
        let header_tr = dom.create_element("tr");

        let model = self.inner.borrow();
        let mut headers = Vec::with_capacity(model.columns.len());
        for col in &model.columns {
            let th = dom.create_element("th");
            let text = dom.create_text_node(&col.header);
            dom.append_child(th, text).unwrap();
            if let Some(w) = col.width {
                dom.node_mut(th).set_width(Size::Fixed(w));
            }
            dom.append_child(header_tr, th).unwrap();
            headers.push(th);
        }
        drop(model);
        *self.header_cells.borrow_mut() = headers;

        dom.append_child(thead, header_tr).unwrap();
        dom.append_child(table, thead).unwrap();

        let tbody = dom.create_element("tbody");
        dom.append_child(table, tbody).unwrap();

        self.table.set(Some(table));
        self.tbody.set(Some(tbody));
        table
    }

    /// Materialize rows `[start, start + count)` into the `<tbody>`,
    /// dropping any previously-materialized rows. No-op before `mount`.
    pub fn show_window(&self, dom: &mut TuiDom, start: usize, count: usize) {
        let Some(tbody) = self.tbody.get() else {
            return;
        };

        // Drop the previous window's rows + spacers (frees the arena slots).
        for id in self.mounted_rows.borrow_mut().drain(..) {
            let _ = dom.drop_subtree(id);
        }
        for id in self.spacers.borrow_mut().drain(..) {
            let _ = dom.drop_subtree(id);
        }
        self.mounted_cells.borrow_mut().clear();

        let total = self.inner.borrow().rows.len();
        let end = (start + count).min(total);

        // Native-scrollbar mode: a top spacer of `start` rows makes the window
        // sit at the right scroll offset, and top + window + bottom = total, so
        // the `<tbody>` scroll extent reflects the whole dataset (proportional
        // thumb) while only the window is materialized.
        if self.scroll_mode.get() && start > 0 {
            let sp = self.make_spacer(dom, start);
            dom.append_child(tbody, sp).unwrap();
            self.spacers.borrow_mut().push(sp);
        }

        let model = self.inner.borrow();
        let ncols = model.columns.len();
        let span = end.saturating_sub(start);
        let mut mounted = Vec::with_capacity(span);
        let mut mounted_cells = Vec::with_capacity(span);
        for row in &model.rows[start.min(model.rows.len())..end] {
            let tr = dom.create_element("tr");
            let mut row_cells = Vec::with_capacity(ncols);
            for c in 0..ncols {
                let td = dom.create_element("td");
                let cell = row.get(c).map(String::as_str).unwrap_or("");
                let text = dom.create_text_node(cell);
                dom.append_child(td, text).unwrap();
                dom.append_child(tr, td).unwrap();
                row_cells.push(td);
            }
            dom.append_child(tbody, tr).unwrap();
            mounted.push(tr);
            mounted_cells.push(row_cells);
        }
        drop(model);

        // Bottom spacer for the rows below the window (see top spacer above).
        if self.scroll_mode.get() && end < total {
            let sp = self.make_spacer(dom, total - end);
            dom.append_child(tbody, sp).unwrap();
            self.spacers.borrow_mut().push(sp);
        }

        *self.mounted_rows.borrow_mut() = mounted;
        *self.mounted_cells.borrow_mut() = mounted_cells;
        self.window_start.set(start);

        if let Some(table) = self.table.get() {
            // `size_columns` (rdom-tui ≥ 0.3.5) stamps a column-width signature
            // on the `<table>` when the widths change, which dirties the table
            // subtree so the `<thead>` headers re-cascade with the new widths —
            // no consumer-side re-cascade hack needed (cf. `TABLE-COLSYNC-DIRTY-1`).
            size_columns(dom, table);
        }

        // Re-assert the cursor highlight onto the freshly-materialized window.
        // Gated so pure-virtualization consumers never get `data-active-*`.
        if self.nav_active.get() {
            self.apply_highlight(dom);
        }
    }

    /// Set the number of visible data rows. This drives scroll-follow (how
    /// far the cursor can travel before the window shifts) and the `PageUp`/
    /// `PageDown` step. Enabling a viewport also engages navigation, so
    /// subsequent [`show_window`](Self::show_window) calls reassert the
    /// cursor highlight.
    pub fn set_viewport_rows(&self, rows: u16) {
        self.viewport_rows.set(rows);
        self.nav_active.set(true);
    }

    /// Build a spacer `<tr>` of `rows` cells tall, marked so consumer CSS and
    /// the highlight pass skip it. Height is `u16`-bounded (~65k rows) — see
    /// [`enable_scrollbar`](Self::enable_scrollbar) for the implication.
    fn make_spacer(&self, dom: &mut TuiDom, rows: usize) -> NodeId {
        let h = rows.min(u16::MAX as usize) as u16;
        let tr = dom.create_element("tr");
        dom.node_mut(tr)
            .set_inline_style(TuiStyle::new().height(Size::Fixed(h)));
        let _ = dom.set_attribute(tr, "data-rdom-spacer", "");
        tr
    }

    /// Engage the **native vertical scrollbar**: the `<tbody>` becomes a
    /// `overflow-y: auto` scroll container `viewport_rows` tall, the window is
    /// bracketed with spacer `<tr>`s so the scroll thumb reflects the *total*
    /// row count, and a `scroll` listener re-windows as the user wheels / drags
    /// (decoupled — scrolling moves the viewport, not the cursor; the cursor
    /// scrolls back into view only when [`navigate`](Self::navigate)d). Call
    /// after [`set_viewport_rows`](Self::set_viewport_rows) /
    /// [`install_nav`](Self::install_nav) and after the first
    /// [`show_window`](Self::show_window). The `<thead>` stays put (it's outside
    /// the scroll container — no sticky needed) so header and body columns stay
    /// aligned.
    ///
    /// **Assumes uniform single-cell rows** (the spacer/offset math is in row
    /// units = cells); wrapped or multi-line cells break the scroll mapping.
    /// The spacer height is `u16`-bounded, so the draggable thumb spans the
    /// first ~65k rows; beyond that, keyboard navigation (unbounded) still
    /// reaches every row.
    pub fn enable_scrollbar(&self, dom: &mut TuiDom) {
        let Some(tbody) = self.tbody.get() else {
            return;
        };
        let vp = self.viewport_rows.get();
        dom.node_mut(tbody).set_inline_style(
            TuiStyle::new()
                .overflow_y(Overflow::Auto)
                .height(Size::Fixed(vp)),
        );
        self.scroll_mode.set(true);

        // Decoupled scroll: on wheel/drag, re-window to the new offset; the
        // cursor is left untouched. The listener NEVER writes scroll_top, so
        // there's no re-entrancy with the cursor path (which DOES write it).
        let view = self.clone();
        dom.add_event_listener(tbody, "scroll", ListenerOptions::default(), move |ctx| {
            let top = ctx.dom.node(tbody).scroll_top().unwrap_or(0).max(0) as usize;
            if top != view.window_start.get() {
                view.show_window(ctx.dom, top, view.viewport_rows.get() as usize);
                ctx.request_redraw();
            }
        })
        .expect("tbody accepts a scroll listener");

        // Re-materialize the current window so the spacers appear.
        self.show_window(dom, self.window_start.get(), vp as usize);
    }

    /// The current logical cursor (active `(row, col)` + scroll offset).
    /// Useful for acting on the focused row (e.g. an `Enter` handler).
    pub fn cursor(&self) -> GridCursor {
        self.cursor.get()
    }

    /// Move the cursor per `nav`, clamped + scrolled into view, and return
    /// `(before, after)`. Caller updates the selection, then calls
    /// [`refresh_after_cursor`](Self::refresh_after_cursor). Returns `None`
    /// for an empty grid.
    fn move_cursor(&self, nav: Nav) -> Option<(GridCursor, GridCursor)> {
        let (rows, cols) = self.with(|t| (t.row_count(), t.columns().len()));
        if rows == 0 || cols == 0 {
            return None;
        }
        let viewport = self.viewport_rows.get() as usize;
        let before = self.cursor.get();
        let after = before
            .navigate(nav, rows, cols, viewport.max(1))
            .follow(viewport, rows);
        self.cursor.set(after);
        Some((before, after))
    }

    /// Re-materialize the window if the cursor scrolled it, else just re-apply
    /// the highlight + selection attributes.
    fn refresh_after_cursor(&self, dom: &mut TuiDom, after: GridCursor) {
        // Native-scrollbar mode: a single write direction — move the scrollbar
        // to keep the cursor visible and let the `scroll` listener re-window +
        // re-highlight. If the cursor stayed within the current window (no
        // scroll change) the listener won't fire, so re-highlight here.
        if self.scroll_mode.get() {
            let scrolled = after.scroll() != self.window_start.get();
            if let Some(tbody) = self.tbody.get() {
                let _ = dom.node_mut(tbody).set_scroll_top(after.scroll() as i32);
            }
            if !scrolled {
                self.apply_highlight(dom);
            }
            return;
        }
        let viewport = self.viewport_rows.get() as usize;
        if viewport > 0 && after.scroll() != self.window_start.get() {
            let rows = self.with(|t| t.row_count());
            let (start, count) = VirtualTable::window_for(viewport as u16, after.scroll(), rows);
            self.show_window(dom, start, count); // re-applies highlight + selection
        } else {
            self.apply_highlight(dom);
        }
    }

    /// Apply a navigation move to the cursor, scroll to keep it visible,
    /// re-materialize the window if it shifted, and update the highlight
    /// attributes. A plain move collapses the *transient* selections (an
    /// in-progress range and a `Ctrl-A` select-all) while keeping the
    /// `Space`-toggled set — see [`GridSelection::collapse_transient`]. Returns
    /// `true` if the cursor actually moved.
    pub fn navigate(&self, dom: &mut TuiDom, nav: Nav) -> bool {
        self.nav_active.set(true);
        let Some((before, after)) = self.move_cursor(nav) else {
            return false;
        };
        self.selection.borrow_mut().collapse_transient();
        self.refresh_after_cursor(dom, after);
        after != before
    }

    // ── Selection (configurable; off by default) ─────────────────────

    /// Set the selection mode. `SelectionMode::None` (default) disables
    /// selection entirely; `Cell` selects rectangular cell ranges; `Row`
    /// selects whole rows. Changing the mode clears any active selection and
    /// engages the highlight contract.
    pub fn set_selection_mode(&self, mode: SelectionMode) {
        self.selection.borrow_mut().set_mode(mode);
        if mode != SelectionMode::None {
            self.nav_active.set(true);
        }
    }

    /// The current selection mode.
    pub fn selection_mode(&self) -> SelectionMode {
        self.selection.borrow().mode()
    }

    /// A snapshot of the current selection — query it with
    /// [`GridSelection::is_selected`] / [`is_active`](GridSelection::is_active).
    pub fn selection(&self) -> GridSelection {
        self.selection.borrow().clone()
    }

    /// Extend the selection by moving the cursor (Shift+arrow): the range
    /// anchors at the pre-move cursor and its head follows. No-op when the
    /// mode is `None`.
    pub fn extend_selection(&self, dom: &mut TuiDom, nav: Nav) -> bool {
        self.nav_active.set(true);
        let Some((before, after)) = self.move_cursor(nav) else {
            return false;
        };
        self.selection
            .borrow_mut()
            .extend((before.row(), before.col()), (after.row(), after.col()));
        self.refresh_after_cursor(dom, after);
        after != before
    }

    /// Toggle the selection at the cursor (`Space`). If a `Shift`-range is
    /// live, the **whole rectangle** is committed into the sticky set (and the
    /// range collapses) — so Shift-select + `Space` builds a persistent range,
    /// repeatable for multiple ranges. With no live range it toggles just the
    /// cursor cell (or row, in `Row` mode).
    pub fn toggle_selection(&self, dom: &mut TuiDom) {
        let c = self.cursor.get();
        {
            let mut sel = self.selection.borrow_mut();
            if !sel.toggle_range() {
                sel.toggle(c.row(), c.col());
            }
        }
        self.apply_highlight(dom);
    }

    /// Select the whole grid (`Ctrl-A`).
    pub fn select_all(&self, dom: &mut TuiDom) {
        self.selection.borrow_mut().select_all();
        self.apply_highlight(dom);
    }

    /// Clear the selection (`Esc`).
    pub fn clear_selection(&self, dom: &mut TuiDom) {
        self.selection.borrow_mut().clear();
        self.apply_highlight(dom);
    }

    // ── Column ops: sort ─────────────────────────────────────────────

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

    /// Re-materialize the currently-shown window (same start + count) — call
    /// after mutating the model via [`with`](Self::with) so the DOM reflects
    /// the change. No-op before [`mount`](Self::mount).
    pub fn refresh(&self, dom: &mut TuiDom) {
        if self.tbody.get().is_none() {
            return;
        }
        let start = self.window_start.get();
        let count = self.mounted_rows.borrow().len();
        self.show_window(dom, start, count);
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
    fn apply_sort_indicator(&self, dom: &mut TuiDom) {
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

    /// Attach a `keydown` listener to `table` that drives [`navigate`] with
    /// the built-in [`nav_for_key`] keymap (arrows, `hjkl`, `g`/`G`,
    /// `Home`/`End`, `PageUp`/`PageDown`). Handled keys are consumed
    /// (`prevent_default`) and trigger a redraw. `viewport_rows` is the
    /// number of visible data rows.
    ///
    /// The `table` must be focusable (e.g. `tabindex="0"`) and focused for
    /// the keys to arrive. Pair with [`highlight_stylesheet`] (or your own
    /// `data-active-*` rules) to see the cursor.
    pub fn install_nav(&self, dom: &mut TuiDom, table: NodeId, viewport_rows: u16) {
        self.set_viewport_rows(viewport_rows);
        let view = self.clone();
        dom.add_event_listener(table, "keydown", ListenerOptions::default(), move |ctx| {
            let Some(kbd) = ctx.event.detail.as_keyboard() else {
                return;
            };
            let key = kbd.key.as_str();
            let shift = kbd.modifiers.shift;
            let ctrl = kbd.modifiers.ctrl || kbd.modifiers.meta;
            let mut handled = true;

            // Selection keys (only when a selection mode is engaged): Ctrl-A
            // select-all, Esc clear, Space toggle the cursor cell/row,
            // Shift+nav extend the range.
            if view.selection_mode() != SelectionMode::None {
                if ctrl && key == "a" {
                    view.select_all(ctx.dom);
                } else if key == "Escape" {
                    view.clear_selection(ctx.dom);
                } else if key == " " {
                    view.toggle_selection(ctx.dom);
                } else if shift {
                    match nav_for_key(key, false) {
                        Some(nav) => {
                            view.extend_selection(ctx.dom, nav);
                        }
                        None => handled = false,
                    }
                } else if let Some(nav) = nav_for_key(key, false) {
                    view.navigate(ctx.dom, nav);
                } else {
                    handled = false;
                }
            } else if let Some(nav) = nav_for_key(key, shift) {
                view.navigate(ctx.dom, nav);
            } else {
                handled = false;
            }

            if handled {
                ctx.event.prevent_default();
                ctx.request_redraw();
            }
        })
        .expect("table node accepts a keydown listener");
        // Reflect the initial cursor onto whatever window is already shown.
        self.apply_highlight(dom);
    }

    /// Write the cursor (`data-active-row` / `-col` / `-cell`) and selection
    /// (`data-selected`) presence attributes onto the materialized window +
    /// header to match the current cursor + selection. Clears them everywhere
    /// they no longer apply, so a single pass both sets and unsets.
    fn apply_highlight(&self, dom: &mut TuiDom) {
        let cursor = self.cursor.get();
        let start = self.window_start.get();
        let sel = self.selection.borrow();

        for (c, &th) in self.header_cells.borrow().iter().enumerate() {
            set_flag(dom, th, "data-active-col", c == cursor.col());
        }

        let rows = self.mounted_rows.borrow();
        let cells = self.mounted_cells.borrow();
        for (i, &tr) in rows.iter().enumerate() {
            let vrow = start + i;
            let row_active = vrow == cursor.row();
            set_flag(dom, tr, "data-active-row", row_active);
            let mut row_selected = false;
            if let Some(row_cells) = cells.get(i) {
                for (c, &td) in row_cells.iter().enumerate() {
                    let col_active = c == cursor.col();
                    set_flag(dom, td, "data-active-col", col_active);
                    set_flag(dom, td, "data-active-cell", row_active && col_active);
                    let selected = sel.is_selected(vrow, c);
                    set_flag(dom, td, "data-selected", selected);
                    row_selected |= selected;
                }
            }
            // `<tr data-selected>` lets CSS mark a whole selected row (and in
            // Row mode every cell carries it too, for a full-width fill).
            set_flag(dom, tr, "data-selected", row_selected);
        }
    }

    /// Borrow the model mutably to update columns/rows. After changing
    /// data, call [`show_window`](Self::show_window) again to re-render.
    pub fn with<R>(&self, f: impl FnOnce(&mut VirtualTable) -> R) -> R {
        f(&mut self.inner.borrow_mut())
    }

    /// Number of rows currently materialized in the DOM.
    pub fn mounted_row_count(&self) -> usize {
        self.mounted_rows.borrow().len()
    }
}

/// Set (`on`) or remove (`!on`) a presence attribute on `id`.
fn set_flag(dom: &mut TuiDom, id: NodeId, attr: &str, on: bool) {
    if on {
        let _ = dom.set_attribute(id, attr, "");
    } else {
        let _ = dom.remove_attribute(id, attr);
    }
}

/// The default highlight selectors + styles for the cursor, focus-gated so
/// the highlight only shows while the table is focused. Returned as
/// `(selector, style)` pairs so consumers can fold them into an existing
/// sheet or tweak the colors.
///
/// The contract is three presence attributes the view writes:
/// - `data-active-row` on the `<tr>` under the cursor,
/// - `data-active-col` on every `<th>`/`<td>` in the cursor's column,
/// - `data-active-cell` on the single `<td>` at the cursor.
///
/// It also styles the **selection** ([`set_selection_mode`](VirtualTableView::set_selection_mode)):
/// - `data-selected` on every selected `<td>` (and the `<tr>` of a row with any
///   selection), painted a distinct blue (`#1e3a5f`).
///
/// Colors, in source-order precedence (all rules are equal zero-specificity
/// `:where()`, so later wins): the active row/column tint (`#181a1c`), then the
/// selection blue (`#1e3a5f`), then a brighter blue (`#2b557e`) where a
/// selected cell *also* sits in the active row/column — a pre-computed
/// "selection over the cross-hair" blend, since a TUI can't alpha-composite
/// opaque cells, so the highlight shows through instead of being flatly
/// overpainted — then the cursor cell last (so it stays visible in a
/// selection): `#2d2f31` gray normally, or the brightest blue (`#3a6ea5`) when
/// the cursor cell is itself selected, so it fits the surrounding blue field.
///
/// (As of rdom-tui 0.3.4 the UA focus tint is scoped to interactive controls,
/// so a focused `<table>` is not washed with the focus background — no
/// `table:focus { background: reset }` workaround is needed.)
///
/// Each selector is wrapped in **`:where()`** so the whole rule carries **zero
/// specificity** (it still only matches a focused table's active cells —
/// `:where()` changes specificity, not matching). That makes these true
/// *defaults*: any author rule of any specificity overrides them, exactly like
/// overriding a browser UA style. Requires rdom-tui ≥ 0.3.4.
pub fn highlight_rules() -> Vec<(&'static str, TuiStyle)> {
    // #181a1c — shared row/column tint.
    let line = Color::Rgb(0x18, 0x1a, 0x1c);
    // #1e3a5f — selected cells (a distinct blue).
    let selected = Color::Rgb(0x1e, 0x3a, 0x5f);
    // #2b557e — a selected cell that also sits in the active row/column. A TUI
    // can't alpha-composite opaque cells, so this is the pre-computed "selection
    // over the row/column tint" — a brighter blue so the cross-hair shows
    // through the selection instead of being flatly overpainted.
    let selected_line = Color::Rgb(0x2b, 0x55, 0x7e);
    // #2d2f31 — the cursor cell by default: a neutral gray matching rdom's focus
    // tint (inputs/tree). When the cursor cell is itself *selected* it sits in
    // the blue selection field, so a gray fill reads oddly — it switches to the
    // brightest blue (`cell_blue`) so it's the focal point of the blue family.
    let cell_gray = Color::Rgb(0x2d, 0x2f, 0x31);
    let cell_blue = Color::Rgb(0x3a, 0x6e, 0xa5);
    vec![
        (
            ":where(table:focus tr[data-active-row])",
            TuiStyle::new().bg(line),
        ),
        (
            ":where(table:focus th[data-active-col])",
            TuiStyle::new().bg(line),
        ),
        (
            ":where(table:focus td[data-active-col])",
            TuiStyle::new().bg(line),
        ),
        (
            ":where(table:focus td[data-selected])",
            TuiStyle::new().bg(selected),
        ),
        // Selection ∩ row/column highlight → the blend (listed after the plain
        // selection so it wins on the intersection; all rules are equal
        // zero-specificity `:where()`, so source order decides).
        (
            ":where(table:focus td[data-selected][data-active-col])",
            TuiStyle::new().bg(selected_line),
        ),
        (
            ":where(table:focus tr[data-active-row] td[data-selected])",
            TuiStyle::new().bg(selected_line),
        ),
        // The cursor cell wins last, so it stays visible inside a selection.
        // Gray normally; a brighter blue when the cursor cell is itself
        // selected (it sits in the blue field, so blue fits).
        (
            ":where(table:focus td[data-active-cell])",
            TuiStyle::new().bg(cell_gray),
        ),
        (
            ":where(table:focus td[data-active-cell][data-selected])",
            TuiStyle::new().bg(cell_blue),
        ),
        // Focused-scroll affordance (rdom `FOCUS-VOCAB-1`): a focused scroll
        // region shows an accent (DodgerBlue) thumb glyph. The substrate's UA
        // `:focus-within::scrollbar-thumb` can't fire for `enable_scrollbar`
        // because the scroll container (`<tbody>`) is a *child* of the focused
        // `<table>` (focus is on the parent, not within the tbody) — so bridge
        // it: when the table holds focus, accent its body scrollbar thumb.
        (
            ":where(table:focus-within tbody)::scrollbar-thumb",
            TuiStyle::new().fg(Color::Rgb(30, 144, 255)),
        ),
    ]
}

/// A ready-made [`Stylesheet`] with the default cursor/selection highlight
/// from [`highlight_rules`]. Drop it straight into `App::new`, or merge the
/// rules into your own sheet for custom colors.
pub fn highlight_stylesheet() -> Stylesheet {
    let mut sheet = Stylesheet::new();
    for (selector, style) in highlight_rules() {
        sheet = sheet
            .rule(selector, style)
            .expect("built-in highlight selectors parse");
    }
    sheet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_at_top() {
        assert_eq!(VirtualTable::window_for(10, 0, 100), (0, 10));
    }

    #[test]
    fn window_near_end_clamps_count() {
        assert_eq!(VirtualTable::window_for(10, 95, 100), (95, 5));
    }

    #[test]
    fn window_past_end_is_empty() {
        assert_eq!(VirtualTable::window_for(10, 200, 100), (100, 0));
    }

    #[test]
    fn window_smaller_dataset_than_viewport() {
        assert_eq!(VirtualTable::window_for(50, 0, 7), (0, 7));
    }

    #[test]
    fn model_row_bookkeeping() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b")]);
        assert!(t.is_empty());
        t.push_row(vec!["1".into(), "2".into()]);
        t.set_rows(vec![
            vec!["x".into(), "y".into()],
            vec!["p".into(), "q".into()],
        ]);
        assert_eq!(t.row_count(), 2);
        assert_eq!(t.columns().len(), 2);
    }

    fn col0(t: &VirtualTable) -> Vec<&str> {
        t.rows().iter().map(|r| r[0].as_str()).collect()
    }

    #[test]
    fn sort_by_orders_rows_both_directions_and_records_state() {
        let mut t = VirtualTable::new(vec![Column::new("a")]);
        t.set_rows(vec![
            vec!["banana".into()],
            vec!["apple".into()],
            vec!["cherry".into()],
        ]);
        t.sort_by(0, SortDir::Ascending);
        assert_eq!(col0(&t), vec!["apple", "banana", "cherry"]);
        assert_eq!(t.sort_state(), Some((0, SortDir::Ascending)));
        t.sort_by(0, SortDir::Descending);
        assert_eq!(col0(&t), vec!["cherry", "banana", "apple"]);
        assert_eq!(t.sort_state(), Some((0, SortDir::Descending)));
    }

    #[test]
    fn sort_is_numeric_aware() {
        // Lexical sort would give 1, 10, 2; numeric gives 1, 2, 10.
        let mut t = VirtualTable::new(vec![Column::new("n")]);
        t.set_rows(vec![vec!["10".into()], vec!["2".into()], vec!["1".into()]]);
        t.sort_by(0, SortDir::Ascending);
        assert_eq!(col0(&t), vec!["1", "2", "10"]);
    }

    #[test]
    fn sort_is_stable_for_equal_keys() {
        // Equal sort keys keep their original relative order (stable sort).
        let mut t = VirtualTable::new(vec![Column::new("k"), Column::new("id")]);
        t.set_rows(vec![
            vec!["x".into(), "first".into()],
            vec!["x".into(), "second".into()],
            vec!["a".into(), "third".into()],
        ]);
        t.sort_by(0, SortDir::Ascending);
        let ids: Vec<&str> = t.rows().iter().map(|r| r[1].as_str()).collect();
        assert_eq!(ids, vec!["third", "first", "second"]);
    }

    #[test]
    fn sort_by_with_uses_a_custom_comparator() {
        let mut t = VirtualTable::new(vec![Column::new("a")]);
        t.set_rows(vec![
            vec!["bb".into()],
            vec!["a".into()],
            vec!["ccc".into()],
        ]);
        t.sort_by_with(0, SortDir::Ascending, |x, y| x.len().cmp(&y.len()));
        assert_eq!(col0(&t), vec!["a", "bb", "ccc"]);
        assert_eq!(t.sort_state(), Some((0, SortDir::Ascending)));
    }

    fn headers(t: &VirtualTable) -> Vec<&str> {
        t.columns().iter().map(|c| c.header.as_str()).collect()
    }

    #[test]
    fn move_column_permutes_columns_and_every_row() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b"), Column::new("c")]);
        t.set_rows(vec![
            vec!["a0".into(), "b0".into(), "c0".into()],
            vec!["a1".into(), "b1".into(), "c1".into()],
        ]);
        t.move_column(0, 2); // a → end: [b, c, a]
        assert_eq!(headers(&t), ["b", "c", "a"]);
        assert_eq!(t.rows()[0], ["b0", "c0", "a0"]);
        assert_eq!(t.rows()[1], ["b1", "c1", "a1"]);

        t.move_column(2, 0); // a → front: [a, b, c]
        assert_eq!(headers(&t), ["a", "b", "c"]);
        assert_eq!(t.rows()[0], ["a0", "b0", "c0"]);
    }

    #[test]
    fn move_column_is_a_noop_for_invalid_or_equal_indices() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b")]);
        t.set_rows(vec![vec!["x".into(), "y".into()]]);
        t.move_column(0, 0); // equal
        t.move_column(0, 9); // out of range
        t.move_column(9, 0);
        assert_eq!(headers(&t), ["a", "b"]);
        assert_eq!(t.rows()[0], ["x", "y"]);
    }

    #[test]
    fn move_column_remaps_the_sort_column() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b"), Column::new("c")]);
        t.set_rows(vec![vec!["1".into(), "2".into(), "3".into()]]);
        t.sort_by(2, SortDir::Ascending); // sort column c (index 2)
        t.move_column(2, 0); // c moves to the front → sort follows to index 0
        assert_eq!(t.sort_state(), Some((0, SortDir::Ascending)));
    }

    #[test]
    fn remapped_index_tracks_a_move() {
        // move 0 → 2: 0↦2, 1↦0, 2↦1, 3 unchanged
        assert_eq!(VirtualTable::remapped_index(0, 2, 0), 2);
        assert_eq!(VirtualTable::remapped_index(0, 2, 1), 0);
        assert_eq!(VirtualTable::remapped_index(0, 2, 2), 1);
        assert_eq!(VirtualTable::remapped_index(0, 2, 3), 3);
        // move 2 → 0: 2↦0, 0↦1, 1↦2, 3 unchanged
        assert_eq!(VirtualTable::remapped_index(2, 0, 2), 0);
        assert_eq!(VirtualTable::remapped_index(2, 0, 0), 1);
        assert_eq!(VirtualTable::remapped_index(2, 0, 1), 2);
        assert_eq!(VirtualTable::remapped_index(2, 0, 3), 3);
    }
}
