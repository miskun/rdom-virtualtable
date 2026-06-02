# rdom-virtualtable — Project State

Living journal for the virtualized-table crate.

## Thesis

A virtualized table built on rdom's **native `<table>`** — not a canvas paint surface. The model
holds all rows; only a visible window is ever materialized into `<tbody>`, so a dataset of any size
renders a bounded number of `<tr>` nodes. Built strictly on `rdom-tui`'s public API.

Split out of the original data-viz work (which also produced the `rdom-charts` crate): the table is
a different mechanism (element-tree materialization + windowing) from the canvas-painted charts, and
has its own substantial feature roadmap — so it lives in its own focused crate, free to evolve and
version independently.

## Shipped — virtualization core

- `VirtualTable` — column/row model + pure `window_for(viewport_rows, scroll_y, total) -> (start,
  count)`.
- `VirtualTableView` — `mount(dom)` builds `<table>` with a header + empty `<tbody>`;
  `show_window(dom, start, count)` materializes **only** that row slice (drops the previous window
  via `drop_subtree` so arena slots don't leak, then re-syncs column widths via the table builtin);
  `with(|t| …)` updates data; `mounted_row_count()` for assertions.
- Tests: 5 unit (window math + model) + 3 integration (only the window materializes against a
  1000-row model; show_window replaces the prior window; past-end renders header only).
- `examples/scroll_table.rs` — interactive 500-row scroll (j/k or arrows recompute the window).

## Roadmap (not yet done)

- Automatic scroll → window recompute + a spacer so the scrollbar reflects the *total* row count
  (today the consumer drives `show_window` explicitly, as the example shows).
- Sorting; row/cell selection; column resize / reorder / hide.
- Side-loaded data sources; persistence callbacks (sort/order/widths/hidden).

## Review gates

Run the Grumpy Chief Architect + Product/API passes at each milestone; record findings here.
