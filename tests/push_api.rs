//! Windowed push API (`SPEC_DATA_SOURCE.md` §5): a consumer feeds the table a
//! windowed buffer with `set_total` + `apply(epoch, Delta)`, the renderer paints
//! placeholders for not-yet-loaded slots, and stale-epoch pushes are dropped.

use rdom_tui::{NodeId, TuiDom};
use rdom_virtualtable::{CellValue, Column, Delta, Row, VirtualTable, VirtualTableView};

/// A windowed view over an empty model (no resident rows — the consumer pushes).
fn windowed_view(cols: usize) -> VirtualTableView {
    let columns = (0..cols).map(|c| Column::new(format!("c{c}"))).collect();
    VirtualTableView::new(VirtualTable::new(columns))
}

/// Mount + append + size the viewport. Returns the dom and `<table>` id.
fn mounted(view: &VirtualTableView, viewport: u16) -> (TuiDom, NodeId) {
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(viewport);
    (dom, table)
}

/// The `<tr>` node ids currently under `<tbody>`, in order (data rows only).
fn tbody_rows(dom: &TuiDom, table: NodeId) -> Vec<NodeId> {
    for child in dom.node(table).children() {
        if child.node_name() == "tbody" {
            return child
                .children()
                .filter(|c| c.node_name() == "tr" && !c.has_attribute("data-rdom-spacer"))
                .map(|c| c.id())
                .collect();
        }
    }
    Vec::new()
}

fn row_cells(dom: &TuiDom, tr: NodeId) -> Vec<NodeId> {
    dom.node(tr)
        .children()
        .filter(|c| c.node_name() == "td")
        .map(|c| c.id())
        .collect()
}

fn row(key: &str, cells: &[&str]) -> Row {
    Row::new(key, cells.iter().map(|s| CellValue::from(*s)).collect())
}

#[test]
fn resync_renders_loaded_rows_and_placeholders_for_the_rest() {
    let view = windowed_view(2);
    let (mut dom, table) = mounted(&view, 5);
    view.set_total(&mut dom, 100);
    let ep = view.window_epoch();
    // Only the first two of the five visible slots are loaded.
    view.apply(
        &mut dom,
        ep,
        Delta::Resync {
            start: 0,
            rows: vec![row("a", &["A0", "A1"]), row("b", &["B0", "B1"])],
        },
    );
    view.show_window(&mut dom, 0, 5);

    let rows = tbody_rows(&dom, table);
    assert_eq!(rows.len(), 5, "five visible slots materialized");
    // Rows 0,1 are real data.
    assert_eq!(dom.node(rows[0]).text_content(), "A0A1");
    assert_eq!(dom.node(rows[1]).text_content(), "B0B1");
    for &td in &row_cells(&dom, rows[0]) {
        assert!(
            !dom.has_attribute(td, "data-vt-loading"),
            "loaded cell is not a placeholder"
        );
    }
    // Rows 2,3,4 are placeholders: data-vt-loading, no text.
    for (r, &tr) in rows.iter().enumerate().skip(2) {
        let cells = row_cells(&dom, tr);
        assert_eq!(cells.len(), 2, "placeholder still has one cell per column");
        for &td in &cells {
            assert!(
                dom.has_attribute(td, "data-vt-loading"),
                "row {r} cell is a loading placeholder"
            );
        }
        assert_eq!(dom.node(tr).text_content(), "", "placeholder has no text");
    }
}

#[test]
fn upsert_patches_a_visible_row_in_place() {
    let view = windowed_view(2);
    let (mut dom, table) = mounted(&view, 3);
    view.set_total(&mut dom, 10);
    let ep = view.window_epoch();
    view.apply(
        &mut dom,
        ep,
        Delta::Resync {
            start: 0,
            rows: vec![row("a", &["A0", "A1"]), row("b", &["B0", "B1"])],
        },
    );
    // An in-place change to row "b".
    view.apply(
        &mut dom,
        ep,
        Delta::Upsert {
            rows: vec![row("b", &["B0", "CHANGED"])],
        },
    );
    view.show_window(&mut dom, 0, 3);

    let rows = tbody_rows(&dom, table);
    assert_eq!(dom.node(rows[1]).text_content(), "B0CHANGED");
    // An Upsert for a key not in the window is ignored (no panic, no new row).
    view.apply(
        &mut dom,
        ep,
        Delta::Upsert {
            rows: vec![row("z", &["Z0", "Z1"])],
        },
    );
    view.show_window(&mut dom, 0, 3);
    assert_eq!(tbody_rows(&dom, table).len(), 3);
}

#[test]
fn remove_turns_the_slot_into_a_placeholder() {
    let view = windowed_view(2);
    let (mut dom, table) = mounted(&view, 3);
    view.set_total(&mut dom, 10);
    let ep = view.window_epoch();
    view.apply(
        &mut dom,
        ep,
        Delta::Resync {
            start: 0,
            rows: vec![
                row("a", &["A0", "A1"]),
                row("b", &["B0", "B1"]),
                row("c", &["C0", "C1"]),
            ],
        },
    );
    view.apply(
        &mut dom,
        ep,
        Delta::Remove {
            keys: vec!["b".into()],
        },
    );
    view.show_window(&mut dom, 0, 3);

    let rows = tbody_rows(&dom, table);
    assert_eq!(dom.node(rows[0]).text_content(), "A0A1", "neighbour intact");
    for &td in &row_cells(&dom, rows[1]) {
        assert!(
            dom.has_attribute(td, "data-vt-loading"),
            "removed slot is now a placeholder"
        );
    }
    assert_eq!(dom.node(rows[2]).text_content(), "C0C1", "neighbour intact");
}

#[test]
fn stale_epoch_pushes_are_dropped() {
    let view = windowed_view(2);
    let (mut dom, table) = mounted(&view, 2);
    view.set_total(&mut dom, 10);
    let ep = view.window_epoch();
    view.apply(
        &mut dom,
        ep,
        Delta::Resync {
            start: 0,
            rows: vec![row("a", &["A0", "A1"]), row("b", &["B0", "B1"])],
        },
    );
    // A push echoing a *different* (stale) epoch must not touch the buffer.
    let stale = ep.wrapping_add(7);
    view.apply(
        &mut dom,
        stale,
        Delta::Resync {
            start: 0,
            rows: vec![row("x", &["X0", "X1"]), row("y", &["Y0", "Y1"])],
        },
    );
    view.show_window(&mut dom, 0, 2);

    let rows = tbody_rows(&dom, table);
    assert_eq!(
        dom.node(rows[0]).text_content(),
        "A0A1",
        "stale Resync ignored"
    );
    assert_eq!(dom.node(rows[1]).text_content(), "B0B1");
}
