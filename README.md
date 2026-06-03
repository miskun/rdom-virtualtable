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
rdom-tui = "0.3.4"
```

## Try it

```bash
cargo run --example scroll_table   # navigate a 500-row table (arrows/hjkl, g/G), Ctrl-C to quit
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

## Keyboard navigation

A logical cursor moves an active `(row, col)` over the whole dataset; the view scrolls to keep it
visible and re-materializes the window as needed. Wire the built-in keymap in one call:

```rust
# use rdom_virtualtable::{VirtualTableView, highlight_stylesheet};
# use rdom_tui::{TuiDom, NodeId};
# fn demo(view: &VirtualTableView, dom: &mut TuiDom, table: NodeId) {
dom.node_mut(table).set_attribute("tabindex", "0").ok(); // focusable
view.install_nav(dom, table, /* visible rows */ 14);     // arrows/hjkl, g/G, PageUp/Down
# }
```

The cursor is reflected as **presence attributes** — `data-active-row` on the cursor's `<tr>`,
`data-active-col` on its column's cells, `data-active-cell` on the cursor cell — so **CSS owns the
look**. `highlight_stylesheet()` is a ready-made, focus-gated cross-hair (it only paints while the
table is focused); `highlight_rules()` exposes the same `(selector, style)` pairs to recolor.

The default rules are wrapped in `:where()`, so they carry **zero specificity** — any author rule
overrides them with no specificity fight, exactly like overriding a browser UA default:

```rust,ignore
// Recolor the cursor cell — a plain selector wins over the zero-specificity default.
let sheet = highlight_stylesheet()
    .rule("td[data-active-cell]", TuiStyle::new().bg(Color::Rgb(0x33, 0x55, 0x88)))
    .unwrap();
```

## Selection (configurable)

Selection is **off by default**. Opt in with `set_selection_mode`:

```rust,ignore
# use rdom_virtualtable::{VirtualTableView, SelectionMode};
# fn demo(view: &VirtualTableView) {
view.set_selection_mode(SelectionMode::Cell); // rectangular cell ranges
// view.set_selection_mode(SelectionMode::Row);  // …or whole rows
# }
```

With a mode set, `install_nav` also wires **Shift+arrows** (extend a range), **Space** (toggle),
**Ctrl-A** (select all), **Esc** (clear). **Space over a live Shift-range commits the whole
rectangle** into the persistent selection (then collapses the range), so Shift-select → Space →
move → Shift-select → Space builds multiple ranges by keyboard; with no live range Space toggles the
cursor cell/row. A plain (unmodified) arrow **collapses
the transient selections** — an in-progress Shift-range and a Ctrl-A select-all — like any
spreadsheet; the explicitly **Space-toggled cells stay** until Esc, so you can navigate between
cells to build a discontiguous selection by keyboard. Selected cells get **`data-selected`** (and
the `<tr>` of any selected row) — same focus-gated, `:where()`-defaulted, fully-overridable CSS
contract as the cursor. Query it with `view.selection().is_selected(row, col)`.

## Status

Shipped:

- **Virtualization core** — column/row model, pure `window_for` math, and `show_window`
  materialization (drops the previous window via `drop_subtree`, re-syncs column widths).
- **Keyboard navigation + cursor highlight** — a pure `GridCursor`, the `install_nav` keymap
  (arrows/`hjkl`, `g`/`G`/`Home`/`End`, `PageUp`/`PageDown`) with scroll-follow, and the
  `data-active-*` CSS highlight contract + a default focus-gated stylesheet.
- **Selection** — configurable `SelectionMode::{None, Cell, Row}`; Shift-range / Space-toggle /
  Ctrl-A / Esc; `data-selected` CSS contract + query API.

Planned: sorting; column resize / reorder / hide; a scrollbar spacer; side-loaded data sources;
persistence. See `STATE.md`.

## License

MIT.
