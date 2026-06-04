//! Overflow chip + column show/hide dropdown overlay.
//!
//! Hiding a column is one-way at the cursor (it correctly skips hidden
//! columns), so the recovery path is a trailing "…" chip in the header that
//! opens a floating dropdown listing the hidden columns. Clicking an entry
//! brings that column back.

use rdom_tui::layout::{Length, Position};
use rdom_tui::render::{Buffer, LayoutExt, PaintExt, Rect};
use rdom_tui::style::CascadeExt;
use rdom_tui::{Color, NodeId, TuiDispatchExt, TuiDom, TuiEvent, TuiNodeExt, Value};
use rdom_virtualtable::{Column, VirtualTable, VirtualTableView, highlight_stylesheet};

const VISIBLE: usize = 5;

fn grid(cols: usize) -> VirtualTableView {
    let columns = (0..cols).map(|c| Column::new(format!("c{c}"))).collect();
    let mut model = VirtualTable::new(columns);
    model.set_rows(
        (0..VISIBLE)
            .map(|r| (0..cols).map(|c| format!("r{r}c{c}")).collect())
            .collect(),
    );
    VirtualTableView::new(model)
}

/// Mount + show the window; returns `(dom, table)`.
fn mounted(view: &VirtualTableView) -> (TuiDom, NodeId) {
    let mut dom = TuiDom::new();
    let table = view.mount(&mut dom);
    let root = dom.root();
    dom.append_child(root, table).unwrap();
    view.show_window(&mut dom, 0, VISIBLE);
    (dom, table)
}

/// The header `<tr>` of the table.
fn header_tr(dom: &TuiDom, table: NodeId) -> NodeId {
    for thead in dom.node(table).children() {
        if thead.node_name() == "thead" {
            for tr in thead.children() {
                if tr.node_name() == "tr" {
                    return tr.id();
                }
            }
        }
    }
    panic!("no header tr");
}

/// First descendant (depth-first) carrying the presence attribute `attr`.
fn find_attr(dom: &TuiDom, root: NodeId, attr: &str) -> Option<NodeId> {
    let node = dom.node(root);
    if node.get_attribute(attr).is_some() {
        return Some(root);
    }
    for child in node.child_nodes() {
        if let Some(found) = find_attr(dom, child.id(), attr) {
            return Some(found);
        }
    }
    None
}

/// All descendants carrying `attr`, depth-first.
fn find_all(dom: &TuiDom, root: NodeId, attr: &str, out: &mut Vec<NodeId>) {
    let node = dom.node(root);
    if node.get_attribute(attr).is_some() {
        out.push(root);
    }
    for child in node.child_nodes() {
        find_all(dom, child.id(), attr, out);
    }
}

#[test]
fn no_overflow_chip_when_nothing_is_hidden() {
    let view = grid(3);
    let (dom, table) = mounted(&view);
    assert!(
        find_attr(&dom, table, "data-vt-overflow").is_none(),
        "no chip with all columns visible"
    );
}

#[test]
fn hiding_a_column_adds_the_overflow_chip_to_the_header() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);

    view.set_column_hidden(&mut dom, 1, true);

    let chip = find_attr(&dom, table, "data-vt-overflow").expect("chip appears");
    // It's a header cell …
    assert_eq!(dom.node(chip).node_name(), "th");
    // … parented by the header row, as the trailing cell.
    let tr = header_tr(&dom, table);
    assert_eq!(dom.node(chip).parent_node().unwrap().id(), tr);
    assert_eq!(dom.node(chip).text_content(), "…");
}

#[test]
fn showing_every_column_again_removes_the_chip() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);

    view.set_column_hidden(&mut dom, 1, true);
    assert!(find_attr(&dom, table, "data-vt-overflow").is_some());
    view.set_column_hidden(&mut dom, 1, false);

    assert!(
        find_attr(&dom, table, "data-vt-overflow").is_none(),
        "chip goes away once nothing is hidden"
    );
}

#[test]
fn chip_is_not_a_model_column() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.set_column_hidden(&mut dom, 1, true);

    // The model still has exactly 3 columns …
    assert_eq!(view.with(|t| t.columns().len()), 3);
    // … and the header has 3 real `<th>` plus exactly one overflow chip.
    let tr = header_tr(&dom, table);
    let mut chips = Vec::new();
    find_all(&dom, tr, "data-vt-overflow", &mut chips);
    assert_eq!(chips.len(), 1, "exactly one chip");
    let ths = dom
        .node(tr)
        .children()
        .filter(|c| c.node_name() == "th")
        .count();
    assert_eq!(ths, 4, "3 model headers + 1 chip");
}

#[test]
fn opening_the_menu_lists_the_hidden_columns() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.set_column_hidden(&mut dom, 2, true);
    view.set_column_hidden(&mut dom, 0, true);

    assert!(!view.is_column_menu_open());
    view.toggle_column_menu(&mut dom);
    assert!(view.is_column_menu_open());

    let menu = find_attr(&dom, table, "data-vt-menu").expect("menu overlay");
    // Anchored under the chip → child of the chip, floating.
    let chip = find_attr(&dom, table, "data-vt-overflow").unwrap();
    assert_eq!(dom.node(menu).parent_node().unwrap().id(), chip);
    let s = dom.node(menu).inline_style().expect("menu is styled");
    assert_eq!(s.position, Some(Value::Specified(Position::Absolute)));
    assert_eq!(s.top, Some(Value::Specified(Length::Cells(1))));
    assert!(s.z_index.is_some(), "menu floats above the body");

    // One item per hidden column, sorted by index, labelled + tagged.
    let mut items = Vec::new();
    find_all(&dom, menu, "data-vt-menu-item", &mut items);
    assert_eq!(items.len(), 2);
    assert_eq!(dom.node(items[0]).text_content(), "c0");
    assert_eq!(dom.node(items[0]).get_attribute("data-vt-col"), Some("0"));
    assert_eq!(dom.node(items[1]).text_content(), "c2");
    assert_eq!(dom.node(items[1]).get_attribute("data-vt-col"), Some("2"));
}

#[test]
fn toggle_closes_an_open_menu() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.set_column_hidden(&mut dom, 1, true);

    view.toggle_column_menu(&mut dom);
    assert!(view.is_column_menu_open());
    view.toggle_column_menu(&mut dom);
    assert!(!view.is_column_menu_open());
    assert!(
        find_attr(&dom, table, "data-vt-menu").is_none(),
        "overlay removed on close"
    );
    // The chip survives a menu close (still a hidden column).
    assert!(find_attr(&dom, table, "data-vt-overflow").is_some());
}

#[test]
fn unhiding_from_the_menu_updates_it_and_drops_the_entry() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.set_column_hidden(&mut dom, 0, true);
    view.set_column_hidden(&mut dom, 2, true);
    view.toggle_column_menu(&mut dom);

    // Bring column 0 back (what an item click routes to).
    view.set_column_hidden(&mut dom, 0, false);

    assert!(!view.with(|t| t.is_column_hidden(0)));
    assert!(
        view.is_column_menu_open(),
        "menu stays open while >0 hidden"
    );
    let menu = find_attr(&dom, table, "data-vt-menu").unwrap();
    let mut items = Vec::new();
    find_all(&dom, menu, "data-vt-menu-item", &mut items);
    assert_eq!(items.len(), 1, "only the still-hidden column remains");
    assert_eq!(dom.node(items[0]).get_attribute("data-vt-col"), Some("2"));
}

#[test]
fn unhiding_the_last_column_closes_the_menu_and_removes_the_chip() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.set_column_hidden(&mut dom, 1, true);
    view.toggle_column_menu(&mut dom);
    assert!(view.is_column_menu_open());

    view.set_column_hidden(&mut dom, 1, false);

    assert!(
        !view.is_column_menu_open(),
        "menu closes with nothing hidden"
    );
    assert!(find_attr(&dom, table, "data-vt-menu").is_none());
    assert!(find_attr(&dom, table, "data-vt-overflow").is_none());
}

#[test]
fn the_dropdown_paints_over_the_body() {
    // The whole point of the overlay: an absolute, z-indexed panel that
    // composites *on top of* the body rows. Prove it at the paint layer.
    let view = grid(3);
    let (mut dom, _table) = mounted(&view);
    view.set_column_hidden(&mut dom, 1, true);
    view.toggle_column_menu(&mut dom);

    let vp = Rect::new(0, 0, 40, 12);
    let sheet = highlight_stylesheet();
    dom.cascade(&sheet);
    dom.layout_dom(vp);
    let mut buf = Buffer::empty(vp);
    dom.paint_dom(&mut buf, vp);

    // The menu background (private const, recomputed here).
    let menu_bg = Color::Rgb(0x22, 0x24, 0x26);
    let mut total = 0;
    let mut below_header = 0; // header is row 0; the body starts at row 1
    for y in vp.y..vp.bottom() {
        for x in vp.x..vp.right() {
            if let Some(c) = buf.cell(x, y) {
                if c.bg == menu_bg {
                    total += 1;
                    if y >= 1 {
                        below_header += 1;
                    }
                }
            }
        }
    }
    assert!(total > 0, "the dropdown background actually paints");
    assert!(
        below_header > 0,
        "and it overlays the body region (z-index lifts it above the rows)"
    );
}

/// Dispatch a bubbling `click` at `target` (the listener only reads
/// `event.target`, which `dispatch` populates — no coordinates needed).
fn click(dom: &mut TuiDom, target: NodeId) {
    let mut ev = TuiEvent::new("click");
    dom.dispatch_tui_event(target, &mut ev).unwrap();
}

#[test]
fn clicking_the_chip_opens_the_menu() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.set_column_hidden(&mut dom, 1, true);
    let chip = find_attr(&dom, table, "data-vt-overflow").unwrap();

    click(&mut dom, chip);

    assert!(view.is_column_menu_open(), "chip click opens the dropdown");
}

#[test]
fn clicking_a_menu_item_unhides_that_column() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.set_column_hidden(&mut dom, 1, true);
    view.toggle_column_menu(&mut dom);
    let menu = find_attr(&dom, table, "data-vt-menu").unwrap();
    let mut items = Vec::new();
    find_all(&dom, menu, "data-vt-menu-item", &mut items);

    click(&mut dom, items[0]); // the only hidden column, c1

    assert!(!view.with(|t| t.is_column_hidden(1)), "column came back");
    // Last hidden column shown → menu + chip torn down.
    assert!(!view.is_column_menu_open());
    assert!(find_attr(&dom, table, "data-vt-overflow").is_none());
}

#[test]
fn clicking_outside_dismisses_the_menu() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.set_column_hidden(&mut dom, 1, true);
    view.toggle_column_menu(&mut dom);
    assert!(view.is_column_menu_open());

    click(&mut dom, table); // anywhere that isn't the chip/menu

    assert!(!view.is_column_menu_open(), "outside click closes it");
    // …but only the menu — the chip stays (a column is still hidden).
    assert!(find_attr(&dom, table, "data-vt-overflow").is_some());
}

#[test]
fn close_column_menu_keeps_the_chip() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.set_column_hidden(&mut dom, 1, true);
    view.toggle_column_menu(&mut dom);

    view.close_column_menu(&mut dom);

    assert!(!view.is_column_menu_open());
    assert!(find_attr(&dom, table, "data-vt-menu").is_none());
    assert!(find_attr(&dom, table, "data-vt-overflow").is_some());
}
