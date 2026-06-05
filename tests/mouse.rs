//! Mouse interaction: header-click sort cycle + cell selection (click /
//! Shift+click / Ctrl+click / drag), wired by `install_mouse`.

use crossterm::event::{KeyModifiers, MouseButton as CtButton, MouseEvent, MouseEventKind};
use rdom_tui::{NodeId, TuiDispatchExt, TuiDom, TuiEvent};
use rdom_virtualtable::{Column, SelectionMode, SortDir, VirtualTable, VirtualTableView};

const ROWS: usize = 5;

fn grid(cols: usize) -> VirtualTableView {
    let columns = (0..cols).map(|c| Column::new(format!("c{c}"))).collect();
    let mut model = VirtualTable::new(columns);
    model.set_rows(
        (0..ROWS)
            .map(|r| (0..cols).map(|c| format!("r{r}c{c}")).collect())
            .collect(),
    );
    VirtualTableView::new(model)
}

/// Mount + show all rows + wire mouse. Returns `(dom, table)`.
fn mounted(view: &VirtualTableView) -> (TuiDom, NodeId) {
    let mut dom = TuiDom::new();
    let table = view.mount(&mut dom);
    let root = dom.root();
    dom.append_child(root, table).unwrap();
    view.show_window(&mut dom, 0, ROWS);
    view.install_mouse(&mut dom);
    (dom, table)
}

fn children_named(dom: &TuiDom, parent: NodeId, name: &str) -> Vec<NodeId> {
    dom.node(parent)
        .children()
        .filter(|c| c.node_name() == name)
        .map(|c| c.id())
        .collect()
}

fn first_named(dom: &TuiDom, parent: NodeId, name: &str) -> NodeId {
    children_named(dom, parent, name)[0]
}

/// The header `<th>` for model column `c`.
fn header(dom: &TuiDom, table: NodeId, c: usize) -> NodeId {
    let thead = first_named(dom, table, "thead");
    let tr = first_named(dom, thead, "tr");
    children_named(dom, tr, "th")[c]
}

/// The body `<td>` for window row `r`, column `c`.
fn cell(dom: &TuiDom, table: NodeId, r: usize, c: usize) -> NodeId {
    let tbody = first_named(dom, table, "tbody");
    let tr = children_named(dom, tbody, "tr")[r];
    children_named(dom, tr, "td")[c]
}

fn me(kind: MouseEventKind, mods: KeyModifiers) -> MouseEvent {
    MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: mods,
    }
}

fn dispatch(dom: &mut TuiDom, target: NodeId, mut ev: TuiEvent) {
    dom.dispatch_tui_event(target, &mut ev).unwrap();
}

fn down(dom: &mut TuiDom, target: NodeId, mods: KeyModifiers) {
    dispatch(
        dom,
        target,
        TuiEvent::mousedown(me(MouseEventKind::Down(CtButton::Left), mods)),
    );
}

fn click(dom: &mut TuiDom, target: NodeId) {
    dispatch(
        dom,
        target,
        TuiEvent::click(me(
            MouseEventKind::Down(CtButton::Left),
            KeyModifiers::empty(),
        )),
    );
}

// ── Header-click sort cycle ─────────────────────────────────────────

#[test]
fn header_click_cycles_sort_asc_desc_off() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    let th1 = header(&dom, table, 1);

    assert_eq!(view.sort_state(), None);
    click(&mut dom, th1);
    assert_eq!(view.sort_state(), Some((1, SortDir::Ascending)));
    click(&mut dom, th1);
    assert_eq!(view.sort_state(), Some((1, SortDir::Descending)));
    click(&mut dom, th1);
    assert_eq!(view.sort_state(), None, "third click turns sort off");
}

#[test]
fn header_click_on_a_new_column_starts_ascending() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    let th0 = header(&dom, table, 0);
    let th2 = header(&dom, table, 2);
    click(&mut dom, th0);
    assert_eq!(view.sort_state(), Some((0, SortDir::Ascending)));
    // Clicking a *different* header sorts that one ascending (not continuing the cycle).
    click(&mut dom, th2);
    assert_eq!(view.sort_state(), Some((2, SortDir::Ascending)));
}

// ── Cell click → move cursor ────────────────────────────────────────

#[test]
fn cell_click_moves_the_cursor() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    let c = cell(&dom, table, 3, 2);
    down(&mut dom, c, KeyModifiers::empty());
    assert_eq!((view.cursor().row(), view.cursor().col()), (3, 2));
}

// ── Shift+click → extend range ──────────────────────────────────────

#[test]
fn shift_click_extends_a_range() {
    let view = grid(3);
    view.set_selection_mode(SelectionMode::Cell);
    let (mut dom, table) = mounted(&view);
    // Click anchors at (1,0); Shift+click extends to (3,2).
    let a = cell(&dom, table, 1, 0);
    let b = cell(&dom, table, 3, 2);
    down(&mut dom, a, KeyModifiers::empty());
    down(&mut dom, b, KeyModifiers::SHIFT);

    let sel = view.selection();
    assert!(sel.is_selected(1, 0), "anchor corner");
    assert!(sel.is_selected(3, 2), "head corner");
    assert!(sel.is_selected(2, 1), "interior of the rectangle");
    assert!(!sel.is_selected(0, 0), "outside the rectangle");
    assert_eq!((view.cursor().row(), view.cursor().col()), (3, 2));
}

// ── Ctrl/⌘+click → toggle discontiguous ─────────────────────────────

#[test]
fn ctrl_click_toggles_individual_cells() {
    let view = grid(3);
    view.set_selection_mode(SelectionMode::Cell);
    let (mut dom, table) = mounted(&view);
    let a = cell(&dom, table, 0, 0);
    let b = cell(&dom, table, 4, 2);
    down(&mut dom, a, KeyModifiers::CONTROL);
    down(&mut dom, b, KeyModifiers::CONTROL);
    let sel = view.selection();
    assert!(sel.is_selected(0, 0));
    assert!(sel.is_selected(4, 2));
    assert!(!sel.is_selected(2, 1), "discontiguous — nothing between");
    // Ctrl+click an already-selected cell removes it.
    down(&mut dom, a, KeyModifiers::CONTROL);
    assert!(!view.selection().is_selected(0, 0));
}

// ── Drag → rubber-band range ────────────────────────────────────────

#[test]
fn drag_rubber_bands_a_range() {
    let view = grid(3);
    view.set_selection_mode(SelectionMode::Cell);
    let (mut dom, table) = mounted(&view);

    let a = cell(&dom, table, 0, 0);
    let mid = cell(&dom, table, 2, 1);
    down(&mut dom, a, KeyModifiers::empty());
    dispatch(
        &mut dom,
        mid,
        TuiEvent::mousemove(me(
            MouseEventKind::Drag(CtButton::Left),
            KeyModifiers::empty(),
        )),
    );
    dispatch(
        &mut dom,
        mid,
        TuiEvent::mouseup(me(
            MouseEventKind::Up(CtButton::Left),
            KeyModifiers::empty(),
        )),
    );

    let sel = view.selection();
    assert!(sel.is_selected(0, 0), "drag anchor");
    assert!(sel.is_selected(2, 1), "drag head");
    assert!(sel.is_selected(1, 0), "interior");
    assert!(!sel.is_selected(3, 0), "below the drag");
    assert_eq!((view.cursor().row(), view.cursor().col()), (2, 1));
}

#[test]
fn drag_extends_only_while_the_button_is_held() {
    // A mousemove with no button held (buttons bit 0 clear) must not extend.
    let view = grid(3);
    view.set_selection_mode(SelectionMode::Cell);
    let (mut dom, table) = mounted(&view);
    let a = cell(&dom, table, 0, 0);
    let mid = cell(&dom, table, 2, 1);
    let far = cell(&dom, table, 4, 2);
    down(&mut dom, a, KeyModifiers::empty());
    dispatch(
        &mut dom,
        mid,
        TuiEvent::mouseup(me(
            MouseEventKind::Up(CtButton::Left),
            KeyModifiers::empty(),
        )),
    );
    // After the up, a plain move must not extend a range.
    dispatch(
        &mut dom,
        far,
        TuiEvent::mousemove(me(MouseEventKind::Moved, KeyModifiers::empty())),
    );
    assert!(
        !view.selection().is_selected(4, 2),
        "a move after release doesn't extend"
    );
}
