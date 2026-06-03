//! # rdom-virtualtable
//!
//! A virtualized table for [rdom](https://github.com/miskun/rdom): a
//! native `<table>` that materializes **only the visible row window**, so
//! a dataset of any size renders a bounded number of `<tr>` nodes.
//!
//! Unlike a charting component (which paints a `<canvas>`), this is a real
//! DOM subtree — `<table>` → `<thead>`/`<tbody>` → `<tr>` → `<th>`/`<td>` —
//! built strictly on rdom-tui's public API (the table builtin aligns
//! columns; this crate decides which rows to materialize).
//!
//! ```no_run
//! use rdom_virtualtable::{Column, VirtualTable, VirtualTableView};
//! use rdom_tui::TuiDom;
//!
//! let mut model = VirtualTable::new(vec![Column::new("id"), Column::new("name")]);
//! model.set_rows((0..10_000).map(|i| vec![i.to_string(), format!("row-{i}")]).collect());
//!
//! let view = VirtualTableView::new(model);
//! let mut dom = TuiDom::new();
//! let table = view.mount(&mut dom);       // <table> NodeId — append + size it
//! let (start, count) = VirtualTable::window_for(20, 0, view.with(|t| t.row_count()));
//! view.show_window(&mut dom, start, count); // materialize just that slice
//! # let _ = table;
//! ```
//!
//! ## Keyboard navigation + highlight
//!
//! A logical cursor ([`GridCursor`]) moves an active `(row, col)` over the
//! *whole* dataset; the view scrolls to keep it visible and re-materializes
//! the window as needed. Wire the built-in keymap with
//! [`VirtualTableView::install_nav`] (arrows + `hjkl`, `g`/`G` / `Home`/`End`,
//! `PageUp`/`PageDown`), or drive [`VirtualTableView::navigate`] from your own
//! keymap over [`Nav`] / [`nav_for_key`].
//!
//! The cursor is reflected as **presence attributes** so CSS owns the look —
//! the view never hard-codes colors:
//!
//! - `data-active-row` on the `<tr>` under the cursor,
//! - `data-active-col` on every `<th>`/`<td>` in the cursor's column,
//! - `data-active-cell` on the single `<td>` at the cursor.
//!
//! [`highlight_stylesheet`] gives a ready-made, **focus-gated** cross-hair
//! (the highlight only shows while the table is focused); [`highlight_rules`]
//! exposes the same `(selector, style)` pairs to fold into your own sheet.
//! The default rules are wrapped in `:where()` so they carry **zero
//! specificity** — any author rule of any specificity overrides them, exactly
//! like overriding a browser UA style (no `table:focus` prefix or specificity
//! matching needed). Requires rdom-tui ≥ 0.3.4.

mod grid_cursor;
mod virtual_table;

pub use grid_cursor::{GridCursor, Nav, nav_for_key};
pub use virtual_table::{
    Column, VirtualTable, VirtualTableView, highlight_rules, highlight_stylesheet,
};
