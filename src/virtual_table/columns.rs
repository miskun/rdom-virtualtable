//! Column operations on [`VirtualTableView`] — sort, reorder, hide/show, and
//! the header sort indicator. A child module of `virtual_table`, so it reaches
//! the view's private fields while keeping the impl off the core view file.

use rdom_tui::layout::{Length, Position, ZIndex};
use rdom_tui::runtime::builtins::table::size_columns;
use rdom_tui::{
    Color, ListenerOptions, NodeId, Size, TuiDom, TuiNodeExt, TuiNodeMutExt, TuiStyle, Value,
};

use super::VirtualTableView;
use crate::model::{SortDir, VirtualTable};

/// Presence attribute marking the trailing overflow chip `<th>`.
const OVERFLOW_ATTR: &str = "data-vt-overflow";
/// Presence attribute marking the floating show/hide dropdown `<div>`.
const MENU_ATTR: &str = "data-vt-menu";
/// Presence attribute marking one clickable row in the dropdown.
const MENU_ITEM_ATTR: &str = "data-vt-menu-item";
/// Carries the column index a menu row unhides (read on click).
const MENU_COL_ATTR: &str = "data-vt-col";
/// Fixed width of the overflow chip (keeps it from grabbing flex space).
const CHIP_WIDTH: u16 = 3;
/// Dropdown z-index — above the body (paint sorts by `(z_index, doc_order)`).
const MENU_Z: i16 = 1000;
/// Dropdown background — opaque so it reads over the rows beneath it.
const MENU_BG: Color = Color::Rgb(0x22, 0x24, 0x26);

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
    /// flag follows the column through reordering. No-op for out-of-range `col`
    /// at the DOM level (the model still records it).
    pub fn set_column_hidden(&self, dom: &mut TuiDom, col: usize, hidden: bool) {
        self.inner.borrow_mut().set_column_hidden(col, hidden);
        if let Some(&th) = self.header_cells.borrow().get(col) {
            super::set_flag(dom, th, "data-vt-hidden", hidden);
        }
        // Re-materialize so the body cells pick up (or drop) the attribute.
        self.refresh(dom);
        // Add/remove the "more columns" chip (and keep an open menu in sync).
        self.sync_overflow_chip(dom);
    }

    // ── Show/hide overflow chip + dropdown ───────────────────────────

    /// Whether the show/hide dropdown is currently open.
    pub fn is_column_menu_open(&self) -> bool {
        self.column_menu.get().is_some()
    }

    /// Open the dropdown if closed, close it if open. Wire this to a key (the
    /// chip's own mouse click is handled internally). No-op when no column is
    /// hidden (there's nothing to show, so no chip exists).
    pub fn toggle_column_menu(&self, dom: &mut TuiDom) {
        if self.is_column_menu_open() {
            self.close_column_menu(dom);
        } else {
            self.open_column_menu(dom);
        }
    }

    /// Open the floating show/hide dropdown under the overflow chip. No-op if
    /// already open or if nothing is hidden.
    pub fn open_column_menu(&self, dom: &mut TuiDom) {
        if self.is_column_menu_open() {
            return;
        }
        if self.overflow_chip.get().is_none() {
            return;
        }
        if self.inner.borrow().hidden_columns().is_empty() {
            return;
        }
        // The overlay is a child of the ROOT (a viewport-positioned layer), not
        // the chip — see the chip-creation note for why the chip must not be a
        // positioned ancestor. `rebuild_menu_items` positions it from the chip's
        // measured rect and sets the rest of the style.
        let root = dom.root();
        let menu = dom.create_element("div");
        let _ = dom.set_attribute(menu, MENU_ATTR, "");
        dom.append_child(root, menu).unwrap();
        self.column_menu.set(Some(menu));
        self.rebuild_menu_items(dom);
    }

    /// Close the dropdown (drops the overlay subtree). The chip stays as long
    /// as a column is still hidden.
    pub fn close_column_menu(&self, dom: &mut TuiDom) {
        if let Some(menu) = self.column_menu.take() {
            let _ = dom.drop_subtree(menu);
        }
    }

    /// Reconcile the overflow chip with the hidden set: create it when the
    /// first column hides, drop it (and any open menu) when the last one shows.
    /// When the menu is open and the hidden set changed, refresh its rows.
    fn sync_overflow_chip(&self, dom: &mut TuiDom) {
        let Some(header_tr) = self.header_tr.get() else {
            return;
        };
        let any_hidden = !self.inner.borrow().hidden_columns().is_empty();
        if any_hidden {
            if self.overflow_chip.get().is_none() {
                let th = dom.create_element("th");
                let text = dom.create_text_node("…");
                dom.append_child(th, text).unwrap();
                let _ = dom.set_attribute(th, OVERFLOW_ATTR, "");
                // A plain static cell with a narrow fixed width (keeps it from
                // stealing flex space). The dropdown anchors to the *root* using
                // this chip's measured rect — NOT as a positioned child of the
                // chip — so the chip never establishes a stacking context (a
                // `position: relative` chip with an absolute child painted twice
                // under the flex header row; see STATE.md).
                dom.node_mut(th).set_width(Size::Fixed(CHIP_WIDTH));
                dom.append_child(header_tr, th).unwrap();
                self.overflow_chip.set(Some(th));
                // The new header cell needs a column width.
                if let Some(table) = self.table.get() {
                    size_columns(dom, table);
                }
            } else if self.is_column_menu_open() {
                self.rebuild_menu_items(dom);
            }
        } else {
            // Tear down the menu first (it's a child of the chip), then the chip.
            self.close_column_menu(dom);
            if let Some(th) = self.overflow_chip.take() {
                let _ = dom.drop_subtree(th);
                if let Some(table) = self.table.get() {
                    size_columns(dom, table);
                }
            }
        }
    }

    /// Repopulate the open dropdown with one clickable row per hidden column
    /// (sorted by index), each tagged `data-vt-col` so a click knows which
    /// column to bring back. No-op when the menu is closed.
    fn rebuild_menu_items(&self, dom: &mut TuiDom) {
        let Some(menu) = self.column_menu.get() else {
            return;
        };
        let existing: Vec<NodeId> = dom.node(menu).child_nodes().map(|c| c.id()).collect();
        for id in existing {
            let _ = dom.drop_subtree(id);
        }
        let hidden: Vec<(usize, String)> = self
            .inner
            .borrow()
            .hidden_columns()
            .iter()
            .map(|&(i, label)| (i, label.to_string()))
            .collect();

        // Float the panel just under the chip, sized to its content. It's
        // positioned against the VIEWPORT (root child), so its top/left come
        // from the chip's measured layout rect. An absolutely-positioned box
        // with `width: auto` collapses to zero (no shrink-to-fit), so width and
        // height are explicit: the widest label (+ a padding column each side)
        // by one row per hidden column.
        let label_w = hidden
            .iter()
            .map(|(_, l)| l.chars().count())
            .max()
            .unwrap_or(0);
        let width = (label_w as u16).saturating_add(2).max(1);
        let height = (hidden.len() as u16).max(1);
        // Drop below the chip; right-align the panel's right edge with the
        // chip's so it stays on-screen when the chip sits near the right edge.
        let chip_rect = self
            .overflow_chip
            .get()
            .and_then(|c| dom.node(c).layout_rect());
        let (top, left) = match chip_rect {
            Some(r) => {
                let right_edge = r.x + r.width as i32;
                let left = (right_edge - width as i32).max(0);
                ((r.y + r.height as i32) as i16, left as i16)
            }
            None => (1, 0),
        };
        let mut s = TuiStyle::new()
            .bg(MENU_BG)
            .width(Size::Fixed(width))
            .height(Size::Fixed(height));
        s.position = Some(Value::Specified(Position::Absolute));
        s.top = Some(Value::Specified(Length::Cells(top)));
        s.left = Some(Value::Specified(Length::Cells(left)));
        s.z_index = Some(Value::Specified(ZIndex::Value(MENU_Z)));
        dom.node_mut(menu).set_inline_style(s);

        for (col, label) in hidden {
            let item = dom.create_element("div");
            let _ = dom.set_attribute(item, MENU_ITEM_ATTR, "");
            let _ = dom.set_attribute(item, MENU_COL_ATTR, &col.to_string());
            let text = dom.create_text_node(&label);
            dom.append_child(item, text).unwrap();
            dom.append_child(menu, item).unwrap();
        }
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
            // 1) A menu row → unhide its column (set_column_hidden reconciles
            //    the menu/chip). Scope the read so the immutable borrow ends
            //    before the mutable `set_column_hidden`.
            if menu_open {
                let col = ctx
                    .dom
                    .node(target)
                    .closest("[data-vt-menu-item]")
                    .and_then(|item| item.get_attribute(MENU_COL_ATTR))
                    .and_then(|s| s.parse::<usize>().ok());
                if let Some(col) = col {
                    view.set_column_hidden(ctx.dom, col, false);
                    ctx.request_redraw();
                    return;
                }
            }
            // 2) The chip → toggle the dropdown.
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
