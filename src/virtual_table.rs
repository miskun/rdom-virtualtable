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
//! **Not yet wired (see `STATE.md`):** automatic scroll → window
//! recomputation and a spacer so the scrollbar reflects the *total* row
//! count. For now the consumer drives `show_window` explicitly (e.g.
//! from a `scroll` listener), which is enough to avoid building the full
//! dataset.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rdom_tui::runtime::builtins::table::size_columns;
use rdom_tui::{Color, ListenerOptions, NodeId, Size, Stylesheet, TuiDom, TuiNodeMutExt, TuiStyle};

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

/// The table model: columns + row data. Holds no DOM state.
pub struct VirtualTable {
    columns: Vec<Column>,
    rows: Vec<Vec<String>>,
}

impl VirtualTable {
    pub fn new(columns: Vec<Column>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
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

        // Drop the previous window's rows (frees the arena slots).
        for id in self.mounted_rows.borrow_mut().drain(..) {
            let _ = dom.drop_subtree(id);
        }
        self.mounted_cells.borrow_mut().clear();

        let model = self.inner.borrow();
        let ncols = model.columns.len();
        let end = (start + count).min(model.rows.len());
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

        *self.mounted_rows.borrow_mut() = mounted;
        *self.mounted_cells.borrow_mut() = mounted_cells;
        self.window_start.set(start);

        if let Some(table) = self.table.get() {
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
    /// attributes. A plain move collapses any selection range. Returns `true`
    /// if the cursor actually moved.
    pub fn navigate(&self, dom: &mut TuiDom, nav: Nav) -> bool {
        self.nav_active.set(true);
        let Some((before, after)) = self.move_cursor(nav) else {
            return false;
        };
        self.selection.borrow_mut().collapse_range();
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

    /// Toggle the cursor's cell (or row, in `Row` mode) in the selection
    /// (`Space`).
    pub fn toggle_selection(&self, dom: &mut TuiDom) {
        let c = self.cursor.get();
        self.selection.borrow_mut().toggle(c.row(), c.col());
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
/// overpainted — then the cursor cell (`#2d2f31`, rdom's focus color) last, so
/// the cursor stays visible inside a selection.
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
    // #2d2f31 — the cursor cell, matching rdom's focus tint (inputs/tree).
    let cell = Color::Rgb(0x2d, 0x2f, 0x31);
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
        (
            ":where(table:focus td[data-active-cell])",
            TuiStyle::new().bg(cell),
        ),
    ]
}

/// A ready-made [`Stylesheet`] with the default cursor highlight from
/// [`highlight_rules`]. Drop it straight into `App::new`, or merge the rules
/// into your own sheet for custom colors.
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
}
