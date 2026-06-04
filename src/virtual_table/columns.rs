//! Column operations on [`VirtualTableView`] — sort, reorder, hide/show, and
//! the header sort indicator. A child module of `virtual_table`, so it reaches
//! the view's private fields while keeping the impl off the core view file.

use rdom_tui::layout::{Length, Position, ZIndex};
use rdom_tui::runtime::builtins::table::size_columns;
use rdom_tui::{
    Color, ListenerOptions, NodeId, Padding, Size, TuiDom, TuiNodeMutExt, TuiStyle, Value,
};

use super::VirtualTableView;
use crate::model::{SortDir, VirtualTable};

/// Presence attribute marking the trailing overflow chip `<th>`.
const OVERFLOW_ATTR: &str = "data-vt-overflow";
/// Presence attribute marking the floating show/hide dropdown `<div>`.
const MENU_ATTR: &str = "data-vt-menu";
/// Presence attribute set on the chip while its dropdown is open (the default
/// sheet highlights it so it reads as the panel's tab).
const MENU_OPEN_ATTR: &str = "data-vt-menu-open";
/// Presence attribute marking one clickable row in the dropdown.
const MENU_ITEM_ATTR: &str = "data-vt-menu-item";
/// Carries the column index a menu row unhides (read on click).
const MENU_COL_ATTR: &str = "data-vt-col";
/// Presence attribute on the keyboard-highlighted dropdown row.
const MENU_ACTIVE_ATTR: &str = "data-vt-menu-active";
/// Fixed width of the overflow chip (keeps it from grabbing flex space).
const CHIP_WIDTH: u16 = 3;
/// Cells the native checkbox glyph occupies in a chooser row (`"[x] "`, from
/// the UA `[type=checkbox]::before` content).
const CHECKBOX_W: u16 = 4;
/// Dropdown z-index — above the body (paint sorts by `(z_index, doc_order)`).
const MENU_Z: i16 = 1000;
/// Dropdown background — opaque so it reads over the rows beneath it. Also the
/// open-chip highlight (the chip + panel share a bg, so they read as connected).
pub(super) const MENU_BG: Color = Color::Rgb(0x22, 0x24, 0x26);

impl VirtualTableView {
    /// Current sort `(column, direction)`, or `None` if unsorted.
    pub fn sort_state(&self) -> Option<(usize, SortDir)> {
        self.inner.borrow().sort_state()
    }

    /// Set the sort-direction glyph suffixes appended to the sorted header
    /// text — default `(" ▲", " ▼")`. Include any leading separator yourself
    /// (e.g. `" ^"`). Use **narrow (width-1) glyphs** if your terminal renders
    /// East-Asian *ambiguous-width* characters (`▲`/`▼`, `↑`/`↓`) double-width,
    /// which otherwise shifts header columns after the sorted one by one cell;
    /// `("", "")` disables the glyph (keeping only the `data-sort` attribute).
    /// Takes effect on the next [`sort`](Self::sort) / re-render.
    pub fn set_sort_glyphs(&self, ascending: impl Into<String>, descending: impl Into<String>) {
        *self.sort_glyphs.borrow_mut() = (ascending.into(), descending.into());
    }

    /// Sort by `col` in `dir`, re-materialize the visible window in the new
    /// order, and mark the header (`data-sort="asc|desc"`, which the default
    /// sheet turns into a `▲`/`▼` glyph). The cursor keeps its position; the
    /// **selection is cleared** — it's keyed by row index, which now points at
    /// different data after the reorder. Pass a custom comparator by calling
    /// [`VirtualTable::sort_by_with`] via [`with`](Self::with) then
    /// [`refresh`](Self::refresh) yourself.
    pub fn sort(&self, dom: &mut TuiDom, col: usize, dir: SortDir) {
        self.inner.borrow_mut().sort_by(col, dir);
        self.selection.borrow_mut().clear();
        // Mark the header (and append the glyph to its text) *before* refresh,
        // so `size_columns` measures the glyph and the column is wide enough.
        self.apply_sort_indicator(dom);
        self.refresh(dom);
    }

    /// Toggle the sort on `col`: ascending the first time, then flipping
    /// asc⇄desc on each subsequent call. Ideal for a header-click handler.
    pub fn toggle_sort(&self, dom: &mut TuiDom, col: usize) {
        let dir = match self.sort_state() {
            Some((c, d)) if c == col => d.flipped(),
            _ => SortDir::Ascending,
        };
        self.sort(dom, col, dir);
    }

    /// Move the column at `from` to index `to`: permutes the model (header +
    /// every row's cell), re-syncs the headers + sort glyph, and
    /// re-materializes the visible window in the new order. The **cursor
    /// follows** its column; the **selection is cleared** (a structural change,
    /// like sort). No-op for out-of-range or equal indices.
    pub fn move_column(&self, dom: &mut TuiDom, from: usize, to: usize) {
        let (rows, cols) = self.with(|t| (t.row_count(), t.columns().len()));
        if from >= cols || to >= cols || from == to {
            return;
        }
        self.with(|t| t.move_column(from, to));
        let cur = self.cursor.get();
        let new_col = VirtualTable::remapped_index(from, to, cur.col());
        self.cursor.set(cur.at(cur.row(), new_col, rows, cols));
        self.selection.borrow_mut().clear();
        // Headers persist across `show_window`, so re-sync their labels/glyph
        // AND their widths to the new column order *before* refresh (so a
        // resized width follows its column and `size_columns` measures right).
        self.apply_sort_indicator(dom);
        self.sync_header_widths(dom);
        self.refresh(dom);
    }

    /// Hide or show the column at `col`. A hidden column gets `data-vt-hidden`
    /// on its header `<th>` and every body cell (the default sheet maps that to
    /// `display: none`), the cursor skips it on horizontal navigation, and the
    /// flag follows the column through reordering. Hiding the **last visible**
    /// column is refused (an all-hidden table is useless). No-op for
    /// out-of-range `col` at the DOM level (the model still records it).
    pub fn set_column_hidden(&self, dom: &mut TuiDom, col: usize, hidden: bool) {
        self.apply_column_hidden(dom, col, hidden);
        // Keep an open chooser's checkboxes in sync with the model.
        if self.is_column_menu_open() {
            self.rebuild_menu_items(dom);
        }
    }

    // ── Column-actions column: header chooser (+ row actions, later) ──

    /// Enable the **column-actions column**: a persistent trailing header cell
    /// (the `…` chip) whose dropdown is a checklist of every column — check to
    /// show, uncheck to hide. Opt-in (a generic table shouldn't grow the
    /// affordance unasked), idempotent, and self-contained (the dropdown
    /// anchors to the chip, no root anchoring). Call after [`mount`](Self::mount).
    ///
    /// The body cells of this column are reserved for per-row action triggers
    /// (a follow-up); today only the header chooser is wired.
    pub fn enable_column_actions(&self, dom: &mut TuiDom) {
        let Some(header_tr) = self.header_tr.get() else {
            return;
        };
        if self.overflow_chip.get().is_some() {
            return; // already enabled
        }
        let th = dom.create_element("th");
        let text = dom.create_text_node("…");
        dom.append_child(th, text).unwrap();
        let _ = dom.set_attribute(th, OVERFLOW_ATTR, "");
        // `position: relative` makes the chip the containing block for the
        // absolutely-positioned dropdown, so the affordance is self-contained
        // inside the table subtree. Narrow fixed width so it doesn't grab flex
        // space. (rdom-tui ≥ 0.3.7 fixes the stale-anon-box double-paint a
        // relative chip + dropped absolute child used to trigger.)
        let mut s = TuiStyle::new();
        s.position = Some(Value::Specified(Position::Relative));
        dom.node_mut(th).set_inline_style(s);
        dom.node_mut(th).set_width(Size::Fixed(CHIP_WIDTH));
        dom.append_child(header_tr, th).unwrap();
        self.overflow_chip.set(Some(th));
        if let Some(table) = self.table.get() {
            size_columns(dom, table);
        }
        // Root-level delegation: clicks (chip toggle / outside dismiss) and the
        // native checkbox `change` (reconcile model + block last-visible).
        self.install_menu_clicks(dom);
        self.install_menu_changes(dom);
    }

    /// Whether the show/hide dropdown is currently open.
    pub fn is_column_menu_open(&self) -> bool {
        self.column_menu.get().is_some()
    }

    /// Open the chooser if closed, close it if open. Wire this to a key (the
    /// chip's own mouse click toggles it too). No-op until
    /// [`enable_column_actions`](Self::enable_column_actions).
    pub fn toggle_column_menu(&self, dom: &mut TuiDom) {
        if self.is_column_menu_open() {
            self.close_column_menu(dom);
        } else {
            self.open_column_menu(dom);
        }
    }

    /// Open the floating column chooser under the chip. No-op if already open
    /// or before [`enable_column_actions`](Self::enable_column_actions).
    pub fn open_column_menu(&self, dom: &mut TuiDom) {
        if self.is_column_menu_open() {
            return;
        }
        let Some(chip) = self.overflow_chip.get() else {
            return;
        };
        // The overlay is a child of the chip (its `position: relative`
        // containing block) — the affordance stays entirely within the table
        // subtree. `rebuild_menu_items` sizes + positions it.
        let menu = dom.create_element("div");
        let _ = dom.set_attribute(menu, MENU_ATTR, "");
        dom.append_child(chip, menu).unwrap();
        self.column_menu.set(Some(menu));
        // Mark the chip active so the default sheet highlights it as the
        // panel's tab while open.
        let _ = dom.set_attribute(chip, MENU_OPEN_ATTR, "");
        // Keyboard focus starts on the first row.
        self.menu_cursor.set(0);
        self.rebuild_menu_items(dom);
    }

    /// Close the dropdown (drops the overlay subtree). The chip stays as long
    /// as a column is still hidden.
    pub fn close_column_menu(&self, dom: &mut TuiDom) {
        if let Some(menu) = self.column_menu.take() {
            let _ = dom.drop_subtree(menu);
        }
        if let Some(chip) = self.overflow_chip.get() {
            let _ = dom.remove_attribute(chip, MENU_OPEN_ATTR);
        }
    }

    /// Repopulate the open chooser: one row per column (in column order), built
    /// like HTML — a `<label>` wrapping a native `<input type="checkbox">`
    /// (checked = visible) and the column name. The native checkbox renders the
    /// `[x]`/`[ ]` glyph and toggles itself on click (the label forwards the
    /// click); a `change` listener reconciles the model. No-op when closed.
    fn rebuild_menu_items(&self, dom: &mut TuiDom) {
        let Some(menu) = self.column_menu.get() else {
            return;
        };
        let existing: Vec<NodeId> = dom.node(menu).child_nodes().map(|c| c.id()).collect();
        for id in existing {
            let _ = dom.drop_subtree(id);
        }
        // (index, label, visible) for every column, in order.
        let cols: Vec<(usize, String, bool)> = {
            let m = self.inner.borrow();
            m.columns()
                .iter()
                .enumerate()
                .map(|(i, c)| (i, c.header.clone(), !m.is_column_hidden(i)))
                .collect()
        };

        // Float the panel just under the chip, anchored to the chip's own
        // (position:relative) box — `top: 1` one row below, `right: 0` aligns its
        // right edge with the chip's so it grows leftward. An absolutely-
        // positioned box with `width: auto` collapses to zero (no shrink-to-fit),
        // so width/height are explicit: the checkbox glyph (`[x] `) + the widest
        // label, plus `padding: 0 1`, by one row per column.
        let label_w = cols
            .iter()
            .map(|(_, l, _)| l.chars().count())
            .max()
            .unwrap_or(0);
        let width = (CHECKBOX_W + label_w as u16).saturating_add(2).max(1);
        let height = (cols.len() as u16).max(1);
        let mut s = TuiStyle::new()
            .bg(MENU_BG)
            .width(Size::Fixed(width))
            .height(Size::Fixed(height))
            .padding(Padding::symmetric(1, 0)); // 0 1 — inset rows from the edges
        s.position = Some(Value::Specified(Position::Absolute));
        s.top = Some(Value::Specified(Length::Cells(1)));
        s.right = Some(Value::Specified(Length::Cells(0)));
        s.z_index = Some(Value::Specified(ZIndex::Value(MENU_Z)));
        dom.node_mut(menu).set_inline_style(s);

        let count = cols.len();
        for (col, label, visible) in cols {
            // <label data-vt-menu-item data-vt-col=N><input type=checkbox [checked]> Name</label>
            let row = dom.create_element("label");
            let _ = dom.set_attribute(row, MENU_ITEM_ATTR, "");
            let _ = dom.set_attribute(row, MENU_COL_ATTR, &col.to_string());
            let cb = dom.create_element("input");
            let _ = dom.set_attribute(cb, "type", "checkbox");
            if visible {
                let _ = dom.set_attribute(cb, "checked", "");
            }
            dom.append_child(row, cb).unwrap();
            let text = dom.create_text_node(&label);
            dom.append_child(row, text).unwrap();
            dom.append_child(menu, row).unwrap();
        }
        // Keep the keyboard highlight in range, then mark the focused row.
        if count > 0 {
            let cur = self.menu_cursor.get().min(count - 1);
            self.menu_cursor.set(cur);
            self.apply_menu_highlight(dom);
        }
    }

    /// Visible (non-hidden) column count.
    fn visible_count(&self) -> usize {
        let m = self.inner.borrow();
        m.columns().len()
            - (0..m.columns().len())
                .filter(|&i| m.is_column_hidden(i))
                .count()
    }

    /// Apply a column's hidden state to the model + body WITHOUT rebuilding the
    /// open chooser (so it's safe to call from the checkbox `change` handler,
    /// which must not drop the checkbox mid-dispatch). Blocks hiding the last
    /// visible column. Returns the hidden state actually applied.
    fn apply_column_hidden(&self, dom: &mut TuiDom, col: usize, hidden: bool) -> bool {
        if hidden && self.visible_count() <= 1 && !self.inner.borrow().is_column_hidden(col) {
            return false; // refuse to hide the last visible column
        }
        self.inner.borrow_mut().set_column_hidden(col, hidden);
        if let Some(&th) = self.header_cells.borrow().get(col) {
            super::set_flag(dom, th, "data-vt-hidden", hidden);
        }
        self.refresh(dom);
        hidden
    }

    /// Mark the `menu_cursor`-th dropdown row with `data-vt-menu-active` and
    /// clear it from the rest. No-op when the menu is closed.
    fn apply_menu_highlight(&self, dom: &mut TuiDom) {
        let Some(menu) = self.column_menu.get() else {
            return;
        };
        let cur = self.menu_cursor.get();
        let items: Vec<NodeId> = dom
            .node(menu)
            .child_nodes()
            .filter(|c| c.get_attribute(MENU_ITEM_ATTR).is_some())
            .map(|c| c.id())
            .collect();
        for (i, &item) in items.iter().enumerate() {
            super::set_flag(dom, item, MENU_ACTIVE_ATTR, i == cur);
        }
    }

    /// Move the chooser's keyboard highlight by `delta` rows (clamped). The
    /// chooser lists every column, so the cursor ranges over all columns. No-op
    /// when closed.
    pub fn menu_highlight_move(&self, dom: &mut TuiDom, delta: isize) {
        if !self.is_column_menu_open() {
            return;
        }
        let count = self.inner.borrow().columns().len();
        if count == 0 {
            return;
        }
        let cur = self.menu_cursor.get() as isize;
        let next = (cur + delta).clamp(0, count as isize - 1) as usize;
        self.menu_cursor.set(next);
        self.apply_menu_highlight(dom);
    }

    /// Toggle the highlighted column's visibility (Enter/Space), then rebuild
    /// the chooser so the checkbox reflects it. Refuses to hide the last visible
    /// column. No-op when closed. (Keyboard context — the rebuild is safe here,
    /// unlike the mouse `change` path which must not drop the live checkbox.)
    pub fn menu_activate(&self, dom: &mut TuiDom) {
        if !self.is_column_menu_open() {
            return;
        }
        let col = self.menu_cursor.get();
        let now_hidden = self.inner.borrow().is_column_hidden(col);
        self.apply_column_hidden(dom, col, !now_hidden);
        self.rebuild_menu_items(dom);
    }

    /// Install the root-level `click` delegation (once, from `mount`). In the
    /// bubble phase — after the full event path — so reconciling the chip/menu
    /// (which drops subtrees) is safe. Routes: a menu-item click unhides that
    /// column; a chip click toggles the dropdown; any other click while open
    /// dismisses it.
    pub(super) fn install_menu_clicks(&self, dom: &mut TuiDom) {
        let root = dom.root();
        let view = self.clone();
        dom.add_event_listener(root, "click", ListenerOptions::default(), move |ctx| {
            let Some(target) = ctx.event.target else {
                return;
            };
            let menu_open = view.is_column_menu_open();
            // 1) A click *inside* the open chooser (a checkbox / its label) is a
            //    native toggle — the label + checkbox builtins flip it and fire
            //    `change`, which `install_menu_changes` reconciles. Leave it be.
            if menu_open && ctx.dom.node(target).closest("[data-vt-menu]").is_some() {
                return;
            }
            // 2) The chip glyph (in the chip but outside the menu) → toggle.
            if let Some(chip) = view.overflow_chip.get() {
                if ctx.dom.node(chip).contains(target) {
                    view.toggle_column_menu(ctx.dom);
                    ctx.request_redraw();
                    return;
                }
            }
            // 3) Anywhere else while open → dismiss.
            if menu_open {
                view.close_column_menu(ctx.dom);
                ctx.request_redraw();
            }
        })
        .expect("root accepts a click listener");
    }

    /// Install the root-level `change` listener that reconciles the model when a
    /// chooser checkbox toggles (native click / Space). Reads the checkbox's new
    /// `checked` and applies `hidden = !checked` to its `data-vt-col` column —
    /// without rebuilding (the live checkbox must survive the dispatch). Hiding
    /// the last visible column is refused: the checkbox is re-checked.
    fn install_menu_changes(&self, dom: &mut TuiDom) {
        let root = dom.root();
        let view = self.clone();
        dom.add_event_listener(root, "change", ListenerOptions::default(), move |ctx| {
            let Some(target) = ctx.event.target else {
                return;
            };
            // The changed checkbox's row → column index.
            let col = ctx
                .dom
                .node(target)
                .closest("[data-vt-menu-item]")
                .and_then(|row| row.get_attribute(MENU_COL_ATTR))
                .and_then(|s| s.parse::<usize>().ok());
            let Some(col) = col else {
                return;
            };
            let checked = ctx.dom.node(target).has_attribute("checked");
            let applied = view.apply_column_hidden(ctx.dom, col, !checked);
            // Refused (last visible column): re-check the box so glyph + model
            // agree again.
            if !checked && !applied {
                let _ = ctx.dom.set_attribute(target, "checked", "");
            }
            ctx.request_redraw();
        })
        .expect("root accepts a change listener");
    }

    /// Set column `col` to an explicit `width` (its cells clip/wrap to it), or
    /// `None` to return it to content-auto sizing. The width is stored on the
    /// **model** (`Column::width`), so it **follows the column through a
    /// reorder** — then applied to the header `<th>` as an inline width, which
    /// rdom-tui ≥ 0.3.6 (`TABLE-COLSYNC-1`) respects and propagates to the whole
    /// column. Persists across window changes (the header isn't re-materialized).
    pub fn set_column_width(&self, dom: &mut TuiDom, col: usize, width: Option<u16>) {
        self.inner.borrow_mut().set_column_width(col, width);
        self.sync_header_widths(dom);
        self.refresh(dom);
    }

    /// Apply each column's model width (`Column::width`) to its header `<th>`'s
    /// inline width — the source `size_columns` reads. Re-run after a resize or
    /// reorder so a width tracks its column (the model permutes; the `<th>`
    /// nodes don't move, so their widths must be re-applied from the model).
    fn sync_header_widths(&self, dom: &mut TuiDom) {
        let widths: Vec<Option<u16>> = self
            .inner
            .borrow()
            .columns()
            .iter()
            .map(|c| c.width)
            .collect();
        for (c, &th) in self.header_cells.borrow().iter().enumerate() {
            let w = widths.get(c).copied().flatten();
            dom.node_mut(th)
                .set_width(w.map_or(Size::Auto, Size::Fixed));
        }
    }

    /// The column's current *used* width (after content/explicit resolution),
    /// or `None` before the first layout / for an out-of-range column. Useful
    /// for relative resize, e.g.
    /// `set_column_width(col, Some(column_width(dom, col)? + 1))`.
    pub fn column_width(&self, dom: &TuiDom, col: usize) -> Option<u16> {
        let th = *self.header_cells.borrow().get(col)?;
        dom.node(th).ext().and_then(|e| e.table_used_width)
    }

    /// Reflect the model's sort state onto the headers: `data-sort="asc|desc"`
    /// on the sorted `<th>` (the CSS contract), removed from the rest, and a
    /// `▲`/`▼` glyph appended to the sorted header's **text**. Headers persist
    /// across `show_window` (built once at `mount`), so this only runs on sort.
    ///
    /// The glyph is rendered as header text rather than the cleaner
    /// `th[data-sort]::after` CSS pseudo-element because the substrate's
    /// `size_columns` measures only text-node width (and runs before cascade),
    /// so an `::after` glyph would be clipped by the auto-computed column width.
    /// See `STATE.md` (substrate-friction backlog).
    pub(super) fn apply_sort_indicator(&self, dom: &mut TuiDom) {
        let state = self.inner.borrow().sort_state();
        let headers = self.header_cells.borrow();
        let model = self.inner.borrow();
        let glyphs = self.sort_glyphs.borrow();
        for (c, &th) in headers.iter().enumerate() {
            let label = model.columns().get(c).map_or("", |col| col.header.as_str());
            let (attr, glyph) = match state {
                Some((sc, SortDir::Ascending)) if sc == c => (Some("asc"), glyphs.0.as_str()),
                Some((sc, SortDir::Descending)) if sc == c => (Some("desc"), glyphs.1.as_str()),
                _ => (None, ""),
            };
            match attr {
                Some(v) => {
                    let _ = dom.set_attribute(th, "data-sort", v);
                }
                None => {
                    let _ = dom.remove_attribute(th, "data-sort");
                }
            }
            // Reset the header text to `label (+ glyph)`; the first child of a
            // header `<th>` is its text node (built that way in `mount`).
            if let Some(text_id) = dom.node(th).child_nodes().next().map(|n| n.id()) {
                let _ = dom
                    .node_mut(text_id)
                    .set_node_value(&format!("{label}{glyph}"));
            }
        }
    }
}
