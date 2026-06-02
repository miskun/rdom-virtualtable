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
use rdom_tui::{NodeId, Size, TuiDom, TuiNodeMutExt};

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
}

impl VirtualTableView {
    pub fn new(table: VirtualTable) -> Self {
        Self {
            inner: Rc::new(RefCell::new(table)),
            table: Rc::new(Cell::new(None)),
            tbody: Rc::new(Cell::new(None)),
            mounted_rows: Rc::new(RefCell::new(Vec::new())),
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
        for col in &model.columns {
            let th = dom.create_element("th");
            let text = dom.create_text_node(&col.header);
            dom.append_child(th, text).unwrap();
            if let Some(w) = col.width {
                dom.node_mut(th).set_width(Size::Fixed(w));
            }
            dom.append_child(header_tr, th).unwrap();
        }
        drop(model);

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

        let model = self.inner.borrow();
        let ncols = model.columns.len();
        let end = (start + count).min(model.rows.len());
        let mut mounted = Vec::with_capacity(end.saturating_sub(start));
        for row in &model.rows[start.min(model.rows.len())..end] {
            let tr = dom.create_element("tr");
            for c in 0..ncols {
                let td = dom.create_element("td");
                let cell = row.get(c).map(String::as_str).unwrap_or("");
                let text = dom.create_text_node(cell);
                dom.append_child(td, text).unwrap();
                dom.append_child(tr, td).unwrap();
            }
            dom.append_child(tbody, tr).unwrap();
            mounted.push(tr);
        }
        drop(model);

        *self.mounted_rows.borrow_mut() = mounted;

        if let Some(table) = self.table.get() {
            size_columns(dom, table);
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
