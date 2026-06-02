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

mod virtual_table;

pub use virtual_table::{Column, VirtualTable, VirtualTableView};
