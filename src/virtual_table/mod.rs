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
use std::rc::Rc;

use rdom_tui::runtime::builtins::table::size_columns;
use rdom_tui::{
    Color, Display, ListenerOptions, NodeId, Overflow, Size, Stylesheet, TuiAccessors,
    TuiAccessorsMut, TuiDom, TuiNodeMutExt, TuiStyle, Value,
};

use crate::grid_cursor::{GridCursor, Nav, nav_for_key};
use crate::model::VirtualTable;
use crate::selection::{GridSelection, SelectionMode};

/// Column operations (sort / reorder / hide / sort indicator) — `impl
/// VirtualTableView` blocks kept off this file.
mod columns;

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
    /// Index of the highlighted row in the open dropdown (into the current
    /// hidden-columns list). Meaningful only while the menu is open; reset to 0
    /// on open and clamped as the list shrinks.
    menu_cursor: Rc<Cell<usize>>,
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
            viewport_rows: Rc::new(Cell::new(0)),
            window_start: Rc::new(Cell::new(0)),
            nav_active: Rc::new(Cell::new(false)),
            sort_glyphs: Rc::new(RefCell::new((" \u{25B2}".into(), " \u{25BC}".into()))),
            scroll_mode: Rc::new(Cell::new(false)),
            spacers: Rc::new(RefCell::new(Vec::new())),
            header_tr: Rc::new(Cell::new(None)),
            overflow_chip: Rc::new(Cell::new(None)),
            column_menu: Rc::new(Cell::new(None)),
            menu_cursor: Rc::new(Cell::new(0)),
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
        // Root-level click delegation for the overflow chip / dropdown:
        // chip-toggle, item-unhide, and outside-click dismiss (see
        // `install_menu_clicks`). Installed once, no-ops until a chip exists.
        self.install_menu_clicks(dom);
        table
    }

    /// Materialize rows `[start, start + count)` into the `<tbody>`,
    /// dropping any previously-materialized rows. No-op before `mount`.
    pub fn show_window(&self, dom: &mut TuiDom, start: usize, count: usize) {
        let Some(tbody) = self.tbody.get() else {
            return;
        };

        // Drop the previous window's rows + spacers (frees the arena slots).
        for id in self.mounted_rows.borrow_mut().drain(..) {
            let _ = dom.drop_subtree(id);
        }
        for id in self.spacers.borrow_mut().drain(..) {
            let _ = dom.drop_subtree(id);
        }
        self.mounted_cells.borrow_mut().clear();

        let total = self.inner.borrow().row_count();
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

        let model = self.inner.borrow();
        let ncols = model.columns().len();
        let span = end.saturating_sub(start);
        let mut mounted = Vec::with_capacity(span);
        let mut mounted_cells = Vec::with_capacity(span);
        for row in &model.rows()[start.min(model.rows().len())..end] {
            let tr = dom.create_element("tr");
            let mut row_cells = Vec::with_capacity(ncols);
            for c in 0..ncols {
                let td = dom.create_element("td");
                let cell = row.get(c).map(String::as_str).unwrap_or("");
                let text = dom.create_text_node(cell);
                dom.append_child(td, text).unwrap();
                if model.is_column_hidden(c) {
                    set_flag(dom, td, "data-vt-hidden", true);
                }
                dom.append_child(tr, td).unwrap();
                row_cells.push(td);
            }
            dom.append_child(tbody, tr).unwrap();
            mounted.push(tr);
            mounted_cells.push(row_cells);
        }
        drop(model);

        // Bottom spacer for the rows below the window (see top spacer above).
        if self.scroll_mode.get() && end < total {
            let sp = self.make_spacer(dom, total - end);
            dom.append_child(tbody, sp).unwrap();
            self.spacers.borrow_mut().push(sp);
        }

        *self.mounted_rows.borrow_mut() = mounted;
        *self.mounted_cells.borrow_mut() = mounted_cells;
        self.window_start.set(start);

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

    /// Set the number of visible data rows. This drives scroll-follow (how
    /// far the cursor can travel before the window shifts) and the `PageUp`/
    /// `PageDown` step. Enabling a viewport also engages navigation, so
    /// subsequent [`show_window`](Self::show_window) calls reassert the
    /// cursor highlight.
    pub fn set_viewport_rows(&self, rows: u16) {
        self.viewport_rows.set(rows);
        self.nav_active.set(true);
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

    /// Move the cursor per `nav`, clamped + scrolled into view, and return
    /// `(before, after)`. Caller updates the selection, then calls
    /// [`refresh_after_cursor`](Self::refresh_after_cursor). Returns `None`
    /// for an empty grid.
    fn move_cursor(&self, nav: Nav) -> Option<(GridCursor, GridCursor)> {
        let (rows, cols) = self.with(|t| (t.row_count(), t.columns().len()));
        if rows == 0 || cols == 0 {
            return None;
        }
        let viewport = self.viewport_rows.get() as usize;
        let before = self.cursor.get();
        let mut after = before
            .navigate(nav, rows, cols, viewport.max(1))
            .follow(viewport, rows);
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

    /// Re-materialize the window if the cursor scrolled it, else just re-apply
    /// the highlight + selection attributes.
    fn refresh_after_cursor(&self, dom: &mut TuiDom, after: GridCursor) {
        // Native-scrollbar mode: a single write direction — move the scrollbar
        // to keep the cursor visible and let the `scroll` listener re-window +
        // re-highlight. If the cursor stayed within the current window (no
        // scroll change) the listener won't fire, so re-highlight here.
        if self.scroll_mode.get() {
            let scrolled = after.scroll() != self.window_start.get();
            if let Some(tbody) = self.tbody.get() {
                let _ = dom.node_mut(tbody).set_scroll_top(after.scroll() as i32);
            }
            if !scrolled {
                self.apply_highlight(dom);
            }
            return;
        }
        let viewport = self.viewport_rows.get() as usize;
        if viewport > 0 && after.scroll() != self.window_start.get() {
            let rows = self.with(|t| t.row_count());
            let (start, count) = VirtualTable::window_for(viewport as u16, after.scroll(), rows);
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

    /// A snapshot of the current selection — query it with
    /// [`GridSelection::is_selected`] / [`is_active`](GridSelection::is_active).
    pub fn selection(&self) -> GridSelection {
        self.selection.borrow().clone()
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
            if !sel.toggle_range() {
                sel.toggle(c.row(), c.col());
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

    /// Attach a `keydown` listener to `table` that drives [`navigate`] with
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
            let mut row_selected = false;
            if let Some(row_cells) = cells.get(i) {
                for (c, &td) in row_cells.iter().enumerate() {
                    let col_active = c == cursor.col();
                    set_flag(dom, td, "data-active-col", col_active);
                    set_flag(dom, td, "data-active-cell", row_active && col_active);
                    let selected = sel.is_selected(vrow, c);
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
/// the highlight only shows while the table is focused. Returned as
/// `(selector, style)` pairs so consumers can fold them into an existing
/// sheet or tweak the colors.
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
        (
            ":where(table:focus tr[data-active-row])",
            TuiStyle::new().bg(line),
        ),
        (
            ":where(table:focus th[data-active-col])",
            TuiStyle::new().bg(line),
        ),
        (
            ":where(table:focus td[data-active-col])",
            TuiStyle::new().bg(line),
        ),
        (
            ":where(table:focus td[data-selected])",
            TuiStyle::new().bg(selected),
        ),
        // Selection ∩ row/column highlight → the blend (listed after the plain
        // selection so it wins on the intersection; all rules are equal
        // zero-specificity `:where()`, so source order decides).
        (
            ":where(table:focus td[data-selected][data-active-col])",
            TuiStyle::new().bg(selected_line),
        ),
        (
            ":where(table:focus tr[data-active-row] td[data-selected])",
            TuiStyle::new().bg(selected_line),
        ),
        // The cursor cell wins last, so it stays visible inside a selection.
        // Gray normally; a brighter blue when the cursor cell is itself
        // selected (it sits in the blue field, so blue fits).
        (
            ":where(table:focus td[data-active-cell])",
            TuiStyle::new().bg(cell_gray),
        ),
        (
            ":where(table:focus td[data-active-cell][data-selected])",
            TuiStyle::new().bg(cell_blue),
        ),
        // Focused-scroll affordance (rdom `FOCUS-VOCAB-1`): a focused scroll
        // region shows an accent (DodgerBlue) thumb glyph. The substrate's UA
        // `:focus-within::scrollbar-thumb` can't fire for `enable_scrollbar`
        // because the scroll container (`<tbody>`) is a *child* of the focused
        // `<table>` (focus is on the parent, not within the tbody) — so bridge
        // it: when the table holds focus, accent its body scrollbar thumb.
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
        // `data-vt-menu-open`. Fill its box (the "…" plus the UA `<th>` padding
        // cell on each side) with the dropdown's background so the chip reads as
        // the panel's tab. Same bg as the menu (`columns::MENU_BG`).
        (
            ":where(th[data-vt-overflow][data-vt-menu-open])",
            TuiStyle::new().bg(columns::MENU_BG),
        ),
        // Keyboard highlight inside the open dropdown: the focused row carries
        // `data-vt-menu-active` (Up/Down move it). A brighter blue than the
        // panel bg so the selection reads clearly.
        (
            ":where(div[data-vt-menu-item][data-vt-menu-active])",
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
