//! Native vertical scrollbar (`enable_scrollbar`): the `<tbody>` is a
//! `overflow-y: auto` scroll container whose scroll extent reflects the TOTAL
//! row count (via spacer rows), wheel/drag re-windows decoupled from the
//! cursor, and cursor navigation scrolls the view to keep the cursor visible.

use rdom_tui::render::{LayoutExt, Rect};
use rdom_tui::style::{CascadeExt, Stylesheet};
use rdom_tui::{NodeId, TuiAccessors, TuiAccessorsMut, TuiDom};
use rdom_virtualtable::{Column, Nav, VirtualTable, VirtualTableView};

const VP: u16 = 10;

/// A scrollbar-enabled view over `rows × cols`, mounted + laid out once so the
/// scroll extent is computed. Returns `(dom, view, table, tbody)`.
fn scroll_grid(rows: usize, cols: usize) -> (TuiDom, VirtualTableView, NodeId, NodeId) {
    let columns = (0..cols).map(|c| Column::new(format!("c{c}"))).collect();
    let mut model = VirtualTable::new(columns);
    model.set_rows(
        (0..rows)
            .map(|r| (0..cols).map(|c| format!("r{r}c{c}")).collect())
            .collect(),
    );
    let view = VirtualTableView::new(model);

    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(VP);
    view.enable_scrollbar(&mut dom); // shows the window + spacers, attaches the listener

    let tbody = find_tbody(&dom, table);
    layout(&mut dom);
    (dom, view, table, tbody)
}

fn find_tbody(dom: &TuiDom, table: NodeId) -> NodeId {
    dom.node(table)
        .children()
        .find(|c| c.node_name() == "tbody")
        .map(|c| c.id())
        .expect("a <tbody>")
}

fn layout(dom: &mut TuiDom) {
    dom.cascade(&Stylesheet::new());
    dom.layout_dom(Rect::new(0, 0, 40, VP + 4));
}

/// The data (non-spacer) `<tr>`s under `<tbody>`, in order.
fn data_rows(dom: &TuiDom, tbody: NodeId) -> Vec<NodeId> {
    dom.node(tbody)
        .children()
        .filter(|c| c.node_name() == "tr" && !c.has_attribute("data-rdom-spacer"))
        .map(|c| c.id())
        .collect()
}

/// First column text of the first materialized data row.
fn first_cell(dom: &TuiDom, tbody: NodeId) -> String {
    let tr = data_rows(dom, tbody)[0];
    let td = dom
        .node(tr)
        .children()
        .find(|c| c.node_name() == "td")
        .unwrap();
    td.text_content()
}

#[test]
fn scroll_extent_reflects_total_rows_not_the_window() {
    let (dom, _view, _table, tbody) = scroll_grid(500, 2);
    // Only VP rows are materialized, but the scroll extent is the full 500.
    assert_eq!(
        dom.node(tbody).scroll_height(),
        Some(500),
        "the scroll thumb reflects all 500 rows"
    );
}

#[test]
fn scrolling_rewindows_without_moving_the_cursor() {
    let (mut dom, view, _table, tbody) = scroll_grid(500, 2);
    assert_eq!(first_cell(&dom, tbody), "r0c0");

    // Wheel/drag to row 100 → the `scroll` listener re-windows there.
    dom.node_mut(tbody).set_scroll_top(100).ok();
    assert_eq!(
        first_cell(&dom, tbody),
        "r100c0",
        "the window re-materialized at the scrolled offset"
    );
    assert_eq!(
        view.cursor().row(),
        0,
        "scrolling did NOT move the cursor (decoupled)"
    );
}

#[test]
fn cursor_navigation_scrolls_the_view_into_range() {
    let (mut dom, view, _table, tbody) = scroll_grid(500, 2);
    // Drive the cursor 20 rows down — past the VP-row window, so the view must
    // scroll to keep it visible.
    for _ in 0..20 {
        view.navigate(&mut dom, Nav::Down);
    }
    assert_eq!(view.cursor().row(), 20);
    let top = dom.node(tbody).scroll_top().unwrap_or(0);
    // follow(): scroll = row + 1 - viewport = 20 + 1 - 10 = 11.
    assert_eq!(
        top, 11,
        "the view scrolled so the cursor row is the last visible"
    );
    assert_eq!(
        first_cell(&dom, tbody),
        "r11c0",
        "window follows the cursor"
    );
}

#[test]
fn spacers_are_marked_and_excluded_from_the_data_window() {
    let (dom, view, _table, tbody) = scroll_grid(500, 2);
    let spacers = dom
        .node(tbody)
        .children()
        .filter(|c| c.has_attribute("data-rdom-spacer"))
        .count();
    assert!(
        spacers >= 1,
        "the window is bracketed by at least one spacer row"
    );
    assert_eq!(
        view.mounted_row_count(),
        VP as usize,
        "only the VP-row window is tracked as data (spacers excluded)"
    );
}
