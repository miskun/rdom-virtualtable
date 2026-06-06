//! `on_window_change` / `invalidate` / prefetch (`SPEC_DATA_SOURCE.md` §5):
//! the table asks the consumer to (re)fetch a window when the visible range,
//! sort, or an `invalidate` changes what must be shown — with a fresh epoch and
//! a prefetch-expanded range — and coalesces redundant requests.

use std::cell::RefCell;
use std::rc::Rc;

use rdom_tui::{NodeId, TuiDom};
use rdom_virtualtable::{
    CellValue, Column, Delta, Row, SortDir, SortSpec, VirtualTable, VirtualTableView, WindowRequest,
};

fn windowed_view(cols: usize) -> VirtualTableView {
    let columns = (0..cols).map(|c| Column::new(format!("c{c}"))).collect();
    VirtualTableView::new(VirtualTable::new(columns))
}

fn mounted(view: &VirtualTableView, viewport: u16) -> (TuiDom, NodeId) {
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(viewport);
    (dom, table)
}

fn row(key: &str, cells: &[&str]) -> Row {
    Row::new(key, cells.iter().map(|s| CellValue::from(*s)).collect())
}

/// A recorder for the requests the table fires.
type Log = Rc<RefCell<Vec<WindowRequest>>>;

fn record(view: &VirtualTableView) -> Log {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    view.on_window_change(move |req| l.borrow_mut().push(req));
    log
}

#[test]
fn setting_total_requests_the_initial_window_with_prefetch_margin() {
    let view = windowed_view(2);
    let (mut dom, _t) = mounted(&view, 4);
    let log = record(&view);
    view.set_total(&mut dom, 100); // fires the initial request

    let reqs = log.borrow();
    assert_eq!(reqs.len(), 1, "one request for the initial window");
    // visible 0..4, ±50% margin (count/2 = 2) → 0..6 (clamped at the bottom).
    assert_eq!(reqs[0].range, 0..6);
    assert_eq!(reqs[0].epoch, view.window_epoch(), "epoch matches current");
    assert!(reqs[0].sort.is_empty(), "unsorted");
}

#[test]
fn applying_the_requested_window_stops_re_requesting() {
    let view = windowed_view(2);
    let (mut dom, _t) = mounted(&view, 4);
    let log = record(&view);
    view.set_total(&mut dom, 100);
    let ep = view.window_epoch();
    // Fulfill the request: a Resync covering the visible window.
    let rows: Vec<Row> = (0..6).map(|i| row(&i.to_string(), &["x", "y"])).collect();
    view.apply(&mut dom, ep, Delta::Resync { start: 0, rows });
    // The post-apply re-render must not fire another request (window covered).
    view.show_window(&mut dom, 0, 4);
    assert_eq!(
        log.borrow().len(),
        1,
        "no re-request once the window is loaded"
    );
}

#[test]
fn scrolling_to_an_unloaded_window_fires_a_fresh_request() {
    let view = windowed_view(2);
    let (mut dom, _t) = mounted(&view, 4);
    let log = record(&view);
    view.set_total(&mut dom, 100);
    let ep = view.window_epoch();
    let rows: Vec<Row> = (0..6).map(|i| row(&i.to_string(), &["x", "y"])).collect();
    view.apply(&mut dom, ep, Delta::Resync { start: 0, rows });

    // Scroll far past the loaded window.
    view.show_window(&mut dom, 50, 4);
    let reqs = log.borrow();
    assert_eq!(reqs.len(), 2, "the unloaded window triggers a request");
    assert_eq!(reqs[1].range, 48..56, "50..54 expanded by the ±2 margin");
    assert!(reqs[1].epoch > reqs[0].epoch, "fresh epoch");
}

#[test]
fn invalidate_drops_stale_rows_and_forces_a_request() {
    let view = windowed_view(2);
    let (mut dom, table) = mounted(&view, 3);
    let log = record(&view);
    view.set_total(&mut dom, 100);
    let ep = view.window_epoch();
    let rows: Vec<Row> = (0..5).map(|i| row(&i.to_string(), &["x", "y"])).collect();
    view.apply(&mut dom, ep, Delta::Resync { start: 0, rows });
    let before = log.borrow().len();

    view.invalidate(&mut dom);
    assert_eq!(
        log.borrow().len(),
        before + 1,
        "invalidate forces a request"
    );
    assert!(
        log.borrow().last().unwrap().epoch > ep,
        "invalidate bumps the epoch"
    );
    // Stale rows were dropped — the window shows placeholders until the refresh.
    let rows_now = tbody_rows(&dom, table);
    assert!(
        dom.has_attribute(row_cells(&dom, rows_now[0])[0], "data-vt-loading"),
        "row 0 is a placeholder after invalidate"
    );
}

#[test]
fn sorting_requests_with_the_new_sort_spec() {
    let view = windowed_view(2);
    let (mut dom, _t) = mounted(&view, 4);
    let log = record(&view);
    view.set_total(&mut dom, 100);
    let ep = view.window_epoch();
    let rows: Vec<Row> = (0..6).map(|i| row(&i.to_string(), &["x", "y"])).collect();
    view.apply(&mut dom, ep, Delta::Resync { start: 0, rows });

    view.toggle_sort(&mut dom, 0); // sort by column "c0" ascending
    let reqs = log.borrow();
    let last = reqs.last().unwrap();
    assert_eq!(
        last.sort,
        vec![SortSpec {
            column: "c0".into(),
            dir: SortDir::Ascending
        }],
        "the request carries the new sort, keyed by column header"
    );
    assert!(last.epoch > ep, "sort change bumps the epoch");
}

// ── helpers ─────────────────────────────────────────────────────────

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
