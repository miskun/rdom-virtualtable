//! Keyboard-navigation + highlight contract: navigating the cursor writes
//! `data-active-*` attributes onto the materialized window, the highlight
//! follows the cursor across window shifts, and the default focus-gated
//! stylesheet only paints the cursor while the table is focused.

use rdom_tui::render::{Buffer, LayoutExt, PaintExt, Rect};
use rdom_tui::style::{CascadeExt, Stylesheet};
use rdom_tui::{Color, NodeId, TuiDom, TuiStyle};
use rdom_virtualtable::{Column, Nav, VirtualTable, VirtualTableView, highlight_stylesheet};

fn grid(rows: usize, cols: usize) -> VirtualTableView {
    let columns = (0..cols).map(|c| Column::new(format!("c{c}"))).collect();
    let mut model = VirtualTable::new(columns);
    model.set_rows(
        (0..rows)
            .map(|r| (0..cols).map(|c| format!("r{r}c{c}")).collect())
            .collect(),
    );
    VirtualTableView::new(model)
}

/// Collect the `<tr>` node ids currently under `<tbody>`, in order.
fn tbody_rows(dom: &TuiDom, table: NodeId) -> Vec<NodeId> {
    for child in dom.node(table).children() {
        if child.node_name() == "tbody" {
            return child
                .children()
                .filter(|c| c.node_name() == "tr")
                .map(|c| c.id())
                .collect();
        }
    }
    Vec::new()
}

/// `<td>` node ids of a given `<tr>`, in column order.
fn row_cells(dom: &TuiDom, tr: NodeId) -> Vec<NodeId> {
    dom.node(tr)
        .children()
        .filter(|c| c.node_name() == "td")
        .map(|c| c.id())
        .collect()
}

fn header_cells(dom: &TuiDom, table: NodeId) -> Vec<NodeId> {
    for child in dom.node(table).children() {
        if child.node_name() == "thead" {
            for tr in child.children() {
                if tr.node_name() == "tr" {
                    return tr
                        .children()
                        .filter(|c| c.node_name() == "th")
                        .map(|c| c.id())
                        .collect();
                }
            }
        }
    }
    Vec::new()
}

/// Cascade + layout + paint with `sheet`, then count cells painted with `bg`.
fn count_bg(dom: &mut TuiDom, sheet: &Stylesheet, viewport: Rect, bg: Color) -> usize {
    dom.cascade(sheet);
    dom.layout_dom(viewport);
    let mut buf = Buffer::empty(viewport);
    dom.paint_dom(&mut buf, viewport);
    let mut n = 0;
    for y in viewport.y..viewport.bottom() {
        for x in viewport.x..viewport.right() {
            if let Some(c) = buf.cell(x, y) {
                if c.bg == bg {
                    n += 1;
                }
            }
        }
    }
    n
}

/// Mount a focused, navigated grid ready for paint assertions: cursor at row 1,
/// table focused, an 8-row window shown.
fn focused_navigated_grid() -> (TuiDom, NodeId) {
    let view = grid(20, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);
    view.navigate(&mut dom, Nav::Down); // cursor at row 1
    dom.set_focused(Some(table));
    (dom, table)
}

#[test]
fn highlight_marks_cursor_row_column_and_cell() {
    let view = grid(20, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);

    // Move to (row 2, col 1).
    view.navigate(&mut dom, Nav::Down);
    view.navigate(&mut dom, Nav::Down);
    view.navigate(&mut dom, Nav::Right);
    assert_eq!((view.cursor().row(), view.cursor().col()), (2, 1));

    let rows = tbody_rows(&dom, table);
    for (i, &tr) in rows.iter().enumerate() {
        let row_active = i == 2;
        assert_eq!(
            dom.has_attribute(tr, "data-active-row"),
            row_active,
            "tr {i} data-active-row"
        );
        for (c, td) in row_cells(&dom, tr).into_iter().enumerate() {
            assert_eq!(
                dom.has_attribute(td, "data-active-col"),
                c == 1,
                "tr {i} td {c} data-active-col"
            );
            assert_eq!(
                dom.has_attribute(td, "data-active-cell"),
                row_active && c == 1,
                "tr {i} td {c} data-active-cell"
            );
        }
    }

    // Header column under the cursor is flagged too.
    let headers = header_cells(&dom, table);
    assert!(dom.has_attribute(headers[1], "data-active-col"));
    assert!(!dom.has_attribute(headers[0], "data-active-col"));
}

#[test]
fn navigation_past_window_shifts_and_rehighlights() {
    let view = grid(50, 2);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(5);
    view.show_window(&mut dom, 0, 5);

    for _ in 0..10 {
        view.navigate(&mut dom, Nav::Down);
    }
    assert_eq!(view.cursor().row(), 10);

    // The window stayed bounded and shifted to keep the cursor visible.
    assert_eq!(view.mounted_row_count(), 5, "window stays bounded");
    let scroll = view.cursor().scroll();
    assert_eq!(scroll, 6, "scroll follows cursor (10 + 1 - 5)");

    // Exactly one materialized row carries the highlight — the cursor's.
    let rows = tbody_rows(&dom, table);
    let active: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|&(_, &tr)| dom.has_attribute(tr, "data-active-row"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        active,
        vec![10 - scroll],
        "highlight on the cursor's pool row"
    );
}

#[test]
fn highlight_is_focus_gated_at_paint() {
    // Same setup as `focused_navigated_grid` but we toggle focus by hand.
    let view = grid(20, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);
    view.navigate(&mut dom, Nav::Down); // cursor at row 1

    let sheet = highlight_stylesheet();
    let viewport = Rect::new(0, 0, 40, 12);
    let cursor_bg = Color::Rgb(0x1f, 0x21, 0x23); // #1f2123 — the cursor cell

    // Unfocused: the focus-gated rule must not paint the cursor.
    assert_eq!(
        count_bg(&mut dom, &sheet, viewport, cursor_bg),
        0,
        "no cursor bg when unfocused"
    );

    // Focused: the active cell paints with the cursor background.
    dom.set_focused(Some(table));
    assert!(
        count_bg(&mut dom, &sheet, viewport, cursor_bg) > 0,
        "cursor bg appears once the table is focused"
    );
}

#[test]
fn highlight_colors_are_opt_in_defaults() {
    // The `data-active-*` attributes are the contract; the colors are not.
    // With no highlight CSS at all, the default cursor color is never painted —
    // styling is purely the consumer's CSS.
    let (mut dom, _table) = focused_navigated_grid();
    let viewport = Rect::new(0, 0, 40, 12);
    let default_cell = Color::Rgb(0x1f, 0x21, 0x23);

    assert_eq!(
        count_bg(&mut dom, &Stylesheet::new(), viewport, default_cell),
        0,
        "without highlight CSS, the default cursor color is never painted"
    );
}

#[test]
fn consumer_css_overrides_default_colors() {
    // The defaults are wrapped in `:where()` → zero specificity, so even a
    // PLAIN low-specificity author rule overrides them — no `table:focus`
    // prefix, no specificity matching. `td[data-active-cell]` is (0,1,1),
    // which would have LOST to the un-wrapped `table:focus td[...]` (0,2,2)
    // default but beats the `:where()`-wrapped one. This is the browser-easy
    // override the `:where()` wiring buys us.
    let (mut dom, _table) = focused_navigated_grid();
    let viewport = Rect::new(0, 0, 40, 12);

    let custom = Color::Rgb(0x80, 0x00, 0x00); // a color our defaults never use
    let sheet = highlight_stylesheet()
        .rule("td[data-active-cell]", TuiStyle::new().bg(custom))
        .unwrap();

    assert!(
        count_bg(&mut dom, &sheet, viewport, custom) > 0,
        "a plain low-specificity author rule paints its own cursor color"
    );
    assert_eq!(
        count_bg(&mut dom, &sheet, viewport, Color::Rgb(0x1f, 0x21, 0x23)),
        0,
        "the zero-specificity default cursor color is fully overridden"
    );
}
