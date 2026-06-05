//! Column-actions column: the opt-in `…` header chip + its column chooser.
//!
//! `enable_column_actions` mounts a persistent trailing header cell whose
//! dropdown is a checklist of every column (built like HTML — a `<label>`
//! wrapping a native `<input type="checkbox">`): check to show, uncheck to
//! hide. Hiding the last visible column is refused.

use rdom_tui::layout::{Length, Position};
use rdom_tui::render::{Buffer, LayoutExt, PaintExt, Rect, Terminal, TestBackend};
use rdom_tui::style::CascadeExt;
use rdom_tui::{App, Color, NodeId, Padding, TuiDispatchExt, TuiDom, TuiEvent, TuiNodeExt, Value};
use rdom_virtualtable::{Column, Nav, VirtualTable, VirtualTableView, highlight_stylesheet};

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

/// Mount + show the window + enable the column-actions chip; returns `(dom, table)`.
fn mounted(view: &VirtualTableView) -> (TuiDom, NodeId) {
    let mut dom = TuiDom::new();
    let table = view.mount(&mut dom);
    let root = dom.root();
    dom.append_child(root, table).unwrap();
    view.show_window(&mut dom, 0, VISIBLE);
    view.enable_column_actions(&mut dom);
    (dom, table)
}

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

/// Chooser rows (the `<label>`s), in column order.
fn rows(dom: &TuiDom, table: NodeId) -> Vec<NodeId> {
    let menu = find_attr(dom, table, "data-vt-menu").expect("menu open");
    let mut v = Vec::new();
    find_all(dom, menu, "data-vt-menu-item", &mut v);
    v
}

/// The `<input type=checkbox>` inside a chooser row.
fn checkbox(dom: &TuiDom, row: NodeId) -> NodeId {
    dom.node(row)
        .children()
        .find(|c| c.node_name() == "input")
        .expect("row has a checkbox")
        .id()
}

fn checked(dom: &TuiDom, row: NodeId) -> bool {
    dom.node(checkbox(dom, row))
        .get_attribute("checked")
        .is_some()
}

fn active_index(dom: &TuiDom, rows: &[NodeId]) -> Option<usize> {
    rows.iter()
        .position(|&r| dom.node(r).get_attribute("data-vt-menu-active").is_some())
}

// ── Chip lifecycle ──────────────────────────────────────────────────

#[test]
fn enable_column_actions_adds_a_persistent_chip() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    let chip = find_attr(&dom, table, "data-vt-overflow").expect("chip after enable");
    assert_eq!(dom.node(chip).node_name(), "th");
    assert_eq!(dom.node(chip).text_content(), "…");
    // Persistent: still there after hiding and after showing again.
    view.set_column_hidden(&mut dom, 1, true);
    assert!(find_attr(&dom, table, "data-vt-overflow").is_some());
    view.set_column_hidden(&mut dom, 1, false);
    assert!(find_attr(&dom, table, "data-vt-overflow").is_some());
}

#[test]
fn no_chip_until_actions_enabled() {
    let view = grid(3);
    let mut dom = TuiDom::new();
    let table = view.mount(&mut dom);
    let root = dom.root();
    dom.append_child(root, table).unwrap();
    view.show_window(&mut dom, 0, VISIBLE);
    // No enable_column_actions → no chip even after hiding.
    view.set_column_hidden(&mut dom, 1, true);
    assert!(find_attr(&dom, table, "data-vt-overflow").is_none());
}

#[test]
fn chip_is_not_a_model_column() {
    let view = grid(3);
    let (dom, table) = mounted(&view);
    assert_eq!(view.with(|t| t.columns().len()), 3);
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

// ── Chooser contents ────────────────────────────────────────────────

#[test]
fn chooser_lists_all_columns_with_checkbox_state() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.set_column_hidden(&mut dom, 1, true); // c1 hidden

    view.toggle_column_menu(&mut dom);
    assert!(view.is_column_menu_open());

    // The overlay is a child of the chip (self-contained), absolute + padded.
    let menu = find_attr(&dom, table, "data-vt-menu").unwrap();
    let chip = find_attr(&dom, table, "data-vt-overflow").unwrap();
    assert_eq!(dom.node(menu).parent_node().unwrap().id(), chip);
    let s = dom.node(menu).inline_style().unwrap();
    assert_eq!(s.position, Some(Value::Specified(Position::Absolute)));
    assert_eq!(s.top, Some(Value::Specified(Length::Cells(1))));
    assert_eq!(s.right, Some(Value::Specified(Length::Cells(0))));
    assert!(
        s.padding.is_none(),
        "no padding — the half-block border is the edge"
    );
    assert!(s.z_index.is_some());

    // One row per column (ALL of them), native checkbox, checked = visible.
    let rows = rows(&dom, table);
    assert_eq!(rows.len(), 3);
    assert_eq!(dom.node(rows[0]).node_name(), "label");
    assert_eq!(dom.node(rows[0]).text_content(), "c0");
    assert_eq!(
        dom.node(checkbox(&dom, rows[0])).get_attribute("type"),
        Some("checkbox")
    );
    assert!(checked(&dom, rows[0]), "c0 visible → checked");
    assert!(!checked(&dom, rows[1]), "c1 hidden → unchecked");
    assert!(checked(&dom, rows[2]), "c2 visible → checked");
    assert_eq!(dom.node(rows[1]).get_attribute("data-vt-col"), Some("1"));
}

// ── Rendering: one line per row + a visible highlight bar ───────────

#[test]
fn rows_render_on_one_line_with_a_visible_highlight_bar() {
    let view = grid(3);
    let (mut dom, _table) = mounted(&view);
    let vp = Rect::new(0, 0, 40, 12);
    let sheet = highlight_stylesheet();
    dom.cascade(&sheet);
    dom.layout_dom(vp);
    view.toggle_column_menu(&mut dom);
    view.menu_highlight_move(&mut dom, 1); // highlight the 2nd row
    dom.cascade(&sheet);
    dom.layout_dom(vp);
    let mut buf = Buffer::empty(vp);
    dom.paint_dom(&mut buf, vp);

    // Read each painted row's text + count of highlight-bg cells.
    let hl = Color::Rgb(0x2b, 0x55, 0x7e);
    let line = |y: u16| -> (String, usize) {
        let mut s = String::new();
        let mut n = 0;
        for x in 0..vp.width {
            if let Some(c) = buf.cell(x, y) {
                s.push_str(c.symbol());
                if c.bg == hl {
                    n += 1;
                }
            }
        }
        (s, n)
    };
    // Header at y0, the chooser drops at y1: y1 is the half-block top border,
    // so the column rows start at y2. Each row is the checkbox glyph (`[x]`) AND
    // the label on the SAME line (regression: the input used to wrap the label).
    let (r0, hl0) = line(2);
    assert!(
        r0.contains("[x]") && r0.contains("c0"),
        "row 0 on one line: {r0:?}"
    );
    let (r1, hl1) = line(3);
    assert!(
        r1.contains("[x]") && r1.contains("c1"),
        "row 1 on one line: {r1:?}"
    );
    // The highlighted row (cursor moved to row 1) paints a bg bar; the others don't.
    assert!(hl1 > 0, "highlighted row paints a bar");
    assert_eq!(hl0, 0, "non-highlighted row has no bar");
}

#[test]
fn open_chooser_suppresses_the_cursor_crosshair() {
    // With the chooser open, the table's cursor cross-hair + selection should
    // step aside so focus rests on the dropdown.
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    dom.set_focused(Some(table));
    view.set_viewport_rows(VISIBLE as u16);
    view.navigate(&mut dom, Nav::Down); // cursor active at row 1

    let vp = Rect::new(0, 0, 40, 12);
    let sheet = highlight_stylesheet();
    let tint = Color::Rgb(0x18, 0x1a, 0x1c); // active row/column tint
    let count_tint = |dom: &mut TuiDom| {
        dom.cascade(&sheet);
        dom.layout_dom(vp);
        let mut buf = Buffer::empty(vp);
        dom.paint_dom(&mut buf, vp);
        let mut n = 0;
        for y in 0..vp.height {
            for x in 0..vp.width {
                if buf.cell(x, y).map(|c| c.bg) == Some(tint) {
                    n += 1;
                }
            }
        }
        n
    };

    assert!(
        count_tint(&mut dom) > 0,
        "cross-hair paints while focused + closed"
    );
    view.toggle_column_menu(&mut dom);
    assert_eq!(
        count_tint(&mut dom),
        0,
        "cross-hair suppressed while the chooser is open"
    );
    view.toggle_column_menu(&mut dom); // close
    assert!(count_tint(&mut dom) > 0, "cross-hair returns after close");
}

// ── Keyboard navigation ─────────────────────────────────────────────

#[test]
fn opening_highlights_the_first_row() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.toggle_column_menu(&mut dom);
    assert_eq!(active_index(&dom, &rows(&dom, table)), Some(0));
}

#[test]
fn menu_highlight_moves_and_clamps_over_all_columns() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.toggle_column_menu(&mut dom);

    view.menu_highlight_move(&mut dom, 2);
    assert_eq!(active_index(&dom, &rows(&dom, table)), Some(2));
    view.menu_highlight_move(&mut dom, 5); // clamp bottom
    assert_eq!(active_index(&dom, &rows(&dom, table)), Some(2));
    view.menu_highlight_move(&mut dom, -9); // clamp top
    assert_eq!(active_index(&dom, &rows(&dom, table)), Some(0));
}

#[test]
fn menu_activate_toggles_the_highlighted_column() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.toggle_column_menu(&mut dom);
    view.menu_highlight_move(&mut dom, 1); // highlight c1

    view.menu_activate(&mut dom); // hide c1
    assert!(view.with(|t| t.is_column_hidden(1)));
    assert!(
        !checked(&dom, rows(&dom, table)[1]),
        "checkbox reflects hidden"
    );

    view.menu_activate(&mut dom); // show c1 again
    assert!(!view.with(|t| t.is_column_hidden(1)));
    assert!(checked(&dom, rows(&dom, table)[1]));
}

#[test]
fn cannot_hide_the_last_visible_column() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    view.set_column_hidden(&mut dom, 1, true);
    view.set_column_hidden(&mut dom, 2, true); // only c0 visible now

    // Try to hide c0 (the last one) via the keyboard activate path.
    view.toggle_column_menu(&mut dom);
    view.menu_highlight_move(&mut dom, -5); // highlight c0
    view.menu_activate(&mut dom);

    assert!(
        !view.with(|t| t.is_column_hidden(0)),
        "last visible column stays"
    );
    assert!(
        checked(&dom, rows(&dom, table)[0]),
        "its checkbox stays checked"
    );
}

// ── Chip highlight + overlay paint ──────────────────────────────────

#[test]
fn open_chip_is_highlighted() {
    let view = grid(3);
    let (mut dom, table) = mounted(&view);
    let chip = find_attr(&dom, table, "data-vt-overflow").unwrap();
    assert!(dom.node(chip).get_attribute("data-vt-menu-open").is_none());
    view.toggle_column_menu(&mut dom);
    assert!(dom.node(chip).get_attribute("data-vt-menu-open").is_some());
    view.toggle_column_menu(&mut dom);
    assert!(dom.node(chip).get_attribute("data-vt-menu-open").is_none());
}

#[test]
fn dropdown_paints_over_the_body() {
    let view = grid(3);
    let (mut dom, _table) = mounted(&view);
    let vp = Rect::new(0, 0, 40, 12);
    let sheet = highlight_stylesheet();
    dom.cascade(&sheet);
    dom.layout_dom(vp);
    view.toggle_column_menu(&mut dom);
    dom.cascade(&sheet);
    dom.layout_dom(vp);
    let mut buf = Buffer::empty(vp);
    dom.paint_dom(&mut buf, vp);

    let menu_bg = Color::Rgb(0x22, 0x24, 0x26);
    let mut below_header = 0;
    for y in 1..vp.height {
        for x in 0..vp.width {
            if buf.cell(x, y).map(|c| c.bg) == Some(menu_bg) {
                below_header += 1;
            }
        }
    }
    assert!(below_header > 0, "the dropdown paints over the body region");
}

// ── Mouse (App-driven: needs the toggle builtin for native checkboxes) ──

fn app_with_actions(view: &VirtualTableView) -> (App<TestBackend>, NodeId) {
    let mut dom = TuiDom::new();
    let table = view.mount(&mut dom);
    let root = dom.root();
    dom.append_child(root, table).unwrap();
    view.show_window(&mut dom, 0, VISIBLE);
    view.enable_column_actions(&mut dom);
    let term = Terminal::new(TestBackend::new(40, 12)).unwrap();
    let app = App::with_backend(dom, highlight_stylesheet(), term).unwrap();
    (app, table)
}

fn click(dom: &mut TuiDom, target: NodeId) {
    let mut ev = TuiEvent::new("click");
    dom.dispatch_tui_event(target, &mut ev).unwrap();
}

#[test]
fn clicking_the_chip_opens_the_menu() {
    let view = grid(3);
    let (mut app, table) = app_with_actions(&view);
    let chip = find_attr(app.dom(), table, "data-vt-overflow").unwrap();
    click(app.dom_mut(), chip);
    assert!(view.is_column_menu_open());
}

#[test]
fn clicking_a_checkbox_toggles_its_column() {
    let view = grid(3);
    let (mut app, table) = app_with_actions(&view);
    view.toggle_column_menu(app.dom_mut());
    let cb = checkbox(app.dom(), rows(app.dom(), table)[1]); // c1's checkbox (checked)

    click(app.dom_mut(), cb); // native toggle flips it + fires change

    assert!(
        view.with(|t| t.is_column_hidden(1)),
        "c1 hidden after uncheck"
    );
    assert!(
        view.is_column_menu_open(),
        "menu stays open (it's a chooser)"
    );
}

#[test]
fn clicking_outside_dismisses_the_menu() {
    let view = grid(3);
    let (mut app, table) = app_with_actions(&view);
    view.toggle_column_menu(app.dom_mut());
    assert!(view.is_column_menu_open());
    click(app.dom_mut(), table); // not the chip, not the menu
    assert!(!view.is_column_menu_open());
    assert!(
        find_attr(app.dom(), table, "data-vt-overflow").is_some(),
        "chip persists"
    );
}

// ── Regression: PAINT-RELATIVE-ABSPOS-DOUBLE (rdom-tui 0.3.7) ────────

#[test]
fn chip_glyph_paints_once_after_hide_menu_hide() {
    use rdom_tui::{Direction, Display, Flow, Size, TuiNodeMutExt, TuiStyle};
    fn flex_col() -> TuiStyle {
        let mut s = TuiStyle::new().direction(Direction::Column);
        s.display = Some(Value::Specified(Display::Block));
        s.flow = Some(Value::Specified(Flow::Flex));
        s
    }
    let view = grid(3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    dom.node_mut(root).set_inline_style(flex_col());
    let container = dom.create_element("div");
    dom.node_mut(container).set_inline_style(
        flex_col()
            .width(Size::Flex(1))
            .height(Size::Flex(1))
            .padding(Padding::all(1)),
    );
    dom.append_child(root, container).unwrap();
    let table = view.mount(&mut dom);
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    dom.append_child(container, table).unwrap();
    view.show_window(&mut dom, 0, VISIBLE);
    view.install_nav(&mut dom, table, 14);
    view.enable_scrollbar(&mut dom);
    view.enable_column_actions(&mut dom);
    dom.set_focused(Some(table));

    let term = Terminal::new(TestBackend::new(80, 20)).unwrap();
    let mut app = App::with_backend(dom, highlight_stylesheet(), term).unwrap();
    app.draw_if_dirty().unwrap();
    view.set_column_hidden(app.dom_mut(), 2, true);
    app.draw_if_dirty().unwrap();
    view.toggle_column_menu(app.dom_mut());
    app.draw_if_dirty().unwrap();
    view.toggle_column_menu(app.dom_mut());
    app.draw_if_dirty().unwrap();
    view.set_column_hidden(app.dom_mut(), 1, true);
    app.draw_if_dirty().unwrap();

    let vp = Rect::new(0, 0, 80, 20);
    let dom = app.dom_mut();
    dom.layout_dom(vp);
    let mut buf = Buffer::empty(vp);
    dom.paint_dom(&mut buf, vp);
    let glyphs = (0..vp.height)
        .flat_map(|y| (0..vp.width).map(move |x| (x, y)))
        .filter(|&(x, y)| buf.cell(x, y).map(|c| c.symbol()) == Some("…"))
        .count();
    assert_eq!(glyphs, 1, "exactly one '…' chip glyph, got {glyphs}");
}
