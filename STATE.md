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

## Shipped — keyboard navigation + cursor highlight (M1)

Ported from the lens-k8s-tui table best practices, *consumer-first*: built entirely on rdom-tui
0.3's public API (keyboard events, attributes, CSS), no substrate changes. lens implements this as
a native `<vtable>` builtin in its rdom fork (custom layout/paint owns scrollbar, h-scroll, column
mode); we deliberately took the incremental path — nav + CSS highlight now, and any feature that
genuinely needs custom layout/paint (scrollbar, horizontal scroll, column resize) becomes a focused,
documented rdom enhancement when we hit it (the same loop that drove rdom 0.3.0–0.3.2).

- `GridCursor` (pure, `Copy`) — active `(row, col)` + `scroll` over the full dataset; clamped
  `navigate(nav, rows, cols, page)` and a `follow(viewport, rows)` scroll-into-view. `Nav` intent
  enum + `nav_for_key(key, shift)` keymap (arrows + `hjkl`, `g`/`G` & `Home`/`End` = first/last row,
  `PageUp`/`PageDown`). Shift is reserved for range selection (M2) → currently `None`.
- `VirtualTableView`: `set_viewport_rows`, `cursor()`, `navigate(dom, nav)` (moves cursor, scrolls,
  re-windows only when the slice shifts, else cheap attribute re-paint), and `install_nav(dom,
  table, viewport_rows)` (attaches a `keydown` listener over the built-in keymap, `prevent_default`
  + `request_redraw`). `show_window` now records header/cell node ids + window start and reasserts
  the highlight — gated by `nav_active` so pure-virtualization consumers never get `data-active-*`.
- **Highlight contract (durable — also in CLAUDE.md):** the cursor is reflected as *presence
  attributes*, never hard-coded colors — `data-active-row` (`<tr>`), `data-active-col`
  (`<th>`/`<td>` in the column), `data-active-cell` (the cursor `<td>`). `highlight_stylesheet()` /
  `highlight_rules()` provide a default **focus-gated** cross-hair (`table:focus tr[data-active-row]
  { … }`) so the highlight only shows while the table is focused, and the cell rule is listed last
  so it wins over the column rule on the crossing cell (equal specificity → source order).
- Tests: +7 unit (cursor moves/clamp/follow + keymap) and +3 integration (attributes mark the right
  row/col/cell incl. header; nav past the window shifts + re-highlights while staying bounded;
  highlight is focus-gated at paint). **Total: 22 (15 unit + 6 integration + 1 doctest).**
- `examples/scroll_table.rs` — now a navigable 500-row demo (`install_nav` + `highlight_stylesheet`,
  with a `table:focus { background: reset }` rule to suppress the generic focus tint so only the
  cross-hair shows). Live `row · col` read-out in the title.

## Roadmap (not yet done)

- **M2 — selection:** shift+arrows rectangular range, `Space` toggle, `Ctrl-A` select-all, `Esc`
  clear → `data-selected` attributes + a selection query API (web-faithful grid multi-select).
- **M3 — column ops:** reorder (DOM swap, doable consumer-side) and sort hook. Column *resize* needs
  custom layout → flag as an rdom substrate ask.
- **Substrate-friction backlog (promote to rdom when hit):** scrollbar spacer reflecting the *total*
  row count, horizontal scroll, column resize-by-width — each needs custom layout/paint a downstream
  crate can't do; document and prioritize as focused rdom enhancements.
- Side-loaded data sources; persistence callbacks (sort/order/widths/hidden).

## Review gates

Run the Grumpy Chief Architect + Product/API passes at each milestone; record findings here.

### M1 gate — keyboard nav + highlight

- **Architect:** Pure cursor math is isolated and exhaustively unit-tested; DOM wiring is a thin
  reflect-cursor-onto-attributes pass. `nav_active` gate keeps the feature opt-in so existing
  virtualization consumers are untouched. Highlight re-paint is O(window), not O(dataset). No
  substrate changes, no new deps. Non-blocking: `navigate` re-windows via full drop+rebuild on slice
  shift (fine at these sizes; an overscan buffer would cut churn — deferred with the scrollbar work).
- **API:** Cursor reflected as presence attributes = CSS owns the look (no baked colors), matching
  the lens/`<tree>` pattern; focus-gated default means zero-config-correct out of the box and fully
  overridable. `install_nav` is one call; `navigate` + `Nav` + `nav_for_key` let consumers BYO
  keymap. Gate clean (fmt / clippy -D warnings / 22 tests). No blocking findings.
