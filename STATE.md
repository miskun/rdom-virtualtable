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
  `highlight_rules()` provide a default **focus-gated** cross-hair so the highlight only shows while
  the table is focused, and the cell rule is listed last so it wins over the column rule on the
  crossing cell (source order).
- **Defaults are `:where()`-wrapped → zero specificity (requires rdom-tui ≥ 0.3.3).** The override
  question — "why is overriding our defaults harder than overriding browser UA styles?" — has a real
  answer: browsers sort the cascade by *origin* (Author beats UA for free), but a downstream crate
  can only emit Author-origin rules, so defaults + app rules fought on specificity. Rather than
  invent a non-web origin tier, we drove the **web-faithful fix into the substrate**: rdom 0.3.3
  added `:where()` (Selectors L4 — matches like `:is()`, contributes zero specificity). The default
  rules now wrap their `table:focus …` selectors in `:where(…)`, so *any* author rule (even a plain
  `td[data-active-cell] {}`) overrides them with no specificity fight — exactly like a browser UA
  default. Tested end-to-end (`consumer_css_overrides_default_colors`). This is the
  "promote-friction-to-substrate" loop in action (cf. rdom 0.3.0–0.3.2).
- Tests: +7 unit (cursor moves/clamp/follow + keymap) and +3 integration (attributes mark the right
  row/col/cell incl. header; nav past the window shifts + re-highlights while staying bounded;
  highlight is focus-gated at paint). **Total: 22 (15 unit + 6 integration + 1 doctest).**
- `examples/scroll_table.rs` — a navigable 500-row demo (`install_nav` + `highlight_stylesheet`),
  live `row · col` read-out in the title.

## Shipped — rdom-tui 0.3.4 bump (focus vocabulary)

Bumped `rdom-tui = "0.3.4"`, which ships rdom's `FOCUS-VOCAB-1`: the UA focus tint is now scoped to
interactive controls, so a focused `<table>` is no longer washed with the focus background. Dropped
the `table:focus { background: reset }` workaround from the example and the test helper — it's a
no-op now. New regression test `focused_table_needs_no_focus_tint_reset` proves rendering with vs
without the reset is identical (a focused table isn't tinted). The cursor cross-hair (data-active-*
+ `:where()` defaults) is unchanged. (The cursor cell is `#2d2f31` gray by default, turning blue
  `#3a6ea5` only when it's itself selected — see the selection entry below.)
**25 tests (15 unit + 9 integration + 1 doctest).**

## Shipped — selection (M2)

Configurable, consumer-side, same attribute-contract pattern as the cursor.

- `selection.rs` — pure, unit-tested `GridSelection` + `SelectionMode {None (default/off), Cell,
  Row}`. Selection is the *union* of a rectangular range (shift-anchor → cursor head), a toggled
  set, and select-all; `is_selected(row, col)` is mode-aware (Row mode ignores the column). 9 unit
  tests.
- `VirtualTableView`: `set_selection_mode` / `selection_mode` / `selection()` (query snapshot), and
  `extend_selection` / `toggle_selection` / `select_all` / `clear_selection`. `install_nav` routes
  **Shift+arrows** (extend), **Space** (toggle cursor cell/row), **Ctrl-A** (all), **Esc** (clear)
  when a mode is set. **A plain (unmodified) arrow collapses the *transient* selections** — an
  in-progress Shift-range **and** a Ctrl-A select-all (`collapse_transient`) — matching every
  spreadsheet/grid; the explicitly **Space-toggled set survives** until Esc (it's the keyboard
  stand-in for Ctrl+click, so collapsing it would make discontiguous keyboard selection unusable).
  `apply_highlight` now also writes
  **`data-selected`** on each selected `<td>` (and the `<tr>` of a selected row), gated by
  `nav_active` like the cursor.
- **Selection contract:** `data-selected` presence attributes; default `:where()`-wrapped,
  focus-gated blue (`#1e3a5f`) fill in `highlight_rules`. A selected cell that also sits in the
  active row/column gets a brighter `#2b557e` (pre-computed "selection over the cross-hair" blend —
  a TUI can't alpha-composite opaque cells, so the highlight shows through instead of being flatly
  overpainted). The cursor cell wins last so it stays visible inside a selection: `#2d2f31` gray by
  default, switching to the brightest blue `#3a6ea5` only when the cursor cell is *itself* selected
  (keyed `td[data-active-cell][data-selected]`), so it fits the surrounding blue field instead of
  reading as an odd gray patch.
  Precedence is pure source order (all zero-specificity `:where()`). Fully overridable.
- Tests: +6 integration (cell rect, whole-row, toggle, select-all/clear, none-mode no-op, and a
  focus-gated selection *paint* test). **Total: 40 (24 unit + 15 integration + 1 doctest).**
- `examples/scroll_table.rs` opts into `SelectionMode::Cell` + an updated keymap read-out.

## Roadmap (not yet done)

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
