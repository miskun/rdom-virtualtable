//! Virtualized table built on native `<table>` elements.
//!
//! Unlike the chart components (which paint onto a `<canvas>`), the table
//! is a real DOM subtree: `<table>` → `<thead>`/`<tbody>` → `<tr>` →
//! `<th>`/`<td>`. rdom-tui's table builtin aligns columns across rows, so
//! this component only has to materialize the right rows.
//!
//! **Virtualization:** the data lives in the model; only a *window* of
//! rows is ever materialized into the `<tbody>`. A consumer with 100k
//! rows builds at most `count` `<tr>` nodes. Call
//! [`show_window`](VirtualTableView::show_window) with the slice to
//! display; recompute the slice from a scroll offset with
//! [`VirtualTable::window_for`].
//!
//! **Native scrollbar (opt-in):**
//! [`enable_scrollbar`](VirtualTableView::enable_scrollbar) makes the
//! `<tbody>` a vertical scroll container and brackets the window with
//! spacer rows so the scroll thumb reflects the *total* row count; a
//! `scroll` listener re-windows on wheel/drag. Without it, the consumer
//! drives [`show_window`](VirtualTableView::show_window) explicitly.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use rdom_tui::layout::{Border, BorderStyle, UserSelect};
use rdom_tui::runtime::builtins::table::size_columns;
use rdom_tui::{
    Color, Display, ListenerOptions, MouseButton, NodeId, Overflow, Padding, Size, Stylesheet,
    TuiAccessors, TuiAccessorsMut, TuiDom, TuiNodeExt, TuiNodeMutExt, TuiStyle, Value,
};

use crate::data::{Delta, Row, RowKey};
use crate::grid_cursor::{GridCursor, Nav, nav_for_key, reveal_scroll};
use crate::model::VirtualTable;
use crate::selection::{GridSelection, SelectionMode};
use crate::state::{ColumnState, TableState};
use crate::window::{SortSpec, WindowBuffer, WindowRequest};

/// Column operations (sort / reorder / hide / sort indicator) — `impl
/// VirtualTableView` blocks kept off this file.
mod columns;

/// The windowed-source callback (`on_window_change`): a boxed `FnMut` the table
/// calls with a fresh-epoch [`WindowRequest`] whenever the window must refill.
type WindowChangeCb = Box<dyn FnMut(WindowRequest)>;

/// The layout-persistence callback (`on_state_change`): a boxed `FnMut` the
/// table calls with the current [`TableState`] whenever the layout changes.
type StateChangeCb = Box<dyn FnMut(&TableState)>;

/// A shareable handle that owns a [`VirtualTable`] and materializes a
/// window of it as a `<table>` subtree in a `TuiDom`.
#[derive(Clone)]
pub struct VirtualTableView {
    inner: Rc<RefCell<VirtualTable>>,
    table: Rc<Cell<Option<NodeId>>>,
    tbody: Rc<Cell<Option<NodeId>>>,
    mounted_rows: Rc<RefCell<Vec<NodeId>>>,
    /// Cell (`<td>`) node ids per materialized row, in column order. Parallel
    /// to `mounted_rows`; lets highlight run without re-walking the tree.
    mounted_cells: Rc<RefCell<Vec<Vec<NodeId>>>>,
    /// Header (`<th>`) node ids in column order, recorded at `mount`.
    header_cells: Rc<RefCell<Vec<NodeId>>>,
    /// Logical keyboard cursor over the full dataset.
    cursor: Rc<Cell<GridCursor>>,
    /// Selection state (configurable mode; off by default).
    selection: Rc<RefCell<GridSelection>>,
    /// The window buffer the renderer reads. The in-memory model fills it from
    /// its resident slice on every `show_window`; a windowed source fills it via
    /// `apply(epoch, Delta)`. Rendering, key resolution, and total all read
    /// through here.
    buffer: Rc<RefCell<WindowBuffer>>,
    /// `true` once a windowed push source drives the buffer. In windowed
    /// mode the model is empty, so identity resolution reads the buffer (only
    /// loaded rows have a key); in the default in-memory mode it reads the model
    /// (all rows resident, so a key resolves at any index).
    windowed: Rc<Cell<bool>>,
    /// The windowed-source callback (`on_window_change`): the table calls it
    /// whenever the visible range, sort, or an `invalidate` changes what must be
    /// shown, handing a fresh-epoch [`WindowRequest`]. `None` until registered
    /// (in-memory mode never fires it).
    on_window_change: Rc<RefCell<Option<WindowChangeCb>>>,
    /// The prefetch range most recently requested via `on_window_change`. Guards
    /// against re-firing the identical request while its result is in flight
    /// (the table's request coalescing).
    last_request: Rc<RefCell<Option<Range<usize>>>>,
    /// The layout-persistence callback (`on_state_change`): fired whenever
    /// the column layout or sort changes, so the consumer can save UI state.
    on_state_change: Rc<RefCell<Option<StateChangeCb>>>,
    /// While set, `notify_state_change` is a no-op — used to silence the save
    /// callback during [`restore_state`](VirtualTableView::restore_state).
    suppress_state_change: Rc<Cell<bool>>,
    /// Visible data-row count — drives scroll-follow and the page step.
    viewport_rows: Rc<Cell<u16>>,
    /// Logical row that materialized pool row 0 currently represents.
    window_start: Rc<Cell<usize>>,
    /// Whether navigation/highlight is engaged. Pure-virtualization
    /// consumers (no cursor) leave this `false`, so `show_window` never
    /// writes `data-active-*` attributes behind their back.
    nav_active: Rc<Cell<bool>>,
    /// Sort-direction glyph suffixes `(ascending, descending)` appended to the
    /// sorted header text. Default `(" ▲", " ▼")`. Configurable because `▲`/`▼`
    /// are East-Asian *ambiguous-width*: a terminal set to render ambiguous
    /// glyphs double-width shifts later header columns by one — set narrow
    /// glyphs (`" ^"` / `" v"`, `" ↑"` / `" ↓"`) or `""` to avoid it.
    sort_glyphs: Rc<RefCell<(String, String)>>,
    /// Whether the native vertical scrollbar is engaged (`enable_scrollbar`).
    /// When set, `show_window` brackets the row window with spacer `<tr>`s so
    /// the `<tbody>`'s scroll extent reflects the *total* row count.
    scroll_mode: Rc<Cell<bool>>,
    /// The spacer `<tr>` ids (top + bottom) currently in `<tbody>`, dropped
    /// alongside the row window on the next `show_window`.
    spacers: Rc<RefCell<Vec<NodeId>>>,
    /// The header `<tr>` — the overflow chip is appended here when a column is
    /// hidden (the header persists across `show_window`, so the chip does too).
    header_tr: Rc<Cell<Option<NodeId>>>,
    /// The trailing `<th data-vt-overflow>` ("…"), present iff ≥1 column is
    /// hidden. Clicking it (or a consumer key via [`toggle_column_menu`]) opens
    /// the show/hide dropdown. **Not** a model column — invisible to
    /// `columns()`, sort, width sync, and the cursor.
    ///
    /// [`toggle_column_menu`]: VirtualTableView::toggle_column_menu
    overflow_chip: Rc<Cell<Option<NodeId>>>,
    /// The open show/hide dropdown overlay (a floating `<div data-vt-menu>`
    /// child of the chip), or `None` when closed.
    column_menu: Rc<Cell<Option<NodeId>>>,
    /// The welded "tab" box over the chip — a bordered box whose bottom edge
    /// coincides with the panel's top row, so `join_borders` welds chip-tab and
    /// panel into one tab-panel outline. Born with the menu on open and dropped
    /// with it on close (`None` while closed). A sibling of the panel, not inside
    /// it (it must anchor to the chip, above the panel), so it's dropped
    /// separately — safe since rdom-tui 0.3.10 (`CASCADE-FREED-ROOT-1`).
    menu_tab: Rc<Cell<Option<NodeId>>>,
    /// Index of the highlighted row in the open dropdown (into the current
    /// hidden-columns list). Meaningful only while the menu is open; reset to 0
    /// on open and clamped as the list shrinks.
    menu_cursor: Rc<Cell<usize>>,
    /// `true` between a left mousedown on a data cell and the matching mouseup —
    /// drives rubber-band range selection: `mousemove` while set extends the
    /// range to the hovered cell. Set by [`install_mouse`](Self::install_mouse).
    mouse_drag: Rc<Cell<bool>>,
}

impl VirtualTableView {
    pub fn new(table: VirtualTable) -> Self {
        Self {
            inner: Rc::new(RefCell::new(table)),
            table: Rc::new(Cell::new(None)),
            tbody: Rc::new(Cell::new(None)),
            mounted_rows: Rc::new(RefCell::new(Vec::new())),
            mounted_cells: Rc::new(RefCell::new(Vec::new())),
            header_cells: Rc::new(RefCell::new(Vec::new())),
            cursor: Rc::new(Cell::new(GridCursor::new())),
            selection: Rc::new(RefCell::new(GridSelection::new(SelectionMode::None))),
            buffer: Rc::new(RefCell::new(WindowBuffer::new())),
            windowed: Rc::new(Cell::new(false)),
            on_window_change: Rc::new(RefCell::new(None)),
            last_request: Rc::new(RefCell::new(None)),
            on_state_change: Rc::new(RefCell::new(None)),
            suppress_state_change: Rc::new(Cell::new(false)),
            viewport_rows: Rc::new(Cell::new(0)),
            window_start: Rc::new(Cell::new(0)),
            nav_active: Rc::new(Cell::new(false)),
            sort_glyphs: Rc::new(RefCell::new((" \u{25B2}".into(), " \u{25BC}".into()))),
            scroll_mode: Rc::new(Cell::new(false)),
            spacers: Rc::new(RefCell::new(Vec::new())),
            header_tr: Rc::new(Cell::new(None)),
            overflow_chip: Rc::new(Cell::new(None)),
            column_menu: Rc::new(Cell::new(None)),
            menu_tab: Rc::new(Cell::new(None)),
            menu_cursor: Rc::new(Cell::new(0)),
            mouse_drag: Rc::new(Cell::new(false)),
        }
    }

    /// Build `<table><thead>…</thead><tbody></tbody></table>`, remember
    /// the `<table>` and `<tbody>` ids, and return the `<table>` id. The
    /// `<tbody>` starts empty — call [`show_window`](Self::show_window).
    pub fn mount(&self, dom: &mut TuiDom) -> NodeId {
        let table = dom.create_element("table");
        let thead = dom.create_element("thead");
        let header_tr = dom.create_element("tr");

        let model = self.inner.borrow();
        let mut headers = Vec::with_capacity(model.columns().len());
        for col in model.columns() {
            let th = dom.create_element("th");
            let text = dom.create_text_node(&col.header);
            dom.append_child(th, text).unwrap();
            if let Some(w) = col.width {
                dom.node_mut(th).set_width(Size::Fixed(w));
            }
            dom.append_child(header_tr, th).unwrap();
            headers.push(th);
        }
        drop(model);
        *self.header_cells.borrow_mut() = headers;

        dom.append_child(thead, header_tr).unwrap();
        dom.append_child(table, thead).unwrap();

        let tbody = dom.create_element("tbody");
        dom.append_child(table, tbody).unwrap();

        self.table.set(Some(table));
        self.tbody.set(Some(tbody));
        self.header_tr.set(Some(header_tr));
        table
        // The column-actions chip + its click/change listeners are opt-in —
        // see `enable_column_actions`. A table that never calls it pays nothing.
    }

    /// Materialize rows `[start, start + count)` into the `<tbody>`,
    /// dropping any previously-materialized rows. No-op before `mount`.
    ///
    /// In the default in-memory mode this first fills the window buffer from
    /// the model's resident slice; in windowed mode the buffer is already
    /// filled by `apply`. Either way the rows render *from the buffer*, and a
    /// visible index the buffer has no row for paints a `data-vt-loading`
    /// placeholder rather than a blank or a stale row.
    pub fn show_window(&self, dom: &mut TuiDom, start: usize, count: usize) {
        let Some(tbody) = self.tbody.get() else {
            return;
        };
        // In-memory mode: copy the model's slice into the buffer + publish the
        // total. Windowed mode asks the source for any not-yet-loaded slice (a
        // no-op if the buffer already covers the window).
        if self.windowed.get() {
            self.request_window(start, count, false);
        } else {
            self.fill_buffer_from_model(start, count);
        }
        self.render_window(dom, tbody, start, count);

        if let Some(table) = self.table.get() {
            // `size_columns` (rdom-tui ≥ 0.3.5) stamps a column-width signature
            // on the `<table>` when the widths change, which dirties the table
            // subtree so the `<thead>` headers re-cascade with the new widths —
            // no consumer-side re-cascade hack needed (cf. `TABLE-COLSYNC-DIRTY-1`).
            size_columns(dom, table);
        }

        // Re-assert the cursor highlight onto the freshly-materialized window.
        // Gated so pure-virtualization consumers never get `data-active-*`.
        if self.nav_active.get() {
            self.apply_highlight(dom);
        }
    }

    /// In-memory filler: copy the model's `[start, end)` slice into the window
    /// buffer and publish the model's row count as the buffer total. The
    /// per-window clone is bounded by the viewport (~tens of rows), not the
    /// dataset.
    fn fill_buffer_from_model(&self, start: usize, count: usize) {
        let model = self.inner.borrow();
        let total = model.row_count();
        let end = (start + count).min(total);
        let rows: Vec<Row> = if start < end {
            model.rows()[start..end].to_vec()
        } else {
            Vec::new()
        };
        drop(model);
        let mut buf = self.buffer.borrow_mut();
        buf.set_total(total);
        buf.set_window(start, rows);
    }

    /// Render the visible window `[start, start + count)` from the buffer:
    /// drop the previous rows + spacers, materialize one `<tr>` per visible
    /// index (real cells when the buffer has the row, a `data-vt-loading`
    /// placeholder when it doesn't), and bracket the window with scroll spacers
    /// so the thumb reflects the buffer total.
    fn render_window(&self, dom: &mut TuiDom, tbody: NodeId, start: usize, count: usize) {
        // Drop the previous window's rows + spacers (frees the arena slots).
        for id in self.mounted_rows.borrow_mut().drain(..) {
            let _ = dom.drop_subtree(id);
        }
        for id in self.spacers.borrow_mut().drain(..) {
            let _ = dom.drop_subtree(id);
        }
        self.mounted_cells.borrow_mut().clear();

        let total = self.buffer.borrow().total();
        let end = (start + count).min(total);

        // Native-scrollbar mode: a top spacer of `start` rows makes the window
        // sit at the right scroll offset, and top + window + bottom = total, so
        // the `<tbody>` scroll extent reflects the whole dataset (proportional
        // thumb) while only the window is materialized.
        if self.scroll_mode.get() && start > 0 {
            let sp = self.make_spacer(dom, start);
            dom.append_child(tbody, sp).unwrap();
            self.spacers.borrow_mut().push(sp);
        }

        // Column config (count + hidden flags) is resident in both modes.
        let hidden: Vec<bool> = {
            let model = self.inner.borrow();
            (0..model.columns().len())
                .map(|c| model.is_column_hidden(c))
                .collect()
        };
        let ncols = hidden.len();
        let span = end.saturating_sub(start);
        let mut mounted = Vec::with_capacity(span);
        let mut mounted_cells = Vec::with_capacity(span);
        for vi in start..end {
            let tr = dom.create_element("tr");
            let mut row_cells = Vec::with_capacity(ncols);
            // Resolve the row's cells up front so the buffer borrow doesn't span
            // the DOM mutations below.
            let cells: Option<Vec<(String, Option<&'static str>)>> =
                self.buffer.borrow().row_at(vi).map(|row| {
                    (0..ncols)
                        .map(|c| {
                            let v = row.cell(c);
                            (v.display(), v.status().map(|l| l.as_attr()))
                        })
                        .collect()
                });
            match cells {
                Some(cells) => {
                    for (c, (text, status)) in cells.into_iter().enumerate() {
                        let td = dom.create_element("td");
                        let tn = dom.create_text_node(&text);
                        dom.append_child(td, tn).unwrap();
                        if let Some(level) = status {
                            let _ = dom.set_attribute(td, "data-vt-status", level);
                        }
                        if hidden[c] {
                            set_flag(dom, td, "data-vt-hidden", true);
                        }
                        dom.append_child(tr, td).unwrap();
                        row_cells.push(td);
                    }
                }
                None => {
                    // Placeholder row: a position in the visible range the buffer
                    // hasn't loaded yet. `data-vt-loading` lets consumer CSS paint
                    // a shimmer; the cell is empty, never stale.
                    for &is_hidden in &hidden {
                        let td = dom.create_element("td");
                        let _ = dom.set_attribute(td, "data-vt-loading", "");
                        if is_hidden {
                            set_flag(dom, td, "data-vt-hidden", true);
                        }
                        dom.append_child(tr, td).unwrap();
                        row_cells.push(td);
                    }
                }
            }
            dom.append_child(tbody, tr).unwrap();
            mounted.push(tr);
            mounted_cells.push(row_cells);
        }

        // Bottom spacer for the rows below the window (see top spacer above).
        if self.scroll_mode.get() && end < total {
            let sp = self.make_spacer(dom, total - end);
            dom.append_child(tbody, sp).unwrap();
            self.spacers.borrow_mut().push(sp);
        }

        *self.mounted_rows.borrow_mut() = mounted;
        *self.mounted_cells.borrow_mut() = mounted_cells;
        self.window_start.set(start);
    }

    /// Set the number of visible data rows. This drives scroll-follow (how
    /// far the cursor can travel before the window shifts) and the `PageUp`/
    /// `PageDown` step. Enabling a viewport also engages navigation, so
    /// subsequent [`show_window`](Self::show_window) calls reassert the
    /// cursor highlight.
    pub fn set_viewport_rows(&self, rows: u16) {
        self.viewport_rows.set(rows);
        self.nav_active.set(true);
    }

    // ── Windowed data source (push API) ──────────────────────────────

    /// The current window epoch. A windowed consumer echoes this back through
    /// [`apply`](Self::apply) so stale pushes (out-of-order async results, late
    /// deltas from a torn-down subscription) are dropped. Bumped whenever what
    /// must be shown changes.
    pub fn window_epoch(&self) -> u64 {
        self.buffer.borrow().epoch()
    }

    /// Set the total row count of the (filtered) result — drives the scrollbar
    /// extent. Puts the view in **windowed mode** (identity resolves from the
    /// buffer, not the model) and re-renders so the spacers reflect the new
    /// total. The consumer derives `total` from a count query/subscription.
    pub fn set_total(&self, dom: &mut TuiDom, total: usize) {
        self.windowed.set(true);
        self.buffer.borrow_mut().set_total(total);
        self.rerender_current(dom);
    }

    /// Apply a [`Delta`] for window `epoch`. **Pushes whose epoch ≠ the current
    /// window epoch are dropped silently** — this is what makes out-of-order
    /// async results and late deltas from a torn-down subscription safe. A
    /// `Resync` replaces the buffer for its range; `Upsert`/`Remove` patch by
    /// [`RowKey`](crate::RowKey) (an `Upsert` for a key not in the window is
    /// ignored, and any patch before the first `Resync` of an epoch is a no-op).
    /// Re-renders the current window (placeholders for not-yet-loaded slots) and
    /// reasserts the highlight. Puts the view in windowed mode.
    pub fn apply(&self, dom: &mut TuiDom, epoch: u64, delta: Delta) {
        {
            let mut buf = self.buffer.borrow_mut();
            if epoch != buf.epoch() {
                return; // stale push — drop
            }
            match delta {
                Delta::Resync { start, rows } => buf.set_window(start, rows),
                Delta::Upsert { rows } => {
                    for row in rows {
                        buf.upsert(row);
                    }
                }
                Delta::Remove { keys } => {
                    for key in &keys {
                        buf.remove(key);
                    }
                }
            }
        }
        self.windowed.set(true);
        self.rerender_current(dom);
    }

    /// Re-render the current visible window from the buffer (no re-fill) +
    /// reassert the highlight. The window stays `window_start ..
    /// window_start + viewport`; `viewport` falls back to the buffered span when
    /// no viewport is set. Routes through [`show_window`](Self::show_window),
    /// which skips the in-memory fill in windowed mode.
    fn rerender_current(&self, dom: &mut TuiDom) {
        if self.tbody.get().is_none() {
            return;
        }
        self.show_window(dom, self.window_start.get(), self.current_count());
    }

    /// The current visible row count: the viewport if set, else the buffered
    /// span (a consumer driving `show_window` directly without a viewport).
    fn current_count(&self) -> usize {
        let vp = self.viewport_rows.get() as usize;
        if vp == 0 {
            self.buffer.borrow().len()
        } else {
            vp
        }
    }

    /// Register the windowed-source callback. The table calls it whenever the
    /// visible range, sort, or an [`invalidate`](Self::invalidate) changes what
    /// must be shown, handing a fresh-epoch [`WindowRequest`]; the consumer
    /// re-queries its source and pushes [`apply`](Self::apply) back, echoing the
    /// epoch. Registering it puts the view in **windowed mode**. The callback
    /// must not synchronously re-enter the view's windowing methods (it should
    /// enqueue an async fetch and return).
    pub fn on_window_change(&self, cb: impl FnMut(WindowRequest) + 'static) {
        self.windowed.set(true);
        *self.on_window_change.borrow_mut() = Some(Box::new(cb));
    }

    /// Force a re-fetch of the current window: drop the buffered rows (so stale
    /// data doesn't paint while the refresh is in flight — placeholders show
    /// instead) and re-fire `on_window_change` with a fresh epoch. The consumer
    /// calls this when *its* filter changes (the table has no filter UI) or as a
    /// "refresh now" hook.
    pub fn invalidate(&self, dom: &mut TuiDom) {
        *self.last_request.borrow_mut() = None;
        self.buffer.borrow_mut().clear_rows();
        self.request_window(self.window_start.get(), self.current_count(), true);
        self.rerender_current(dom);
    }

    /// Windowed-mode reset before a structural re-fetch (a sort change): drop
    /// the stale rows + the in-flight request marker so the following
    /// `show_window` issues a fresh request with the new parameters. No-op in
    /// in-memory mode (the model re-fills the buffer directly).
    fn reset_window_for_refetch(&self) {
        if self.windowed.get() {
            *self.last_request.borrow_mut() = None;
            self.buffer.borrow_mut().clear_rows();
        }
    }

    /// The table's current sort as [`SortSpec`]s (keyed by column header, stable
    /// across reorder). Empty when unsorted.
    fn current_sort_specs(&self) -> Vec<SortSpec> {
        let model = self.inner.borrow();
        match model.sort_state() {
            Some((col, dir)) => model
                .columns()
                .get(col)
                .map(|c| {
                    vec![SortSpec {
                        column: c.header.clone(),
                        dir,
                    }]
                })
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// Fire `on_window_change` for the window `[start, start + count)` if needed.
    /// Skips when the buffer already covers the visible range (`force` overrides,
    /// e.g. `invalidate`) or when the same prefetch range is already in flight.
    /// Expands the visible range by a ±50% prefetch margin so adjacent scroll is
    /// shimmer-free, bumps the epoch, and stamps `last_request`. No-op when no
    /// callback is registered (in-memory mode).
    fn request_window(&self, start: usize, count: usize, force: bool) {
        if self.on_window_change.borrow().is_none() {
            return;
        }
        let total = self.buffer.borrow().total();
        let end = (start + count).min(total);
        if !force {
            let covered = {
                let b = self.buffer.borrow();
                (start..end).all(|i| b.is_loaded(i))
            };
            if covered {
                return;
            }
        }
        let margin = count / 2; // ±50% prefetch margin
        let range = start.saturating_sub(margin)..(end + margin).min(total);
        if !force && self.last_request.borrow().as_ref() == Some(&range) {
            return;
        }
        let epoch = self.buffer.borrow_mut().bump_epoch();
        *self.last_request.borrow_mut() = Some(range.clone());
        let req = WindowRequest {
            epoch,
            range,
            sort: self.current_sort_specs(),
        };
        // Take the callback out across the call so a re-entrant render
        // (callback → apply → rerender → show_window → request_window) sees no
        // callback: it neither re-fires nor double-borrows the RefCell.
        let taken = self.on_window_change.borrow_mut().take();
        if let Some(mut cb) = taken {
            cb(req);
            *self.on_window_change.borrow_mut() = Some(cb);
        }
    }

    // ── Persistable UI state ─────────────────────────────────────────

    /// Snapshot the column layout (order, widths, hidden) + the active sort as a
    /// [`TableState`] the consumer can persist. Columns are in display order;
    /// everything is keyed by column header (stable across reorders).
    pub fn table_state(&self) -> TableState {
        let model = self.inner.borrow();
        let columns = model
            .columns()
            .iter()
            .enumerate()
            .map(|(i, c)| ColumnState {
                header: c.header.clone(),
                width: c.width,
                hidden: model.is_column_hidden(i),
            })
            .collect();
        let sort = model.sort_state().and_then(|(col, dir)| {
            model.columns().get(col).map(|c| SortSpec {
                column: c.header.clone(),
                dir,
            })
        });
        TableState { columns, sort }
    }

    /// Register a callback fired whenever the layout changes — a sort, a column
    /// reorder, a width change, or a hide/show. Hand the consumer the current
    /// [`TableState`] so it can persist UI state. Not fired during
    /// [`restore_state`](Self::restore_state) (a restore isn't a user edit).
    pub fn on_state_change(&self, cb: impl FnMut(&TableState) + 'static) {
        *self.on_state_change.borrow_mut() = Some(Box::new(cb));
    }

    /// Re-apply a previously-saved [`TableState`]: reorder columns to match, set
    /// each column's width + hidden flag, and apply the sort — all by header, so
    /// columns present in one but not the other are matched where possible and
    /// skipped otherwise. The `on_state_change` callback is suppressed for the
    /// duration.
    pub fn restore_state(&self, dom: &mut TuiDom, state: &TableState) {
        self.suppress_state_change.set(true);
        // 1. Reorder to the saved order (selection-sort by header).
        for (target, col) in state.columns.iter().enumerate() {
            let cur = self.with(|t| t.columns().iter().position(|c| c.header == col.header));
            if let Some(cur) = cur {
                if cur != target {
                    self.move_column(dom, cur, target);
                }
            }
        }
        // 2. Widths + hidden, by header (order-independent).
        for col in &state.columns {
            if let Some(idx) =
                self.with(|t| t.columns().iter().position(|c| c.header == col.header))
            {
                self.set_column_width(dom, idx, col.width);
                self.set_column_hidden(dom, idx, col.hidden);
            }
        }
        // 3. Sort.
        match &state.sort {
            Some(spec) => {
                if let Some(idx) =
                    self.with(|t| t.columns().iter().position(|c| c.header == spec.column))
                {
                    self.sort(dom, idx, spec.dir);
                }
            }
            None => self.clear_sort(dom),
        }
        self.suppress_state_change.set(false);
    }

    /// Fire `on_state_change` with the current snapshot — unless suppressed (a
    /// restore in progress) or no callback is registered. Called at the end of
    /// every layout mutation.
    pub(crate) fn notify_state_change(&self) {
        if self.suppress_state_change.get() || self.on_state_change.borrow().is_none() {
            return;
        }
        let state = self.table_state();
        // Take across the call so a re-entrant mutation from the callback
        // doesn't double-borrow the RefCell.
        let taken = self.on_state_change.borrow_mut().take();
        if let Some(mut cb) = taken {
            cb(&state);
            *self.on_state_change.borrow_mut() = Some(cb);
        }
    }

    /// Build a spacer `<tr>` of `rows` cells tall, marked so consumer CSS and
    /// the highlight pass skip it. Height is `u16`-bounded (~65k rows) — see
    /// [`enable_scrollbar`](Self::enable_scrollbar) for the implication.
    fn make_spacer(&self, dom: &mut TuiDom, rows: usize) -> NodeId {
        let h = rows.min(u16::MAX as usize) as u16;
        let tr = dom.create_element("tr");
        dom.node_mut(tr)
            .set_inline_style(TuiStyle::new().height(Size::Fixed(h)));
        let _ = dom.set_attribute(tr, "data-rdom-spacer", "");
        tr
    }

    /// Engage the **native vertical scrollbar**: the `<tbody>` becomes a
    /// `overflow-y: auto` scroll container `viewport_rows` tall, the window is
    /// bracketed with spacer `<tr>`s so the scroll thumb reflects the *total*
    /// row count, and a `scroll` listener re-windows as the user wheels / drags
    /// (decoupled — scrolling moves the viewport, not the cursor; the cursor
    /// scrolls back into view only when [`navigate`](Self::navigate)d). Call
    /// after [`set_viewport_rows`](Self::set_viewport_rows) /
    /// [`install_nav`](Self::install_nav) and after the first
    /// [`show_window`](Self::show_window). The `<thead>` stays put (it's outside
    /// the scroll container — no sticky needed) so header and body columns stay
    /// aligned.
    ///
    /// **Assumes uniform single-cell rows** (the spacer/offset math is in row
    /// units = cells); wrapped or multi-line cells break the scroll mapping.
    /// The spacer height is `u16`-bounded, so the draggable thumb spans the
    /// first ~65k rows; beyond that, keyboard navigation (unbounded) still
    /// reaches every row.
    pub fn enable_scrollbar(&self, dom: &mut TuiDom) {
        let Some(tbody) = self.tbody.get() else {
            return;
        };
        let vp = self.viewport_rows.get();
        dom.node_mut(tbody).set_inline_style(
            TuiStyle::new()
                .overflow_y(Overflow::Auto)
                .height(Size::Fixed(vp)),
        );
        self.scroll_mode.set(true);

        // Decoupled scroll: on wheel/drag, re-window to the new offset; the
        // cursor is left untouched. The listener NEVER writes scroll_top, so
        // there's no re-entrancy with the cursor path (which DOES write it).
        let view = self.clone();
        dom.add_event_listener(tbody, "scroll", ListenerOptions::default(), move |ctx| {
            let top = ctx.dom.node(tbody).scroll_top().unwrap_or(0).max(0) as usize;
            if top != view.window_start.get() {
                view.show_window(ctx.dom, top, view.viewport_rows.get() as usize);
                ctx.request_redraw();
            }
        })
        .expect("tbody accepts a scroll listener");

        // Re-materialize the current window so the spacers appear.
        self.show_window(dom, self.window_start.get(), vp as usize);
    }

    /// The current logical cursor (active `(row, col)` + scroll offset).
    /// Useful for acting on the focused row (e.g. an `Enter` handler).
    pub fn cursor(&self) -> GridCursor {
        self.cursor.get()
    }

    /// The logical index of the first row in the currently materialized window.
    /// In scroll-mode this tracks the `<tbody>`'s `scroll_top`.
    pub fn window_start(&self) -> usize {
        self.window_start.get()
    }

    /// Move the cursor per `nav`, clamped + scrolled into view, and return
    /// `(before, after)`. Caller updates the selection, then calls
    /// [`refresh_after_cursor`](Self::refresh_after_cursor). Returns `None`
    /// for an empty grid.
    fn move_cursor(&self, nav: Nav) -> Option<(GridCursor, GridCursor)> {
        let cols = self.with(|t| t.columns().len());
        let rows = self.total_rows();
        if rows == 0 || cols == 0 {
            return None;
        }
        let viewport = self.viewport_rows.get() as usize;
        let before = self.cursor.get();
        let mut after = before.navigate(nav, rows, cols, viewport.max(1));
        // Skip hidden columns on a horizontal move: keep scanning in the move
        // direction for a visible column; if none exists that way, stay put.
        if after.col() != before.col() && self.inner.borrow().is_column_hidden(after.col()) {
            let dir: isize = if after.col() > before.col() { 1 } else { -1 };
            let model = self.inner.borrow();
            let mut c = after.col() as isize;
            while c >= 0 && (c as usize) < cols && model.is_column_hidden(c as usize) {
                c += dir;
            }
            drop(model);
            after = if c >= 0 && (c as usize) < cols {
                after.at(after.row(), c as usize, rows, cols)
            } else {
                before
            };
        }
        self.cursor.set(after);
        Some((before, after))
    }

    /// Scroll the cursor into view if its move pushed it off-window, else just
    /// re-apply the highlight.
    ///
    /// **Single scroll authority** (`SCROLL-SINGLE-OWNER-1`): scroll-into-view
    /// is computed from the *current* window position via `reveal_scroll` —
    /// the `<tbody>`'s `scroll_top` in native-scrollbar mode, `window_start`
    /// in pure-windowed mode — never from a copy held on the cursor. So:
    /// - a wheel / drag scroll the cursor didn't drive is honored (the next
    ///   keyboard move reveals the cursor relative to where the view actually
    ///   sits, no snap-back to a stale offset); and
    /// - an autoscroll drag needs **no special-casing** — the drag head stays
    ///   within the materialized window, so `reveal_scroll` returns the current
    ///   offset and never writes, leaving the substrate's autoscroll `scroll_top`
    ///   untouched (this replaces the old `!mouse_drag` guard).
    fn refresh_after_cursor(&self, dom: &mut TuiDom, after: GridCursor) {
        let viewport = self.viewport_rows.get() as usize;
        let rows = self.total_rows();

        if self.scroll_mode.get() {
            // Truth = the <tbody>'s scroll_top. Reveal the cursor relative to it
            // and write once; the `scroll` listener re-windows + re-highlights.
            let Some(tbody) = self.tbody.get() else {
                self.apply_highlight(dom);
                return;
            };
            let cur_top = dom.node(tbody).scroll_top().unwrap_or(0).max(0) as usize;
            let want = reveal_scroll(after.row(), cur_top, viewport, rows);
            if want != cur_top {
                let _ = dom.node_mut(tbody).set_scroll_top(want as i32);
            } else {
                // Already in view — the listener won't fire, so re-highlight here.
                self.apply_highlight(dom);
            }
            return;
        }

        // Pure-windowed mode: truth = `window_start`; the same reveal, applied
        // by re-materializing the window at the new offset.
        let cur_top = self.window_start.get();
        let want = reveal_scroll(after.row(), cur_top, viewport, rows);
        if viewport > 0 && want != cur_top {
            let (start, count) = VirtualTable::window_for(viewport as u16, want, rows);
            self.show_window(dom, start, count); // re-applies highlight + selection
        } else {
            self.apply_highlight(dom);
        }
    }

    /// Apply a navigation move to the cursor, scroll to keep it visible,
    /// re-materialize the window if it shifted, and update the highlight
    /// attributes. A plain move collapses the *transient* selections (an
    /// in-progress range and a `Ctrl-A` select-all) while keeping the
    /// `Space`-toggled set — see [`GridSelection::collapse_transient`]. Returns
    /// `true` if the cursor actually moved.
    pub fn navigate(&self, dom: &mut TuiDom, nav: Nav) -> bool {
        self.nav_active.set(true);
        let Some((before, after)) = self.move_cursor(nav) else {
            return false;
        };
        self.selection.borrow_mut().collapse_transient();
        self.refresh_after_cursor(dom, after);
        after != before
    }

    // ── Selection (configurable; off by default) ─────────────────────

    /// Set the selection mode. `SelectionMode::None` (default) disables
    /// selection entirely; `Cell` selects rectangular cell ranges; `Row`
    /// selects whole rows. Changing the mode clears any active selection and
    /// engages the highlight contract.
    pub fn set_selection_mode(&self, mode: SelectionMode) {
        self.selection.borrow_mut().set_mode(mode);
        if mode != SelectionMode::None {
            self.nav_active.set(true);
        }
    }

    /// The current selection mode.
    pub fn selection_mode(&self) -> SelectionMode {
        self.selection.borrow().mode()
    }

    /// A snapshot of the current selection — query its mode / activity /
    /// predicate ([`is_active`](GridSelection::is_active),
    /// [`is_all`](GridSelection::is_all), [`explicit`](GridSelection::explicit),
    /// [`except`](GridSelection::except)). To ask whether a *positional* cell is
    /// selected, use [`is_cell_selected`](Self::is_cell_selected) — it resolves
    /// the row's identity from the model, which the bare snapshot can't.
    pub fn selection(&self) -> GridSelection {
        self.selection.borrow().clone()
    }

    /// Resolve a view index to its row identity. In-memory mode reads the model
    /// (all rows resident → a key resolves at any index); windowed mode reads
    /// the buffer (only loaded rows have a key — an index off the loaded window
    /// is `None`). The single seam every identity lookup (selection, highlight)
    /// goes through, so the two modes differ in exactly one place.
    fn key_at(&self, index: usize) -> Option<RowKey> {
        if self.windowed.get() {
            self.buffer.borrow().key_at(index).cloned()
        } else {
            self.inner.borrow().rows().get(index).map(|r| r.key.clone())
        }
    }

    /// The dataset's row count for cursor / nav / scroll math. The sibling of
    /// [`key_at`](Self::key_at): in windowed mode the model is empty, so the
    /// total lives in the buffer (the full filtered result, e.g. 100k); in
    /// in-memory mode it's the resident row count. Every place that bounds the
    /// cursor or computes a scroll offset must read this, never `row_count()`
    /// directly — otherwise windowed nav pins the cursor at row 0.
    fn total_rows(&self) -> usize {
        if self.windowed.get() {
            self.buffer.borrow().total()
        } else {
            self.inner.borrow().row_count()
        }
    }

    /// The stable identity ([`RowKey`](crate::RowKey)) of the row at view index
    /// `row`, or `None` if that row isn't currently loaded (windowed mode, past
    /// the buffered window). Lets a consumer act on "the cursor's row" — e.g.
    /// push a live [`Delta::Upsert`](crate::Delta) for it.
    pub fn row_key_at(&self, row: usize) -> Option<RowKey> {
        self.key_at(row)
    }

    /// Is the cell at view index `(row, col)` selected? Resolves the row's
    /// [`RowKey`](crate::RowKey) via the mode-aware key seam so the answer
    /// follows identity across re-sorts / live updates. A row index past the
    /// loaded data is never selected (no identity to match).
    pub fn is_cell_selected(&self, row: usize, col: usize) -> bool {
        self.key_at(row)
            .is_some_and(|k| self.selection.borrow().is_selected(row, col, &k))
    }

    /// Extend the selection by moving the cursor (Shift+arrow): the range
    /// anchors at the pre-move cursor and its head follows. No-op when the
    /// mode is `None`.
    pub fn extend_selection(&self, dom: &mut TuiDom, nav: Nav) -> bool {
        self.nav_active.set(true);
        let Some((before, after)) = self.move_cursor(nav) else {
            return false;
        };
        self.selection
            .borrow_mut()
            .extend((before.row(), before.col()), (after.row(), after.col()));
        self.refresh_after_cursor(dom, after);
        after != before
    }

    /// Toggle the selection at the cursor (`Space`). If a `Shift`-range is
    /// live, the **whole rectangle** is committed into the sticky set (and the
    /// range collapses) — so Shift-select + `Space` builds a persistent range,
    /// repeatable for multiple ranges. With no live range it toggles just the
    /// cursor cell (or row, in `Row` mode).
    pub fn toggle_selection(&self, dom: &mut TuiDom) {
        let c = self.cursor.get();
        {
            let mut sel = self.selection.borrow_mut();
            // `key_at` resolves each index per the active mode (model or buffer);
            // it borrows a *different* RefCell than `sel`, so no borrow conflict.
            if !sel.toggle_range(|r| self.key_at(r)) {
                if let Some(key) = self.key_at(c.row()) {
                    sel.toggle(key, c.col());
                }
            }
        }
        self.apply_highlight(dom);
    }

    /// Select the whole grid (`Ctrl-A`).
    pub fn select_all(&self, dom: &mut TuiDom) {
        self.selection.borrow_mut().select_all();
        self.apply_highlight(dom);
    }

    /// Clear the selection (`Esc`).
    pub fn clear_selection(&self, dom: &mut TuiDom) {
        self.selection.borrow_mut().clear();
        self.apply_highlight(dom);
    }

    // ── Mouse-driven cursor + selection (logical coordinates) ────────

    /// Move the cursor to logical `(row, col)`, scroll it into view, and
    /// collapse the transient selection — the plain click-to-select gesture
    /// (the mouse analog of an arrow-key move). Returns `true` if the cursor
    /// moved. No-op for an empty grid.
    pub fn set_cursor_at(&self, dom: &mut TuiDom, row: usize, col: usize) -> bool {
        let cols = self.with(|t| t.columns().len());
        let rows = self.total_rows();
        if rows == 0 || cols == 0 {
            return false;
        }
        self.nav_active.set(true);
        let before = self.cursor.get();
        let after = before.at(row.min(rows - 1), col.min(cols - 1), rows, cols);
        self.cursor.set(after);
        self.selection.borrow_mut().collapse_transient();
        self.refresh_after_cursor(dom, after);
        after != before
    }

    /// Extend the selection to logical `(row, col)` — Shift+click and drag.
    /// Anchors at the pre-extend cursor if no range is active, head at
    /// `(row, col)`; moves the cursor to the head. When the selection mode is
    /// `None` this is just a cursor move (matching the keyboard, where Shift
    /// without a mode does nothing special).
    pub fn extend_selection_to(&self, dom: &mut TuiDom, row: usize, col: usize) -> bool {
        if self.selection_mode() == SelectionMode::None {
            return self.set_cursor_at(dom, row, col);
        }
        let cols = self.with(|t| t.columns().len());
        let rows = self.total_rows();
        if rows == 0 || cols == 0 {
            return false;
        }
        self.nav_active.set(true);
        let before = self.cursor.get();
        let after = before.at(row.min(rows - 1), col.min(cols - 1), rows, cols);
        self.cursor.set(after);
        self.selection
            .borrow_mut()
            .extend((before.row(), before.col()), (after.row(), after.col()));
        self.refresh_after_cursor(dom, after);
        true
    }

    /// Toggle logical `(row, col)` in the selection and move the cursor there —
    /// Ctrl/⌘+click (the mouse analog of `Space`). When the mode is `None` this
    /// is just a cursor move.
    pub fn toggle_at(&self, dom: &mut TuiDom, row: usize, col: usize) -> bool {
        if self.selection_mode() == SelectionMode::None {
            return self.set_cursor_at(dom, row, col);
        }
        let cols = self.with(|t| t.columns().len());
        let rows = self.total_rows();
        if rows == 0 || cols == 0 {
            return false;
        }
        self.nav_active.set(true);
        let after = self
            .cursor
            .get()
            .at(row.min(rows - 1), col.min(cols - 1), rows, cols);
        self.cursor.set(after);
        if let Some(key) = self.key_at(row) {
            self.selection.borrow_mut().toggle(key, col);
        }
        self.refresh_after_cursor(dom, after);
        true
    }

    /// Logical `(row, col)` of the data cell (`<td>`) containing `target`, or
    /// `None` if `target` isn't inside a materialized body cell. Maps the cell's
    /// window row back to a logical row via `window_start`.
    fn data_cell_of(&self, dom: &TuiDom, target: NodeId) -> Option<(usize, usize)> {
        let td = dom.node(target).closest("td")?.id();
        let cells = self.mounted_cells.borrow();
        for (r, row) in cells.iter().enumerate() {
            if let Some(c) = row.iter().position(|&id| id == td) {
                return Some((self.window_start.get() + r, c));
            }
        }
        None
    }

    /// Column index of the header cell (`<th>`) containing `target`, or `None`
    /// (the trailing overflow chip is not a model column, so it returns `None`).
    fn header_col_of(&self, dom: &TuiDom, target: NodeId) -> Option<usize> {
        let th = dom.node(target).closest("th")?.id();
        self.header_cells.borrow().iter().position(|&id| id == th)
    }

    /// Map screen coords to a logical `(row, col)`, **clamped into the
    /// materialized window** — the drag-extend path. Under autoscroll the
    /// pointer is beyond the viewport edge (no cell under it), so this resolves
    /// the cell by coordinate against the mounted cell rects instead of the
    /// event target. The resolved row's window index is translated to a logical
    /// row via `window_start` (which the autoscroll's `scroll` event updated).
    fn drag_cell_at(&self, dom: &TuiDom, cx: i32, cy: i32) -> Option<(usize, usize)> {
        let cells = self.mounted_cells.borrow();
        let n = cells.len();
        if n == 0 {
            return None;
        }
        let row_rect = |r: usize| {
            cells
                .get(r)
                .and_then(|row| row.first())
                .and_then(|&id| dom.node(id).layout_rect())
        };
        // Clamp cy into the mounted rows' vertical band, find the row it lands in.
        let top = row_rect(0)?.y;
        let last = row_rect(n - 1)?;
        let cyc = cy.clamp(top, (last.y + last.height as i32 - 1).max(top));
        let mut row = n - 1;
        for r in 0..n {
            if let Some(rr) = row_rect(r)
                && cyc < rr.y + rr.height as i32
            {
                row = r;
                break;
            }
        }
        // Column: the last visible cell in that row whose left edge is <= cx
        // (breaking at the containing one); cx clamped to the first visible cell.
        let row_cells = &cells[row];
        let first_left = row_cells.iter().find_map(|&id| {
            dom.node(id)
                .layout_rect()
                .filter(|r| r.width > 0)
                .map(|r| r.x)
        })?;
        let cxc = cx.max(first_left);
        let mut col = 0usize;
        for (c, &id) in row_cells.iter().enumerate() {
            let Some(r) = dom.node(id).layout_rect() else {
                continue;
            };
            if r.width == 0 {
                continue; // hidden column
            }
            if cxc >= r.x {
                col = c;
            }
            if cxc < r.x + r.width as i32 {
                break;
            }
        }
        Some((self.window_start.get() + row, col))
    }

    /// Re-materialize the currently-shown window (same start + count) — call
    /// after mutating the model via [`with`](Self::with) so the DOM reflects
    /// the change. No-op before [`mount`](Self::mount).
    pub fn refresh(&self, dom: &mut TuiDom) {
        if self.tbody.get().is_none() {
            return;
        }
        let start = self.window_start.get();
        let count = self.mounted_rows.borrow().len();
        self.show_window(dom, start, count);
    }

    /// Attach a `keydown` listener to `table` that drives [`navigate`](Self::navigate) with
    /// the built-in [`nav_for_key`] keymap (arrows, `hjkl`, `g`/`G`,
    /// `Home`/`End`, `PageUp`/`PageDown`). Handled keys are consumed
    /// (`prevent_default`) and trigger a redraw. `viewport_rows` is the
    /// number of visible data rows.
    ///
    /// The `table` must be focusable (e.g. `tabindex="0"`) and focused for
    /// the keys to arrive. Pair with [`highlight_stylesheet`] (or your own
    /// `data-active-*` rules) to see the cursor.
    pub fn install_nav(&self, dom: &mut TuiDom, table: NodeId, viewport_rows: u16) {
        self.set_viewport_rows(viewport_rows);
        let view = self.clone();
        dom.add_event_listener(table, "keydown", ListenerOptions::default(), move |ctx| {
            let Some(kbd) = ctx.event.detail.as_keyboard() else {
                return;
            };
            let key = kbd.key.as_str();
            let shift = kbd.modifiers.shift;
            let ctrl = kbd.modifiers.ctrl || kbd.modifiers.meta;

            // While the show/hide dropdown is open it OWNS the keyboard (modal):
            // Up/Down move the menu highlight, Enter/Space activate the
            // highlighted row (un-hide that column), Esc closes. Table cursor /
            // selection navigation is frozen — we always return here so arrows
            // never leak through to the cells behind the open menu.
            if view.is_column_menu_open() {
                match key {
                    "Escape" => view.close_column_menu(ctx.dom),
                    "ArrowDown" | "j" => view.menu_highlight_move(ctx.dom, 1),
                    "ArrowUp" | "k" => view.menu_highlight_move(ctx.dom, -1),
                    "Enter" | " " => view.menu_activate(ctx.dom),
                    _ => return, // unrelated key: ignore, but don't move the table
                }
                ctx.event.prevent_default();
                ctx.request_redraw();
                return;
            }

            let mut handled = true;

            // Selection keys (only when a selection mode is engaged): Ctrl-A
            // select-all, Esc clear, Space toggle the cursor cell/row,
            // Shift+nav extend the range.
            if view.selection_mode() != SelectionMode::None {
                if ctrl && key == "a" {
                    view.select_all(ctx.dom);
                } else if key == "Escape" {
                    view.clear_selection(ctx.dom);
                } else if key == " " {
                    view.toggle_selection(ctx.dom);
                } else if shift {
                    match nav_for_key(key, false) {
                        Some(nav) => {
                            view.extend_selection(ctx.dom, nav);
                        }
                        None => handled = false,
                    }
                } else if let Some(nav) = nav_for_key(key, false) {
                    view.navigate(ctx.dom, nav);
                } else {
                    handled = false;
                }
            } else if let Some(nav) = nav_for_key(key, shift) {
                view.navigate(ctx.dom, nav);
            } else {
                handled = false;
            }

            if handled {
                ctx.event.prevent_default();
                ctx.request_redraw();
            }
        })
        .expect("table node accepts a keydown listener");
        // Reflect the initial cursor onto whatever window is already shown.
        self.apply_highlight(dom);
    }

    /// Wire mouse interaction (root-delegated, so it survives window
    /// re-materialization). Idempotent concerns aside, call once after
    /// [`mount`](Self::mount):
    ///
    /// - **Header click** cycles that column's sort: asc → desc → off (off
    ///   restores the as-inserted order). See [`cycle_sort`](Self::cycle_sort).
    /// - **Click a body cell** moves the cursor to it (collapsing any range).
    /// - **Shift+click** extends the selection range to the clicked cell.
    /// - **Ctrl/⌘+click** toggles the clicked cell/row in the selection.
    /// - **Press + drag** rubber-bands a range from the press cell to the cell
    ///   under the cursor. When the body is scrollable
    ///   ([`enable_scrollbar`](Self::enable_scrollbar)), dragging past the top
    ///   or bottom edge **autoscrolls** the window in and keeps the rectangle
    ///   growing to rows that weren't materialized when the drag began — the
    ///   substrate drives this via pointer capture + its drag-autoscroll tick
    ///   (rdom-tui ≥ 0.3.11; see `DRAG-AUTOSCROLL`). The press captures the
    ///   pointer on the `<table>` and arms autoscroll; `mouseup` releases it.
    ///
    /// The selection gestures (drag / Shift / Ctrl) need a selection mode
    /// ([`set_selection_mode`](Self::set_selection_mode)); plain clicks and sort
    /// work regardless. Pair with [`install_nav`](Self::install_nav) for the
    /// keyboard. The table cells should set `user-select: none` (the
    /// [`highlight_stylesheet`] does) so a drag rubber-bands cells instead of
    /// starting a text selection.
    pub fn install_mouse(&self, dom: &mut TuiDom) {
        let root = dom.root();

        // mousedown on a body cell: start the interaction (+ a potential drag).
        let view = self.clone();
        dom.add_event_listener(root, "mousedown", ListenerOptions::default(), move |ctx| {
            if view.is_column_menu_open() {
                return;
            }
            let Some(target) = ctx.event.target else {
                return;
            };
            let Some(m) = ctx.event.detail.as_mouse() else {
                return;
            };
            if m.button != MouseButton::Left {
                return;
            }
            let Some((row, col)) = view.data_cell_of(ctx.dom, target) else {
                return;
            };
            if m.modifiers.ctrl || m.modifiers.meta {
                view.toggle_at(ctx.dom, row, col);
            } else if m.modifiers.shift {
                view.extend_selection_to(ctx.dom, row, col);
            } else {
                view.set_cursor_at(ctx.dom, row, col);
                view.mouse_drag.set(true);
                // Capture the pointer so the drag keeps coming when the cursor
                // leaves the table, and opt into edge autoscroll so dragging
                // past the viewport scrolls + keeps selecting (DRAG-AUTOSCROLL).
                // `prevent_default` below claims the drag from text selection.
                if let Some(table) = view.table.get() {
                    let _ = ctx.dom.set_pointer_capture(table);
                    ctx.dom.set_drag_autoscroll(true);
                }
            }
            ctx.event.prevent_default(); // suppress the default text-selection drag
            ctx.request_redraw();
        })
        .expect("root accepts a mousedown listener");

        // mousemove while dragging: extend the range to the hovered cell.
        let view = self.clone();
        dom.add_event_listener(root, "mousemove", ListenerOptions::default(), move |ctx| {
            if !view.mouse_drag.get() {
                return;
            }
            let Some(m) = ctx.event.detail.as_mouse() else {
                return;
            };
            // Left button released out of band → end the drag defensively.
            if m.buttons & 0b0001 == 0 {
                view.mouse_drag.set(false);
                return;
            }
            // Resolve the target cell: the node under the pointer if it's a
            // body cell, else by COORDS clamped into the window. The coords path
            // is what handles a captured drag (target is the table, not a cell)
            // and autoscroll (the synthetic move is beyond the edge — nothing
            // under it).
            let cell = ctx
                .event
                .target
                .and_then(|t| view.data_cell_of(ctx.dom, t))
                .or_else(|| view.drag_cell_at(ctx.dom, m.client_x, m.client_y));
            if let Some((row, col)) = cell
                && view.extend_selection_to(ctx.dom, row, col)
            {
                ctx.request_redraw();
            }
        })
        .expect("root accepts a mousemove listener");

        // mouseup anywhere: end the drag (the range stays as committed).
        let view = self.clone();
        dom.add_event_listener(root, "mouseup", ListenerOptions::default(), move |_ctx| {
            view.mouse_drag.set(false);
        })
        .expect("root accepts a mouseup listener");

        // click on a header: cycle the sort. Body-cell clicks are handled on
        // mousedown; the overflow chip / chooser are handled by
        // `install_menu_clicks`. Skip while the chooser owns the pointer.
        let view = self.clone();
        dom.add_event_listener(root, "click", ListenerOptions::default(), move |ctx| {
            if view.is_column_menu_open() {
                return;
            }
            let Some(target) = ctx.event.target else {
                return;
            };
            if let Some(col) = view.header_col_of(ctx.dom, target) {
                view.cycle_sort(ctx.dom, col);
                ctx.request_redraw();
            }
        })
        .expect("root accepts a click listener");
    }

    /// Write the cursor (`data-active-row` / `-col` / `-cell`) and selection
    /// (`data-selected`) presence attributes onto the materialized window +
    /// header to match the current cursor + selection. Clears them everywhere
    /// they no longer apply, so a single pass both sets and unsets.
    fn apply_highlight(&self, dom: &mut TuiDom) {
        let cursor = self.cursor.get();
        let start = self.window_start.get();
        let sel = self.selection.borrow();

        for (c, &th) in self.header_cells.borrow().iter().enumerate() {
            set_flag(dom, th, "data-active-col", c == cursor.col());
        }

        let rows = self.mounted_rows.borrow();
        let cells = self.mounted_cells.borrow();
        for (i, &tr) in rows.iter().enumerate() {
            let vrow = start + i;
            let row_active = vrow == cursor.row();
            set_flag(dom, tr, "data-active-row", row_active);
            let key = self.key_at(vrow);
            let mut row_selected = false;
            if let Some(row_cells) = cells.get(i) {
                for (c, &td) in row_cells.iter().enumerate() {
                    let col_active = c == cursor.col();
                    set_flag(dom, td, "data-active-col", col_active);
                    set_flag(dom, td, "data-active-cell", row_active && col_active);
                    let selected = key.as_ref().is_some_and(|k| sel.is_selected(vrow, c, k));
                    set_flag(dom, td, "data-selected", selected);
                    row_selected |= selected;
                }
            }
            // `<tr data-selected>` lets CSS mark a whole selected row (and in
            // Row mode every cell carries it too, for a full-width fill).
            set_flag(dom, tr, "data-selected", row_selected);
        }
    }

    /// Borrow the model mutably to update columns/rows. After changing
    /// data, call [`show_window`](Self::show_window) again to re-render.
    pub fn with<R>(&self, f: impl FnOnce(&mut VirtualTable) -> R) -> R {
        f(&mut self.inner.borrow_mut())
    }

    /// Number of rows currently materialized in the DOM.
    pub fn mounted_row_count(&self) -> usize {
        self.mounted_rows.borrow().len()
    }
}

/// Set (`on`) or remove (`!on`) a presence attribute on `id`.
fn set_flag(dom: &mut TuiDom, id: NodeId, attr: &str, on: bool) {
    if on {
        let _ = dom.set_attribute(id, attr, "");
    } else {
        let _ = dom.remove_attribute(id, attr);
    }
}

/// The default highlight selectors + styles for the cursor, focus-gated so
/// the highlight only shows while the table region is focused. Returned as
/// `(selector, style)` pairs so consumers can fold them into an existing
/// sheet or tweak the colors.
///
/// **Gated on `:focus-within`, not `:focus`** — with `enable_scrollbar` the
/// `<tbody>` is itself a focusable scroll container, so clicking the body
/// (e.g. the empty area past the last column) moves focus from the `<table>`
/// to the `<tbody>`. A `table:focus` gate would drop the highlight there (and
/// keep it dropped, since a subsequent cell click `prevent_default`s the
/// focus-on-click); `table:focus-within` keeps it lit whenever focus rests
/// anywhere inside the table.
///
/// The contract is three presence attributes the view writes:
/// - `data-active-row` on the `<tr>` under the cursor,
/// - `data-active-col` on every `<th>`/`<td>` in the cursor's column,
/// - `data-active-cell` on the single `<td>` at the cursor.
///
/// It also styles the **selection** ([`set_selection_mode`](VirtualTableView::set_selection_mode)):
/// - `data-selected` on every selected `<td>` (and the `<tr>` of a row with any
///   selection), painted a distinct blue (`#1e3a5f`).
///
/// Colors, in source-order precedence (all rules are equal zero-specificity
/// `:where()`, so later wins): the active row/column tint (`#181a1c`), then the
/// selection blue (`#1e3a5f`), then a brighter blue (`#2b557e`) where a
/// selected cell *also* sits in the active row/column — a pre-computed
/// "selection over the cross-hair" blend, since a TUI can't alpha-composite
/// opaque cells, so the highlight shows through instead of being flatly
/// overpainted — then the cursor cell last (so it stays visible in a
/// selection): `#2d2f31` gray normally, or the brightest blue (`#3a6ea5`) when
/// the cursor cell is itself selected, so it fits the surrounding blue field.
///
/// (As of rdom-tui 0.3.4 the UA focus tint is scoped to interactive controls,
/// so a focused `<table>` is not washed with the focus background — no
/// `table:focus { background: reset }` workaround is needed.)
///
/// Each selector is wrapped in **`:where()`** so the whole rule carries **zero
/// specificity** (it still only matches a focused table's active cells —
/// `:where()` changes specificity, not matching). That makes these true
/// *defaults*: any author rule of any specificity overrides them, exactly like
/// overriding a browser UA style. Requires rdom-tui ≥ 0.3.4.
///
/// The cursor/selection rules are also gated on
/// **`table:focus-within:not([data-vt-menu-open])`** — while the column chooser is open
/// the view stamps `data-vt-menu-open` on the `<table>`, so the cross-hair +
/// selection step aside and focus rests on the dropdown.
pub fn highlight_rules() -> Vec<(&'static str, TuiStyle)> {
    // #181a1c — shared row/column tint.
    let line = Color::Rgb(0x18, 0x1a, 0x1c);
    // #1e3a5f — selected cells (a distinct blue).
    let selected = Color::Rgb(0x1e, 0x3a, 0x5f);
    // #2b557e — a selected cell that also sits in the active row/column. A TUI
    // can't alpha-composite opaque cells, so this is the pre-computed "selection
    // over the row/column tint" — a brighter blue so the cross-hair shows
    // through the selection instead of being flatly overpainted.
    let selected_line = Color::Rgb(0x2b, 0x55, 0x7e);
    // #2d2f31 — the cursor cell by default: a neutral gray matching rdom's focus
    // tint (inputs/tree). When the cursor cell is itself *selected* it sits in
    // the blue selection field, so a gray fill reads oddly — it switches to the
    // brightest blue (`cell_blue`) so it's the focal point of the blue family.
    let cell_gray = Color::Rgb(0x2d, 0x2f, 0x31);
    let cell_blue = Color::Rgb(0x3a, 0x6e, 0xa5);
    vec![
        // A drag over the cells rubber-bands a cell range (install_mouse), so
        // suppress the default text drag-selection. `user-select` inherits, so
        // setting it on the table covers every cell.
        ("table", TuiStyle::new().user_select(UserSelect::None)),
        (
            ":where(table:focus-within:not([data-vt-menu-open]) tr[data-active-row])",
            TuiStyle::new().bg(line),
        ),
        (
            ":where(table:focus-within:not([data-vt-menu-open]) th[data-active-col])",
            TuiStyle::new().bg(line),
        ),
        (
            ":where(table:focus-within:not([data-vt-menu-open]) td[data-active-col])",
            TuiStyle::new().bg(line),
        ),
        (
            ":where(table:focus-within:not([data-vt-menu-open]) td[data-selected])",
            TuiStyle::new().bg(selected),
        ),
        // Selection ∩ row/column highlight → the blend (listed after the plain
        // selection so it wins on the intersection; all rules are equal
        // zero-specificity `:where()`, so source order decides).
        (
            ":where(table:focus-within:not([data-vt-menu-open]) td[data-selected][data-active-col])",
            TuiStyle::new().bg(selected_line),
        ),
        (
            ":where(table:focus-within:not([data-vt-menu-open]) tr[data-active-row] td[data-selected])",
            TuiStyle::new().bg(selected_line),
        ),
        // The cursor cell wins last, so it stays visible inside a selection.
        // Gray normally; a brighter blue when the cursor cell is itself
        // selected (it sits in the blue field, so blue fits).
        (
            ":where(table:focus-within:not([data-vt-menu-open]) td[data-active-cell])",
            TuiStyle::new().bg(cell_gray),
        ),
        (
            ":where(table:focus-within:not([data-vt-menu-open]) td[data-active-cell][data-selected])",
            TuiStyle::new().bg(cell_blue),
        ),
        // Focused-scroll affordance (rdom `FOCUS-VOCAB-1`): a focused scroll
        // region shows an accent (DodgerBlue) thumb glyph. Gated on
        // `table:focus-within tbody` so it accents the body scrollbar thumb
        // whenever focus rests anywhere in the table — whether on the `<table>`
        // itself (keyboard) or on the `<tbody>` scroll container (after a click
        // in the body), matching the cross-hair's `:focus-within` gate above.
        (
            ":where(table:focus-within tbody)::scrollbar-thumb",
            TuiStyle::new().fg(Color::Rgb(30, 144, 255)),
        ),
        // Hidden columns: the view stamps `data-vt-hidden` on a hidden column's
        // header + cells; this maps it to `display: none`. Zero-specificity, so
        // a consumer can override (e.g. render hidden columns collapsed instead).
        (":where([data-vt-hidden])", {
            let mut s = TuiStyle::new();
            s.display = Some(Value::Specified(Display::None));
            s
        }),
        // Open overflow chip: while its dropdown is open the chip carries
        // `data-vt-menu-open` and reads as the panel's tab — `▐…▌`, soft
        // half-block side edges in the dropdown's bg color, whose right edge
        // lines up with the panel's right edge directly below (the panel anchors
        // `right: 0` to the chip). Padding drops to 0 so the border cells replace
        // the UA `<th>` side padding (chip width 3 = `▐` + `…` + `▌`). NOT wrapped
        // in `:where()` — it must outrank the UA `th { padding }` rule to claim
        // those side cells; the cursor/selection rules below stay low-specificity
        // for consumer overriding, but this is structural chrome, not a tint.
        (
            "th[data-vt-overflow][data-vt-menu-open]",
            TuiStyle::new()
                .bg(columns::MENU_BG)
                .padding(Padding::all(0))
                .border(Border {
                    left: BorderStyle::HalfBlock,
                    right: BorderStyle::HalfBlock,
                    ..Border::none()
                })
                .border_fg(columns::MENU_BG),
        ),
        // Keyboard highlight inside the open dropdown: the focused row carries
        // `data-vt-menu-active` (Up/Down move it). A brighter blue than the
        // panel bg so the selection reads clearly.
        (
            ":where([data-vt-menu-item][data-vt-menu-active])",
            TuiStyle::new().bg(Color::Rgb(0x2b, 0x55, 0x7e)),
        ),
    ]
}

/// A ready-made [`Stylesheet`] with the default cursor/selection highlight
/// from [`highlight_rules`]. Drop it straight into `App::new`, or merge the
/// rules into your own sheet for custom colors.
pub fn highlight_stylesheet() -> Stylesheet {
    let mut sheet = Stylesheet::new();
    for (selector, style) in highlight_rules() {
        sheet = sheet
            .rule(selector, style)
            .expect("built-in highlight selectors parse");
    }
    sheet
}
