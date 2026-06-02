# rdom-virtualtable

A **virtualized table** for [rdom](https://github.com/miskun/rdom), the browser-faithful DOM for
terminal applications.

It's a real DOM subtree — native `<table>` → `<thead>`/`<tbody>` → `<tr>` → `<th>`/`<td>` — that
materializes **only the visible row window**. A dataset of any size renders a bounded number of
`<tr>` nodes; scrolling swaps which slice is in the DOM. Built strictly on `rdom-tui`'s public API
(the table builtin aligns columns; this crate decides which rows to materialize).

## Install

```toml
[dependencies]
rdom-virtualtable = "0.1"
rdom-tui = "0.3"
```

## Try it

```bash
cargo run --example scroll_table   # scroll a 500-row table (j/k or ↑/↓), Ctrl-C to quit
```

## Example

```rust
use rdom_virtualtable::{Column, VirtualTable, VirtualTableView};
use rdom_tui::TuiDom;

let mut model = VirtualTable::new(vec![Column::new("id"), Column::new("name")]);
model.set_rows((0..10_000).map(|i| vec![i.to_string(), format!("row-{i}")]).collect());

let view = VirtualTableView::new(model);
let mut dom = TuiDom::new();
let table = view.mount(&mut dom);          // <table> NodeId — append + size it

// Show a 20-row window starting at the current scroll offset.
let (start, count) = VirtualTable::window_for(20, /* scroll_y */ 0, view.with(|t| t.row_count()));
view.show_window(&mut dom, start, count);  // only these rows are materialized
```

## Status

Shipped: the **virtualization core** — column/row model, pure `window_for` math, and
`show_window` materialization (drops the previous window via `drop_subtree`, re-syncs column
widths).

Planned: sorting, row/cell selection, column resize/reorder/hide, a scrollbar spacer + automatic
scroll→window recompute, side-loaded data sources, persistence. See `STATE.md`.

## License

MIT.
