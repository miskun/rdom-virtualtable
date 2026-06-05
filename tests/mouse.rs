//! Mouse interaction: header-click sort cycle + cell selection (click /
//! Shift+click / Ctrl+click / drag), wired by `install_mouse`.

use crossterm::event::{
    Event as CtEvent, KeyModifiers, MouseButton as CtButton, MouseEvent, MouseEventKind,
};
use rdom_tui::render::{Terminal, TestBackend};
use rdom_tui::{App, NodeId, TuiDispatchExt, TuiDom, TuiEvent};
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

// ── End-to-end: drag past the edge autoscrolls + keeps selecting ────

#[test]
fn drag_past_the_edge_autoscrolls_and_extends_the_selection() {
    // The headline behavior: a cell-range drag held past the bottom of a
    // scrollable virtual table scrolls the window in and keeps the rectangle
    // growing to rows that weren't materialized when the drag started.
    let mut model = VirtualTable::new((0..3).map(|c| Column::new(format!("c{c}"))).collect());
    model.set_rows(
        (0..40)
            .map(|r| (0..3).map(|c| format!("r{r}c{c}")).collect())
            .collect(),
    );
    let view = VirtualTableView::new(model);
    view.set_selection_mode(SelectionMode::Cell);

    let mut dom = TuiDom::new();
    let table = view.mount(&mut dom);
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    let root = dom.root();
    dom.append_child(root, table).unwrap();
    let visible = 6u16;
    view.show_window(&mut dom, 0, visible as usize);
    view.install_nav(&mut dom, table, visible);
    view.install_mouse(&mut dom);
    view.enable_scrollbar(&mut dom);
    dom.set_focused(Some(table));

    let term = Terminal::new(TestBackend::new(24, 8)).unwrap();
    let mut app = App::with_backend(dom, rdom_virtualtable::highlight_stylesheet(), term).unwrap();
    app.draw_if_dirty().unwrap();

    let me = |kind, x: u16, y: u16| {
        CtEvent::Mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::empty(),
        })
    };
    // Press a top cell (anchors the range + captures + arms autoscroll), then
    // hold a drag at the bottom edge of the viewport. `x = 2` lands in the
    // first column (col 0) of a 24-wide / 3-column table.
    let col = 0;
    app.handle_event(me(MouseEventKind::Down(CtButton::Left), 2, 1));
    assert_eq!(
        app.dom().pointer_capture(),
        Some(table),
        "mousedown captured the table"
    );
    assert!(app.dom().drag_autoscroll(), "mousedown armed autoscroll");
    app.handle_event(me(MouseEventKind::Drag(CtButton::Left), 2, 7));
    // Tick the autoscroll several periods while held at the edge.
    for _ in 0..8 {
        app.advance(50).unwrap();
    }

    let sel = view.selection();
    assert!(sel.is_selected(0, col), "anchor row still selected");
    assert!(
        (0..40).filter(|&r| sel.is_selected(r, col)).count() > visible as usize,
        "autoscroll extended the selection beyond the initial {visible}-row window"
    );
    assert!(
        sel.is_selected(visible as usize + 2, col),
        "a row that was off-screen at drag start is now selected"
    );

    // Release stops autoscroll; the selection is stable afterward.
    app.handle_event(me(MouseEventKind::Up(CtButton::Left), 2, 7));
    let count_after_release = (0..40)
        .filter(|&r| view.selection().is_selected(r, col))
        .count();
    app.advance(500).unwrap();
    assert_eq!(
        (0..40)
            .filter(|&r| view.selection().is_selected(r, col))
            .count(),
        count_after_release,
        "no further autoscroll after release"
    );
}

#[test]
fn drag_autoscroll_keeps_a_stable_column_and_an_unclipped_window() {
    // Regression for the live symptoms (rdom-tui ≤ 0.3.12): dragging a
    // cell-range past the edge flickered the selected column (the synthetic
    // autoscroll move resolved col 0 because the re-windowed cells laid out
    // unsized) and cropped the window top (under-counted scroll extent clamped
    // scroll_top below window_start). Fixed in rdom-tui 0.3.13 by cascading the
    // mid-tick relayout. Drag in the NAME column (col 1) and assert the column
    // stays col 1 (no col-0 bleed) and scroll_top tracks window_start each tick.
    use rdom_tui::TuiAccessors;
    let mut model = VirtualTable::new((0..3).map(|c| Column::new(format!("c{c}"))).collect());
    model.set_rows(
        (0..40)
            .map(|r| (0..3).map(|c| format!("r{r}c{c}")).collect())
            .collect(),
    );
    let view = VirtualTableView::new(model);
    view.set_selection_mode(SelectionMode::Cell);

    let mut dom = TuiDom::new();
    let table = view.mount(&mut dom);
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    let root = dom.root();
    dom.append_child(root, table).unwrap();
    let visible = 6u16;
    view.show_window(&mut dom, 0, visible as usize);
    view.install_nav(&mut dom, table, visible);
    view.install_mouse(&mut dom);
    view.enable_scrollbar(&mut dom);
    dom.set_focused(Some(table));

    let term = Terminal::new(TestBackend::new(24, 8)).unwrap();
    let mut app = App::with_backend(dom, rdom_virtualtable::highlight_stylesheet(), term).unwrap();
    app.draw_if_dirty().unwrap();

    let me = |kind, x: u16, y: u16| {
        CtEvent::Mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::empty(),
        })
    };
    // x = 10 lands in the NAME column (col 1) of a 24-wide / 3-column table.
    let col = 1;
    app.handle_event(me(MouseEventKind::Down(CtButton::Left), 10, 1));
    app.handle_event(me(MouseEventKind::Drag(CtButton::Left), 10, 7));
    for _ in 0..6 {
        app.advance(50).unwrap();
        // No crop: the materialized window sits flush with the scroll offset.
        let tbody = first_named(app.dom(), table, "tbody");
        let st = app.dom().node(tbody).scroll_top().unwrap_or(-1) as usize;
        assert_eq!(
            st,
            view.window_start(),
            "scroll_top must track window_start (no cropped top)"
        );
    }

    let sel = view.selection();
    // The cursor stayed in the NAME column — no flicker into col 0 / col 2.
    assert_eq!(view.cursor().col(), col, "cursor column is stable");
    let selected_in_col1 = (0..40).filter(|&r| sel.is_selected(r, col)).count();
    assert!(
        selected_in_col1 > visible as usize,
        "the NAME-column selection extended past the initial window"
    );
    // The rectangle is exactly col 1 — neither id (col 0) nor status (col 2)
    // bled in from a flickering head column.
    for r in 0..40 {
        if sel.is_selected(r, col) {
            assert!(
                !sel.is_selected(r, 0),
                "row {r}: col 0 must not be selected"
            );
            assert!(
                !sel.is_selected(r, 2),
                "row {r}: col 2 must not be selected"
            );
        }
    }
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
