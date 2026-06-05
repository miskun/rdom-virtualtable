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
rdom-tui = "0.3.11"
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
view.install_mouse(dom);                                 // click/drag select + header-click sort
# }
```

`install_mouse` adds the pointer: **click a header** cycles its sort (asc → desc → off), **click a
cell** moves the cursor, **Shift+click** extends a range, **Ctrl/⌘+click** toggles a cell, and
**press-drag** rubber-bands a range. With a scrollable body (`enable_scrollbar`), **dragging past the
top/bottom edge autoscrolls** the window in and keeps the range growing to rows that weren't on
screen when the drag began — browser-style (rdom-tui ≥ 0.3.11's drag-autoscroll). (Selection gestures
need a selection mode — see below; clicks and sort work regardless.)

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

## Sort

Sort by a column — `toggle_sort` cycles ascending ⇄ descending, ideal for a header-click or key
handler:

```rust,ignore
# use rdom_virtualtable::{VirtualTableView, SortDir};
# use rdom_tui::TuiDom;
# fn demo(view: &VirtualTableView, dom: &mut TuiDom) {
view.toggle_sort(dom, 1);                 // sort by column 1 (asc, then flips)
view.sort(dom, 0, SortDir::Descending);   // …or sort explicitly
# }
```

The default comparator is **numeric-aware** (both cells parse as numbers → numeric, else
lexicographic) and **stable**; pass your own via `VirtualTable::sort_by_with`. The sorted header
gets **`data-sort="asc|desc"`** (style it however you like) plus a `▲`/`▼` glyph. Sorting clears the
selection (it's keyed by row index, which now points at different data).

The glyph is configurable via `set_sort_glyphs(asc, desc)` — `▲`/`▼` are East-Asian
ambiguous-width, so if your terminal renders ambiguous glyphs double-width (shifting later header
columns), set narrow ones, e.g. `view.set_sort_glyphs(" ^", " v")`, or `("", "")` to disable.

## Reorder columns

```rust,ignore
# use rdom_virtualtable::VirtualTableView;
# use rdom_tui::TuiDom;
# fn demo(view: &VirtualTableView, dom: &mut TuiDom) {
view.move_column(dom, 0, 2); // move column 0 to index 2
# }
```

`move_column` permutes the header and every row's cell, the cursor follows its column, and the sort
indicator stays on the moved column. (Like sort, it clears the selection.)

`set_column_hidden(dom, col, hidden)` hides/shows a column — it gets `data-vt-hidden` (the default
sheet maps that to `display: none`) on its header + cells, the cursor skips it on horizontal
navigation, and the hidden flag follows the column through reordering. Hiding the **last visible**
column is refused.

## Column-actions column (chooser)

`enable_column_actions(dom)` adds a persistent **`…` chip** as the trailing header cell. Clicking it
(or `toggle_column_menu(dom)` from a key) opens a **column chooser** — a checklist of *every* column,
built like HTML (a `<label>` wrapping a native `<input type="checkbox">`): **check to show, uncheck
to hide**. It's opt-in (a generic table shouldn't grow the affordance unasked) and self-contained —
the dropdown is an `position: absolute` + `z-index` panel anchored to the chip's own box, nothing
reaches outside the table subtree, so it drops into any layout.

While open the chooser **owns the keyboard** (modal, via `install_nav`): **↑ / ↓** (or `k` / `j`)
move the highlight, **Enter / Space** toggle the highlighted column, **Esc** closes — and the table
cursor is **frozen** so arrows don't leak to the cells behind it. It also dismisses on an **outside
click**. The table's cursor cross-hair + selection **step aside** while it's open (the default sheet
gates them on `:not([data-vt-menu-open])`), so focus rests on the chooser. Mouse toggling is the native checkbox (the `<label>` forwards the click); a `change`
listener reconciles the model. The highlighted row carries **`data-vt-menu-active`** and the open
chip **`data-vt-menu-open`** (filled with the panel's background so it reads as the panel's tab) —
restyle via those selectors. The chip is a header affordance, not a model column — it never affects
`columns()`, sort, widths, or the cursor.

> The column-actions column's body cells are reserved for **per-row action triggers** (edit / remove
> / open-in-… dropdowns) — a planned follow-up. Today only the header chooser is wired.

`set_column_width(dom, col, Some(w))` resizes a column to an explicit width (`None` returns it to
content-auto); `column_width(dom, col)` reads the current used width. On rdom-tui ≥ 0.3.6 the table
respects explicit widths, so it sticks across re-renders — and `Column::with_width` works.

## Scrollbar

```rust,ignore
# use rdom_virtualtable::VirtualTableView;
# use rdom_tui::TuiDom;
# fn demo(view: &VirtualTableView, dom: &mut TuiDom) {
view.set_viewport_rows(14);
view.enable_scrollbar(dom);  // native vertical scrollbar, thumb reflects ALL rows
# }
```

`enable_scrollbar` makes the `<tbody>` a vertical scroll container and brackets the window with
spacer rows, so the scroll thumb is proportional to the **total** row count while only the visible
window is materialized. Wheel / drag scroll is **decoupled** from the cursor (spreadsheet-style);
keyboard navigation scrolls the view to keep the cursor visible. Assumes uniform single-cell rows;
the draggable thumb spans the first ~65k rows (keyboard nav reaches the rest).

For **horizontal** scroll of a wide table, wrap it in a `Row`-flex `overflow-x: auto` container (the
TUI analogue of `<div style="overflow-x:auto"><table>`); header and body scroll together.

## Status

Shipped:

- **Virtualization core** — column/row model, pure `window_for` math, and `show_window`
  materialization (drops the previous window via `drop_subtree`, re-syncs column widths).
- **Keyboard navigation + cursor highlight** — a pure `GridCursor`, the `install_nav` keymap
  (arrows/`hjkl`, `g`/`G`/`Home`/`End`, `PageUp`/`PageDown`) with scroll-follow, and the
  `data-active-*` CSS highlight contract + a default focus-gated stylesheet.
- **Selection** — configurable `SelectionMode::{None, Cell, Row}`; Shift-range / Space-toggle /
  Ctrl-A / Esc; `data-selected` CSS contract + query API.
- **Sort** — `sort` / `toggle_sort` with a numeric-aware default comparator and a custom-comparator
  hook; `data-sort` header contract + a ▲/▼ glyph.
- **Column reorder / hide-show / resize** — `move_column`, `set_column_hidden`, `set_column_width`
  (+ `Column::with_width`); cursor + sort indicator follow, cursor skips hidden columns.
- **Column-actions column** — opt-in `enable_column_actions`; a persistent `…` chip whose dropdown is
  a native-checkbox **column chooser** (check to show, uncheck to hide; last column protected),
  modal keyboard nav, self-contained overlay. (Body-cell per-row actions: planned.)
- **Native scrollbar** — opt-in `enable_scrollbar`; proportional thumb (spacer rows), decoupled
  wheel/drag, cursor-follows-on-nav. Horizontal scroll via a `Row`-flex `overflow-x` wrapper.

Planned: side-loaded data sources; persistence. See `STATE.md`.

## License

MIT.
