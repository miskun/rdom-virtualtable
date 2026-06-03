//! Sort contract at the DOM/paint level: sorting reorders the materialized
//! window, marks the sorted header with `data-sort="asc|desc"` and a ▲/▼ glyph
//! in the header text, and clears the selection (it's row-index-keyed, so it's
//! meaningless after a reorder).

use rdom_tui::render::{Buffer, LayoutExt, PaintExt, Rect};
use rdom_tui::style::{CascadeExt, Stylesheet};
use rdom_tui::{NodeId, TuiDom};
use rdom_virtualtable::{
    Column, Nav, SelectionMode, SortDir, VirtualTable, VirtualTableView, highlight_stylesheet,
};

fn view_with(rows: &[&[&str]], cols: usize) -> VirtualTableView {
    let columns = (0..cols).map(|c| Column::new(format!("c{c}"))).collect();
    let mut model = VirtualTable::new(columns);
    model.set_rows(
        rows.iter()
            .map(|r| r.iter().map(|s| s.to_string()).collect())
            .collect(),
    );
    VirtualTableView::new(model)
}

fn mounted(view: &VirtualTableView, dom: &mut TuiDom, visible: usize) -> NodeId {
    let root = dom.root();
    let table = view.mount(dom);
    dom.append_child(root, table).unwrap();
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    view.set_viewport_rows(visible as u16);
    view.show_window(dom, 0, visible);
    table
}

/// The first `<td>` text of each `<tr>` under `<tbody>`, in DOM order.
fn col0_text(dom: &TuiDom, table: NodeId) -> Vec<String> {
    let mut out = Vec::new();
    for child in dom.node(table).children() {
        if child.node_name() == "tbody" {
            for tr in child.children().filter(|c| c.node_name() == "tr") {
                if let Some(td) = tr.children().find(|c| c.node_name() == "td") {
                    out.push(td.text_content());
                }
            }
        }
    }
    out
}

/// The `c`-th `<td>` text of each `<tr>` under `<tbody>`, in DOM order.
fn col_text(dom: &TuiDom, table: NodeId, c: usize) -> Vec<String> {
    let mut out = Vec::new();
    for child in dom.node(table).children() {
        if child.node_name() == "tbody" {
            for tr in child.children().filter(|c| c.node_name() == "tr") {
                if let Some(td) = tr.children().filter(|c| c.node_name() == "td").nth(c) {
                    out.push(td.text_content());
                }
            }
        }
    }
    out
}

/// Header `<th>` text contents in column order.
fn header_texts(dom: &TuiDom, table: NodeId) -> Vec<String> {
    header_ids(dom, table)
        .into_iter()
        .map(|th| dom.node(th).text_content())
        .collect()
}

/// `<th>` node ids in column order.
fn header_ids(dom: &TuiDom, table: NodeId) -> Vec<NodeId> {
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

/// Cascade + layout + paint, then test whether any cell renders `sym`.
fn has_symbol(dom: &mut TuiDom, sheet: &Stylesheet, viewport: Rect, sym: &str) -> bool {
    dom.cascade(sheet);
    dom.layout_dom(viewport);
    let mut buf = Buffer::empty(viewport);
    dom.paint_dom(&mut buf, viewport);
    for y in viewport.y..viewport.bottom() {
        for x in viewport.x..viewport.right() {
            if let Some(c) = buf.cell(x, y) {
                if c.symbol() == sym {
                    return true;
                }
            }
        }
    }
    false
}

#[test]
fn sort_reorders_the_materialized_window() {
    let view = view_with(&[&["banana"], &["apple"], &["cherry"]], 1);
    let mut dom = TuiDom::new();
    let table = mounted(&view, &mut dom, 8);
    assert_eq!(col0_text(&dom, table), ["banana", "apple", "cherry"]);

    view.sort(&mut dom, 0, SortDir::Ascending);
    assert_eq!(col0_text(&dom, table), ["apple", "banana", "cherry"]);

    view.sort(&mut dom, 0, SortDir::Descending);
    assert_eq!(col0_text(&dom, table), ["cherry", "banana", "apple"]);
}

#[test]
fn sort_marks_the_header_and_toggles_direction_and_column() {
    let view = view_with(&[&["b", "x"], &["a", "y"]], 2);
    let mut dom = TuiDom::new();
    let table = mounted(&view, &mut dom, 8);
    let hs = header_ids(&dom, table);

    view.sort(&mut dom, 1, SortDir::Ascending);
    assert_eq!(dom.get_attribute(hs[1], "data-sort"), Some("asc"));
    assert_eq!(
        dom.get_attribute(hs[0], "data-sort"),
        None,
        "other header clear"
    );

    view.toggle_sort(&mut dom, 1); // same column → flip
    assert_eq!(dom.get_attribute(hs[1], "data-sort"), Some("desc"));

    view.toggle_sort(&mut dom, 0); // switch column → col0 asc, col1 cleared
    assert_eq!(dom.get_attribute(hs[0], "data-sort"), Some("asc"));
    assert_eq!(dom.get_attribute(hs[1], "data-sort"), None);
}

#[test]
fn sort_clears_the_index_keyed_selection() {
    let view = view_with(&[&["b"], &["a"], &["c"]], 1);
    let mut dom = TuiDom::new();
    let _table = mounted(&view, &mut dom, 8);
    view.set_selection_mode(SelectionMode::Cell);
    view.select_all(&mut dom);
    assert!(view.selection().is_active(), "selection set");

    view.sort(&mut dom, 0, SortDir::Ascending);
    assert!(
        !view.selection().is_active(),
        "sorting clears the selection (it's keyed by row index)"
    );
}

#[test]
fn sorted_header_shows_the_direction_glyph() {
    let view = view_with(&[&["b"], &["a"]], 1);
    let mut dom = TuiDom::new();
    let _table = mounted(&view, &mut dom, 8);
    let vp = Rect::new(0, 0, 40, 12);
    let sheet = highlight_stylesheet();

    assert!(
        !has_symbol(&mut dom, &sheet, vp, "▲"),
        "no glyph before sorting"
    );
    view.sort(&mut dom, 0, SortDir::Ascending);
    assert!(
        has_symbol(&mut dom, &sheet, vp, "▲"),
        "▲ after ascending sort"
    );
    view.toggle_sort(&mut dom, 0);
    assert!(
        has_symbol(&mut dom, &sheet, vp, "▼"),
        "▼ after toggling to descending"
    );
    assert!(
        !has_symbol(&mut dom, &sheet, vp, "▲"),
        "old direction glyph removed"
    );
}

#[test]
fn move_column_reorders_headers_and_cells() {
    let view = view_with(&[&["a0", "b0", "c0"], &["a1", "b1", "c1"]], 3);
    let mut dom = TuiDom::new();
    let table = mounted(&view, &mut dom, 8);
    assert_eq!(header_texts(&dom, table), ["c0", "c1", "c2"]);

    view.move_column(&mut dom, 0, 2); // first column → end
    assert_eq!(header_texts(&dom, table), ["c1", "c2", "c0"]);
    // The moved column's data now lives at position 2; old col 1 is at 0.
    assert_eq!(col_text(&dom, table, 0), ["b0", "b1"]);
    assert_eq!(col_text(&dom, table, 2), ["a0", "a1"]);
}

#[test]
fn move_column_carries_the_sort_indicator_with_the_column() {
    let view = view_with(&[&["1", "2", "3"]], 3);
    let mut dom = TuiDom::new();
    let table = mounted(&view, &mut dom, 8);
    view.sort(&mut dom, 2, SortDir::Ascending);
    let hs = header_ids(&dom, table);
    assert_eq!(dom.get_attribute(hs[2], "data-sort"), Some("asc"));

    view.move_column(&mut dom, 2, 0); // sorted column moves to the front
    assert_eq!(view.sort_state(), Some((0, SortDir::Ascending)));
    // `<th>` nodes don't move (text is reassigned), so the same ids re-checked:
    assert_eq!(dom.get_attribute(hs[0], "data-sort"), Some("asc"));
    assert_eq!(dom.get_attribute(hs[2], "data-sort"), None);
}

#[test]
fn move_column_clears_selection_and_cursor_follows() {
    let view = view_with(&[&["a0", "b0", "c0"], &["a1", "b1", "c1"]], 3);
    let mut dom = TuiDom::new();
    let _table = mounted(&view, &mut dom, 8);
    view.set_selection_mode(SelectionMode::Cell);
    view.select_all(&mut dom);
    assert!(view.selection().is_active());
    assert_eq!(view.cursor().col(), 0);

    view.move_column(&mut dom, 0, 2); // cursor's column (0) → 2
    assert!(
        !view.selection().is_active(),
        "reorder clears the selection"
    );
    assert_eq!(view.cursor().col(), 2, "cursor follows its column");
}

#[test]
fn sort_glyphs_are_configurable() {
    let view = view_with(&[&["b"], &["a"]], 1);
    let mut dom = TuiDom::new();
    let _table = mounted(&view, &mut dom, 8);
    view.set_sort_glyphs(" ^", " v"); // width-1 ASCII (terminal-ambiguity-safe)
    let vp = Rect::new(0, 0, 40, 12);
    let sheet = highlight_stylesheet();

    view.sort(&mut dom, 0, SortDir::Ascending);
    assert!(
        has_symbol(&mut dom, &sheet, vp, "^"),
        "custom ascending glyph"
    );
    assert!(
        !has_symbol(&mut dom, &sheet, vp, "▲"),
        "default glyph replaced"
    );
    view.toggle_sort(&mut dom, 0);
    assert!(
        has_symbol(&mut dom, &sheet, vp, "v"),
        "custom descending glyph"
    );
}

/// Cascade + layout + paint, then return the painted buffer as joined text.
fn buffer_text(dom: &mut TuiDom, sheet: &Stylesheet, vp: Rect) -> String {
    dom.cascade(sheet);
    dom.layout_dom(vp);
    let mut buf = Buffer::empty(vp);
    dom.paint_dom(&mut buf, vp);
    let mut s = String::new();
    for y in vp.y..vp.bottom() {
        for x in vp.x..vp.right() {
            s.push_str(buf.cell(x, y).map(|c| c.symbol()).unwrap_or(" "));
        }
        s.push('\n');
    }
    s
}

#[test]
fn hidden_column_is_marked_and_not_painted() {
    let view = view_with(&[&["A", "MID", "C"]], 3);
    let mut dom = TuiDom::new();
    let table = mounted(&view, &mut dom, 8);

    view.set_column_hidden(&mut dom, 1, true); // hide the middle column
    assert!(
        dom.has_attribute(header_ids(&dom, table)[1], "data-vt-hidden"),
        "the hidden column's header is marked"
    );

    let txt = buffer_text(&mut dom, &highlight_stylesheet(), Rect::new(0, 0, 40, 12));
    assert!(
        !txt.contains("MID"),
        "the hidden column's cell is not painted"
    );
    assert!(
        txt.contains('A') && txt.contains('C'),
        "visible columns still paint"
    );

    // Showing it again brings it back.
    view.set_column_hidden(&mut dom, 1, false);
    let txt = buffer_text(&mut dom, &highlight_stylesheet(), Rect::new(0, 0, 40, 12));
    assert!(txt.contains("MID"), "un-hidden column paints again");
}

#[test]
fn cursor_skips_a_hidden_column() {
    let view = view_with(&[&["a", "b", "c"]], 3);
    let mut dom = TuiDom::new();
    let _table = mounted(&view, &mut dom, 8);
    view.set_column_hidden(&mut dom, 1, true); // hide the middle column

    assert_eq!(view.cursor().col(), 0);
    view.navigate(&mut dom, Nav::Right); // 0 → skip hidden 1 → land on 2
    assert_eq!(view.cursor().col(), 2, "Right skips the hidden column");
    view.navigate(&mut dom, Nav::Left); // 2 → skip 1 → 0
    assert_eq!(view.cursor().col(), 0, "Left skips it too");
}
