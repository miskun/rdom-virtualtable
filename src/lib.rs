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
//!
//! ## Selection (configurable; off by default)
//!
//! Opt in with [`VirtualTableView::set_selection_mode`] —
//! [`SelectionMode::Cell`] (rectangular cell ranges) or [`SelectionMode::Row`]
//! (whole rows); [`SelectionMode::None`] (default) disables it. With a mode
//! set, [`install_nav`](VirtualTableView::install_nav) also wires
//! **`Shift`+arrows** (extend a range), **`Space`** (toggle), **`Ctrl-A`**
//! (select all), and **`Esc`** (clear). `Space` over a live `Shift`-range
//! commits the whole rectangle into the sticky set (then collapses the range),
//! so Shift-select → `Space` → move → … builds multiple ranges; with no live
//! range it toggles the cursor cell/row. A plain arrow collapses the
//! *transient* selections (an in-progress range and a `Ctrl-A` select-all),
//! like any spreadsheet; the explicitly `Space`-toggled set survives until
//! `Esc`. Selection is
//! reflected as **`data-selected`** on each selected `<td>` (and the `<tr>` of
//! any row with a selection), styled by the same focus-gated, `:where()`
//! defaults; query it with [`VirtualTableView::selection`] →
//! [`GridSelection::is_selected`].
//!
//! ## Sort
//!
//! [`VirtualTableView::sort`] / [`toggle_sort`](VirtualTableView::toggle_sort)
//! sort by a column ([`SortDir`]). The default comparator is numeric-aware
//! (both cells parse as numbers → numeric, else lexicographic) and stable;
//! [`VirtualTable::sort_by_with`] takes a custom comparator (the sort hook).
//! The sorted header carries **`data-sort="asc|desc"`** (the CSS contract) plus
//! a `▲`/`▼` glyph. Sorting clears the selection (it's keyed by row index).
//!
//! ## Column reorder
//!
//! [`VirtualTableView::move_column`] moves a column, permuting the header and
//! every row's cell; the cursor and the sort indicator follow the moved
//! column. Like sort, it clears the selection.

mod grid_cursor;
mod selection;
mod virtual_table;

pub use grid_cursor::{GridCursor, Nav, nav_for_key};
pub use selection::{GridSelection, SelectionMode};
pub use virtual_table::{
    Column, SortDir, VirtualTable, VirtualTableView, highlight_rules, highlight_stylesheet,
};
