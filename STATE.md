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
  **Shift+arrows** (extend), **Space** (toggle), **Ctrl-A** (all), **Esc** (clear)
  when a mode is set. **Space over a live Shift-range commits the whole rectangle** into the sticky
  toggled set (and collapses the range) — Shift-select → Space → move → Shift-select → Space builds
  multiple persistent ranges, the standard-friendly answer to multi-range keyboard selection (a true
  toggle: an already-selected range is removed). With no live range, Space toggles the cursor
  cell/row. **A plain (unmodified) arrow collapses the *transient* selections** — an
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

## Shipped — column reorder (M3, part 2)

Move a column, consumer-first — no DOM-node surgery (the `<th>` nodes stay put; their text is
reassigned, data flows through fixed positions, exactly like the cells).

- `VirtualTable::move_column(from, to)` — permutes `columns` + **every row's cell** by the same
  move, and remaps the recorded sort column so the sort follows. `remapped_index(from, to, i)` is
  the pure index map (also used to move the cursor). No-op for out-of-range/equal indices.
- `VirtualTableView::move_column(dom, from, to)` — mutates the model, **moves the cursor with its
  column** (`GridCursor::at`), **clears the selection** (a structural change, like sort), re-syncs
  the header labels/glyph (`apply_sort_indicator`), and re-materializes the window. The header
  `<th>` nodes are *not* moved — their text is reassigned — so node identity + listeners survive.
- Tests: +5 unit (permutes cols+rows, no-op guards, remaps sort col, `remapped_index` map) and +3
  integration (headers+cells reorder, sort indicator follows the moved column, selection cleared +
  cursor follows). **Total: 64 (37 unit + 26 integration + 1 doctest).**
- `examples/scroll_table.rs`: **`[`** / **`]`** move the cursor's column left / right.

## Shipped — sort (M3, part 1)

Model-side sort + a CSS-contract header indicator, consumer-first.

- `VirtualTable`: `sort_by(col, dir)` (default comparator — **numeric-aware** when both cells parse
  as numbers, else lexicographic; **stable**), `sort_by_with(col, dir, cmp)` (the **sort hook** — a
  custom `Fn(&str, &str) -> Ordering`), `sort_state()`, and `rows()` accessor. `SortDir {Ascending,
  Descending}` + `flipped()`.
- `VirtualTableView`: `sort(dom, col, dir)`, `toggle_sort(dom, col)` (asc the first time, then
  asc⇄desc), `sort_state()`, and `refresh(dom)` (re-materialize the current window after any model
  mutation). Sorting **clears the selection** — it's keyed by row index, which points at different
  data after a reorder (a row-identity-keyed selection that survives sort is future work).
- **Sort contract:** `data-sort="asc|desc"` on the sorted `<th>` (the CSS hook), plus a `▲`/`▼`
  glyph. **The glyph is rendered as header *text*, not the cleaner `th[data-sort]::after` CSS** —
  because the substrate's `table::size_columns` measures only text-node width (and runs before
  cascade), so an `::after` glyph is clipped by the auto-computed column width. The `::after`
  approach works in isolation (verified) but not under `size_columns`. → **substrate-friction item
  below.**
- **Configurable glyph** via `set_sort_glyphs(asc, desc)` (default `(" ▲", " ▼")`). `▲`/`▼` are
  East-Asian *ambiguous-width*; a terminal that renders ambiguous glyphs double-width would shift
  later header columns — set narrow glyphs (`" ^"`/`" v"`) or `""` to avoid it.
- **Stale-header layout fix — now substrate-owned (rdom-tui ≥ 0.3.5).** `size_columns` writes column
  widths via `inline_style` without dirtying the cells, and the `<thead>` headers sit outside the
  `<tbody>` subtree `show_window` rebuilds — so under the runtime's **incremental (subtree) cascade**
  a sorted header kept a *stale computed width* (visible shift, fixed only by a later mutation like
  navigating right). Originally worked around here by stamping `data-vt-rev` on the `<table>`; the
  root cause was fixed upstream (`TABLE-COLSYNC-DIRTY-1`: `size_columns` itself stamps a column-width
  signature when widths change), so the consumer-side hack was **removed** on the bump to 0.3.5.
- Tests: +5 unit (sort both directions + state, numeric-aware, stable, custom comparator) and a new
  `tests/render_sort.rs` (+4 integration: reorders the window, marks/toggles the header + column,
  clears selection, glyph paints). **Total: 57 (33 unit + 23 integration + 1 doctest).**
- `examples/scroll_table.rs`: press **`s`** to sort the cursor's column (toggles asc⇄desc).

## Shipped — native vertical scrollbar (M4)

Opt-in (`enable_scrollbar`), built entirely on the existing rdom-tui scroll substrate — **no new
substrate API** (the grumpy-architect call: reject a "declared virtual scroll extent"; use the
web's spacer technique + the standard `scrollTop` accessor).

- `VirtualTableView::enable_scrollbar(dom)` — makes the `<tbody>` a vertical `overflow-y: auto`
  scroll container `viewport_rows` tall, and `show_window` brackets the window with spacer `<tr>`s
  (`data-rdom-spacer`, height-only, marked so consumer CSS + the highlight pass skip them) so the
  `<tbody>` scroll extent equals the **total** row count → the thumb is proportional while only the
  window is materialized.
- **Decoupled (spreadsheet-style):** a `scroll` listener re-windows on wheel/drag without touching
  the cursor; cursor navigation writes `scroll_top` (one write direction, no re-entrancy guard) so
  the listener re-windows + the cursor scrolls back into view. `first_row = scroll_top` (the
  `<thead>` is outside the scroll container — no sticky, so header/body columns stay aligned).
- **Assumes uniform single-cell rows**; the draggable thumb spans the first ~65k rows (`u16` spacer
  height), keyboard nav reaches the rest. Sticky-header was abandoned: the consumer can't set
  `position: sticky; top: 0` via the public `TuiStyle` (`Length` unexported, no `top()` builder) —
  the tbody-scroll design sidesteps it (prototype-validated before building).
- **Horizontal scroll** of a wide table: wrap it in a `Row`-flex `overflow-x` container (the web
  `<div overflow-x:auto>` pattern); a `<table>` can't be its own cross-axis scroll container
  (rdom `SCROLL-CROSS-AXIS-1`). No component code needed.
- Tests: `tests/render_scrollbar.rs` (+4: extent reflects total, decoupled re-window, cursor
  scrolls into range, spacers marked/excluded). `examples/scroll_table.rs` opts in.

## Roadmap (not yet done)

- **Column ops (remaining):** column *hide/show* (consumer-side, like reorder). Column *resize*
  needs custom layout → an rdom substrate ask.
- **Substrate-friction backlog (promote to rdom when hit):**
  - `table::size_columns` ignores generated `::before`/`::after` content width and runs pre-cascade —
    so a CSS `::after` sort glyph is clipped (we render the glyph as header text instead). Once it
    measures pseudo width post-cascade, move the glyph to a `th[data-sort]::after` default rule.
  - **`TABLE-COLSYNC-DIRTY-1` — RESOLVED in rdom-tui 0.3.5.** `size_columns` now stamps a
    column-width signature so resized cells re-cascade; the `data-vt-rev` hack was removed.
  - **Scrollbar spacer (total-row extent) + horizontal scroll — DONE** (above) on the existing scroll
    substrate, no new rdom API. Column *resize-by-width* still needs custom layout → substrate ask.
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

### M3 gate — column ops (sort + reorder)

- **Architect:** Pure model ops (`sort_by` / `sort_by_with` / `move_column` / `remapped_index`) are
  isolated and unit-tested incl. edge cases (numeric-aware, stable, no-op guards, sort-col remap);
  the view layer is thin orchestration (mutate model → re-sync headers → `refresh`). `refresh` is a
  good single re-render primitive shared by sort/reorder/`with`. Gate clean (fmt / clippy -D warnings
  / 64 tests).
  - *Non-blocking — destructive, O(n) model mutation.* Both sort (`rows.sort_by`, O(n log n)) and
    reorder (`move_column`, O(rows) cell shuffles) **physically reorder the row data**, so the
    original order is lost and a reorder touches every row. Fine for explicit, infrequent actions at
    current scale; for very large datasets a non-destructive **column display-permutation** (O(cols),
    rows untouched) would be cheaper and reversible. Recorded as a future option, not a fix now.
  - *Non-blocking — `VirtualTableView` is growing.* mount + window + nav + selection + sort + reorder
    + highlight now live on one struct (~450 lines). Still coherent, but the next column feature
    (hide/resize) should prompt splitting out a column-ops and/or highlight module.
  - *Non-blocking — substrate friction (recorded above).* `size_columns` ignores `::after` width and
    runs pre-cascade, forcing the sort glyph into header text instead of a pure-CSS `::after` default.
- **API:** Sort surface is clean and contract-first: `data-sort="asc|desc"` mirrors `aria-sort`, the
  numeric-aware default "just works", and `sort_by_with` is the documented hook. `toggle_sort` /
  `move_column` are one-call header handlers; cursor + sort indicator follow a moved column, which is
  the intuitive behavior. `rows()` + `refresh()` give consumers a clean read + re-render path.
  - *Non-blocking — selection cleared on sort/reorder.* Honest (selection is row-index-keyed) and
    documented, but a spreadsheet-style consumer may expect selection to follow the data. Revisit
    with a row-identity-keyed selection.
  - *Non-blocking — glyph in `text_content`.* `th.text_content()` returns e.g. `"c0 ▲"`; the model's
    `Column.header` stays clean. Acceptable; resolves once the glyph moves to `::after`.
  - No blocking findings. M2 (selection) was not formally gated — its contract is covered by tests
    and the M3 review touched its sort/reorder interactions.

### M4 gate — native vertical scrollbar

Review was **front-loaded**: a grumpy critique of the *plan* (recorded in the chat decision log)
caught the worst issues before any feature code, and a gating prototype validated the design.

- **Architect:** The plan's blocking findings were all addressed before building:
  - *Two-sources-of-truth (would-be blocker) — resolved.* In scroll-mode `scroll_top` is the single
    source; `window_start` derives from it; the cursor's `scroll` field is only its private follow
    input (synced to `scroll_top` on cursor move, allowed to diverge on wheel — that *is* decoupling).
  - *Re-entrancy (would-be blocker) — designed out.* One write direction: the `scroll` listener
    re-windows but never writes `scroll_top`; only cursor nav writes it. No guard needed.
  - *Sticky-header design (planned) — abandoned after prototyping.* The consumer can't set
    `position: sticky; top: 0` via the public `TuiStyle` (`Length` unexported, no `top()` builder) —
    a real substrate gap the prototype surfaced. Pivoted to tbody-as-scroll-container (thead static
    outside), which needs no sticky AND keeps header/body aligned (auto-overflow reserves no gutter).
    Prototyping the actual variant (not the assumed one) is what caught this.
  - *Forced height (non-blocking).* `enable_scrollbar` fixes the `<tbody>` height to `viewport_rows`
    (the scroll viewport) rather than the whole table — more defensible than the planned table-height
    force, but still a fixed height; flex-fill (read the laid-out height) is the future end-state.
  - *Uniform 1-cell rows + `u16` spacer (~65k)* — documented limits, not silent. Keyboard nav
    (unbounded) covers beyond 65k; wrapped rows break the mapping (documented).
- **API:** `enable_scrollbar(dom)` is one opt-in call; decoupled wheel/drag + cursor-follows-on-nav
  matches spreadsheet expectations. No new rdom substrate API (spacer technique + standard
  `scroll_top`) — the grumpy-architect call against a "declared virtual scroll extent" held up.
  Spacers are marked (`data-rdom-spacer`) so they never carry highlight/selection or catch consumer
  `tr` styles. Gate clean (fmt / clippy -D warnings / all suites + 4 new scrollbar tests).
  - *Non-blocking — `VirtualTableView` keeps growing* (now + scroll). The hide/resize work should
    finally split a module (carried from the M3 gate).
