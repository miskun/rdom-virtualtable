//! Persistable UI state (`SPEC_DATA_SOURCE.md` P4): `table_state()` snapshots
//! the column layout + sort, `on_state_change` fires on every layout mutation
//! so a consumer can persist it, and `restore_state` re-applies a saved one.

use std::cell::RefCell;
use std::rc::Rc;

use rdom_tui::{NodeId, TuiDom};
use rdom_virtualtable::{Column, SortDir, SortSpec, TableState, VirtualTable, VirtualTableView};

fn mounted_view() -> (VirtualTableView, TuiDom, NodeId) {
    let mut model = VirtualTable::new(vec![
        Column::new("id"),
        Column::new("name"),
        Column::new("status"),
    ]);
    model.set_rows(
        (0..5)
            .map(|i| vec![format!("{i}").into(), format!("n{i}").into(), "ok".into()])
            .collect(),
    );
    let view = VirtualTableView::new(model);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(5);
    view.show_window(&mut dom, 0, 5);
    (view, dom, table)
}

fn headers(s: &TableState) -> Vec<String> {
    s.columns.iter().map(|c| c.header.clone()).collect()
}

#[test]
fn table_state_snapshots_layout_and_sort() {
    let (view, mut dom, _t) = mounted_view();
    let s = view.table_state();
    assert_eq!(headers(&s), ["id", "name", "status"]);
    assert!(s.columns.iter().all(|c| c.width.is_none() && !c.hidden));
    assert_eq!(s.sort, None);

    view.sort(&mut dom, 1, SortDir::Descending);
    view.set_column_width(&mut dom, 0, Some(6));
    view.set_column_hidden(&mut dom, 2, true);
    let s = view.table_state();
    assert_eq!(
        s.sort,
        Some(SortSpec {
            column: "name".into(),
            dir: SortDir::Descending
        })
    );
    assert_eq!(s.columns[0].width, Some(6));
    assert!(s.columns[2].hidden);
}

#[test]
fn on_state_change_fires_for_each_layout_mutation() {
    let (view, mut dom, _t) = mounted_view();
    let log = Rc::new(RefCell::new(Vec::<TableState>::new()));
    let l = log.clone();
    view.on_state_change(move |s| l.borrow_mut().push(s.clone()));

    view.sort(&mut dom, 0, SortDir::Ascending);
    view.move_column(&mut dom, 0, 2);
    view.set_column_width(&mut dom, 0, Some(9));
    view.set_column_hidden(&mut dom, 1, true);
    view.clear_sort(&mut dom);
    assert_eq!(
        log.borrow().len(),
        5,
        "one notification per layout mutation"
    );
    assert_eq!(
        log.borrow().last().unwrap().sort,
        None,
        "last snapshot reflects the cleared sort"
    );
}

#[test]
fn restore_state_round_trips_order_widths_hidden_and_sort() {
    let (view, mut dom, _t) = mounted_view();
    view.move_column(&mut dom, 0, 2); // id → end: [name, status, id]
    view.set_column_width(&mut dom, 0, Some(7)); // name width
    view.set_column_hidden(&mut dom, 1, true); // hide status
    view.sort(&mut dom, 0, SortDir::Descending); // sort by name desc
    let saved = view.table_state();
    assert_eq!(headers(&saved), ["name", "status", "id"]);

    // Fresh view in default order; restoring reproduces the saved layout exactly.
    let (view2, mut dom2, _t2) = mounted_view();
    view2.restore_state(&mut dom2, &saved);
    assert_eq!(view2.table_state(), saved);
}

#[test]
fn restore_state_does_not_fire_the_save_callback() {
    let (view, mut dom, _t) = mounted_view();
    view.move_column(&mut dom, 0, 2);
    view.sort(&mut dom, 0, SortDir::Ascending);
    let saved = view.table_state();

    let (view2, mut dom2, _t2) = mounted_view();
    let count = Rc::new(RefCell::new(0usize));
    let c = count.clone();
    view2.on_state_change(move |_| *c.borrow_mut() += 1);
    view2.restore_state(&mut dom2, &saved);
    assert_eq!(
        *count.borrow(),
        0,
        "restoring a saved state must not emit save notifications"
    );
}
