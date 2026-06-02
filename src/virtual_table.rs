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

    /// Apply a navigation move to the cursor, scroll to keep it visible,
    /// re-materialize the window if it shifted, and update the
    /// `data-active-*` highlight attributes. Returns `true` if the cursor
    /// actually moved.
    ///
    /// Engages navigation (so the highlight contract is honored from here
    /// on) even if the move is a clamped no-op at a boundary.
    pub fn navigate(&self, dom: &mut TuiDom, nav: Nav) -> bool {
        self.nav_active.set(true);
        let (rows, cols) = self.with(|t| (t.row_count(), t.columns().len()));
        if rows == 0 || cols == 0 {
            return false;
        }
        let viewport = self.viewport_rows.get() as usize;
        let page = viewport.max(1);

        let before = self.cursor.get();
        let after = before
            .navigate(nav, rows, cols, page)
            .follow(viewport, rows);
        self.cursor.set(after);

        // Re-window only when the visible slice actually shifts; otherwise a
        // cheap attribute re-paint is enough.
        if viewport > 0 && after.scroll() != self.window_start.get() {
            let (start, count) = VirtualTable::window_for(viewport as u16, after.scroll(), rows);
            self.show_window(dom, start, count); // re-applies highlight
        } else {
            self.apply_highlight(dom);
        }
        after != before
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
            if let Some(nav) = nav_for_key(&kbd.key, kbd.modifiers.shift) {
                view.navigate(ctx.dom, nav);
                ctx.event.prevent_default();
                ctx.request_redraw();
            }
        })
        .expect("table node accepts a keydown listener");
        // Reflect the initial cursor onto whatever window is already shown.
        self.apply_highlight(dom);
    }

    /// Write `data-active-row` / `data-active-col` / `data-active-cell`
    /// presence attributes onto the materialized window + header to match the
    /// cursor. Clears them everywhere they no longer apply, so a single pass
    /// both sets and unsets.
    fn apply_highlight(&self, dom: &mut TuiDom) {
        let cursor = self.cursor.get();
        let start = self.window_start.get();

        for (c, &th) in self.header_cells.borrow().iter().enumerate() {
            set_flag(dom, th, "data-active-col", c == cursor.col());
        }

        let rows = self.mounted_rows.borrow();
        let cells = self.mounted_cells.borrow();
        for (i, &tr) in rows.iter().enumerate() {
            let row_active = start + i == cursor.row();
            set_flag(dom, tr, "data-active-row", row_active);
            if let Some(row_cells) = cells.get(i) {
                for (c, &td) in row_cells.iter().enumerate() {
                    let col_active = c == cursor.col();
                    set_flag(dom, td, "data-active-col", col_active);
                    set_flag(dom, td, "data-active-cell", row_active && col_active);
                }
            }
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
/// The cell rule is listed last so it wins over the column rule on the
/// crossing cell (equal specificity → source order decides).
pub fn highlight_rules() -> Vec<(&'static str, TuiStyle)> {
    vec![
        (
            "table:focus tr[data-active-row]",
            TuiStyle::new().bg(Color::Indexed(236)),
        ),
        (
            "table:focus th[data-active-col]",
            TuiStyle::new().bg(Color::Indexed(238)),
        ),
        (
            "table:focus td[data-active-col]",
            TuiStyle::new().bg(Color::Indexed(238)),
        ),
        (
            "table:focus td[data-active-cell]",
            TuiStyle::new()
                .bg(Color::Indexed(33))
                .fg(Color::Indexed(231)),
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
