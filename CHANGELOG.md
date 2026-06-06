# Changelog

All notable changes to `rdom-virtualtable` are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0]

First release: a virtualized table for [rdom](https://github.com/miskun/rdom),
built strictly on `rdom-tui`'s public API. A native `<table>` subtree that
materializes only the visible row window, so a dataset of any size renders a
bounded number of `<tr>` nodes.

### Added

- **Virtualization core** — `VirtualTable` (column/row model), pure
  `window_for(viewport, scroll, total)` math, and `show_window` materialization
  (drops the previous window, re-syncs column widths).
- **Typed cells** — `CellValue` (`Empty` / `Text` / `Number` / `Bytes` /
  `Duration` / `Status`); a bare `&str`/`String` converts to `Text`.
- **Keyboard navigation + cursor highlight** — a pure `GridCursor`, the
  `install_nav` keymap (arrows/`hjkl`, `g`/`G`/`Home`/`End`, `PageUp`/`PageDown`)
  with scroll-follow, and the `data-active-*` CSS highlight contract with a
  default focus-gated (`:focus-within`) stylesheet.
- **Selection** — configurable `SelectionMode::{None, Cell, Row}`; Shift-range /
  Space-toggle / Ctrl-A / Esc; `data-selected` CSS contract. Keyed by **row
  identity** (`RowKey`), so it survives sort / scroll / live updates; `Ctrl-A`
  over windowed data is a predicate (`all` + `except`).
- **Mouse** — `install_mouse`: header-click sort cycle, click-to-cursor,
  Shift/Ctrl+click, press-drag rubber-band, and edge drag-autoscroll.
- **Sort** — `sort` / `toggle_sort` / `cycle_sort` with a numeric-aware stable
  default comparator and a `sort_by_with` hook; `data-sort` header contract + a
  configurable `▲`/`▼` glyph.
- **Column ops** — `move_column`, `set_column_hidden`, `set_column_width`
  (+ `Column::with_width`); cursor + sort indicator follow, cursor skips hidden
  columns. Opt-in column-actions column (`enable_column_actions`) with a
  native-checkbox column chooser.
- **Native scrollbar** — opt-in `enable_scrollbar`; proportional thumb via spacer
  rows, decoupled wheel/drag, cursor-follows-on-nav.
- **Windowed / live data source** — `set_total` + `apply(epoch, Delta)` push API
  (`Resync` / `Upsert` / `Remove`, epoch-guarded), `on_window_change` window
  requests with a prefetch margin, and `invalidate`. The cursor navigates the
  full total while only the visible window is materialized; not-yet-loaded slots
  render a `data-vt-loading` placeholder. Sync and backend-agnostic.
- **Persistable layout** — `TableState` / `ColumnState`, `table_state()`,
  `on_state_change`, and `restore_state`.

[Unreleased]: https://github.com/miskun/rdom-virtualtable/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/miskun/rdom-virtualtable/releases/tag/v0.1.0
