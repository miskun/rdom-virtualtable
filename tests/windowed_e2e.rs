//! End-to-end windowed loop with a **synthetic source** (`SPEC_DATA_SOURCE.md`
//! P3): register `on_window_change`, and bridge each request back through
//! `apply` — here synchronously (a real consumer fetches async and pushes via
//! the App's inject queue). Proves scroll and sort drive the window correctly
//! over a 100k-row dataset while only the visible slice is ever materialized.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use rdom_tui::{NodeId, TuiDom};
use rdom_virtualtable::{
    CellValue, Column, Delta, Row, SortDir, SortSpec, VirtualTable, VirtualTableView, WindowRequest,
};

const TOTAL: usize = 100_000;

/// The synthetic data source: row at sorted position `pos` is `[id, item-id]`.
/// Sort-aware — descending reverses the id order — so a `SortSpec` round-trips.
fn synth(range: Range<usize>, sort: &[SortSpec]) -> Vec<Row> {
    let desc = matches!(sort.first().map(|s| s.dir), Some(SortDir::Descending));
    range
        .map(|pos| {
            let i = if desc { TOTAL - 1 - pos } else { pos };
            Row::new(
                format!("k{i}"),
                vec![
                    CellValue::from(i.to_string()),
                    CellValue::from(format!("item-{i}")),
                ],
            )
        })
        .collect()
}

/// The consumer bridge: drain the requests the table enqueued and fulfill each
/// with a `Resync`, echoing its epoch. (A real consumer runs the fetch on a
/// background runtime and `apply`s via `AppHandle::inject`.)
fn pump(view: &VirtualTableView, dom: &mut TuiDom, pending: &Rc<RefCell<Vec<WindowRequest>>>) {
    let reqs: Vec<WindowRequest> = pending.borrow_mut().drain(..).collect();
    for req in reqs {
        let start = req.range.start;
        let rows = synth(req.range.clone(), &req.sort);
        view.apply(dom, req.epoch, Delta::Resync { start, rows });
    }
}

/// First-column text of each materialized data row (spacers excluded).
fn first_col(dom: &TuiDom, table: NodeId) -> Vec<String> {
    let mut out = Vec::new();
    for child in dom.node(table).children() {
        if child.node_name() == "tbody" {
            for tr in child
                .children()
                .filter(|c| c.node_name() == "tr" && !c.has_attribute("data-rdom-spacer"))
            {
                if let Some(td) = tr.children().find(|c| c.node_name() == "td") {
                    out.push(td.text_content());
                }
            }
        }
    }
    out
}

fn windowed_table() -> (VirtualTableView, Rc<RefCell<Vec<WindowRequest>>>) {
    let view = VirtualTableView::new(VirtualTable::new(vec![
        Column::new("id"),
        Column::new("name"),
    ]));
    let pending = Rc::new(RefCell::new(Vec::new()));
    let p = pending.clone();
    view.on_window_change(move |req| p.borrow_mut().push(req));
    (view, pending)
}

#[test]
fn scroll_and_sort_drive_the_window_over_100k_rows() {
    let (view, pending) = windowed_table();
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(5);

    // Initial load: set_total fires the first request; the bridge fulfills it.
    view.set_total(&mut dom, TOTAL);
    pump(&view, &mut dom, &pending);
    assert_eq!(first_col(&dom, table), ["0", "1", "2", "3", "4"]);

    // Scroll deep into the dataset — only the new window materializes.
    view.show_window(&mut dom, 50_000, 5);
    pump(&view, &mut dom, &pending);
    assert_eq!(
        first_col(&dom, table),
        ["50000", "50001", "50002", "50003", "50004"]
    );

    // Back to the top, then sort descending: the window now shows the largest
    // ids — the SortSpec round-tripped through the source.
    view.show_window(&mut dom, 0, 5);
    pump(&view, &mut dom, &pending);
    view.toggle_sort(&mut dom, 0); // ascending
    pump(&view, &mut dom, &pending);
    assert_eq!(first_col(&dom, table), ["0", "1", "2", "3", "4"]);
    view.toggle_sort(&mut dom, 0); // descending
    pump(&view, &mut dom, &pending);
    assert_eq!(
        first_col(&dom, table),
        ["99999", "99998", "99997", "99996", "99995"]
    );
}

#[test]
fn never_materializes_more_than_the_window_plus_prefetch() {
    let (view, pending) = windowed_table();
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(10);
    view.set_total(&mut dom, TOTAL);
    pump(&view, &mut dom, &pending);

    // 100k logical rows, but the materialized `<tr>` count is the viewport.
    assert_eq!(first_col(&dom, table).len(), 10);
}

#[test]
fn live_upsert_patches_a_visible_row_without_a_refetch() {
    let (view, pending) = windowed_table();
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(5);
    view.set_total(&mut dom, TOTAL);
    pump(&view, &mut dom, &pending);

    // A live update to the row with key "k2" (the source pushes it directly).
    let ep = view.window_epoch();
    view.apply(
        &mut dom,
        ep,
        Delta::Upsert {
            rows: vec![Row::new(
                "k2",
                vec![CellValue::from("2"), CellValue::from("RENAMED")],
            )],
        },
    );
    // No new request was needed — the patch is in-place.
    assert!(pending.borrow().is_empty(), "upsert needs no refetch");
    // Second column of row index 2 now reads the new value.
    let names = nth_col(&dom, table, 1);
    assert_eq!(names[2], "RENAMED");
}

fn nth_col(dom: &TuiDom, table: NodeId, c: usize) -> Vec<String> {
    let mut out = Vec::new();
    for child in dom.node(table).children() {
        if child.node_name() == "tbody" {
            for tr in child
                .children()
                .filter(|c| c.node_name() == "tr" && !c.has_attribute("data-rdom-spacer"))
            {
                if let Some(td) = tr.children().filter(|c| c.node_name() == "td").nth(c) {
                    out.push(td.text_content());
                }
            }
        }
    }
    out
}
