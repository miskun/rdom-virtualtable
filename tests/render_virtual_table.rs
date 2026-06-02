//! Integration test: a `VirtualTableView` materializes only its window
//! of rows into a real `<table>` subtree and renders through the rdom-tui
//! pipeline (headless).

use rdom_tui::TuiDom;
use rdom_tui::render::{Buffer, LayoutExt, PaintExt, Rect};
use rdom_tui::style::{CascadeExt, Stylesheet};
use rdom_virtualtable::{Column, VirtualTable, VirtualTableView};

fn render(dom: &mut TuiDom, sheet: &Stylesheet, viewport: Rect) -> Buffer {
    dom.cascade(sheet);
    dom.layout_dom(viewport);
    let mut buf = Buffer::empty(viewport);
    dom.paint_dom(&mut buf, viewport);
    buf
}

fn screen_text(buf: &Buffer) -> String {
    let area = buf.area;
    let mut s = String::new();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(c) = buf.cell(x, y) {
                if !c.is_spacer() {
                    s.push_str(c.symbol());
                }
            }
        }
        s.push('\n');
    }
    s
}

fn big_table() -> VirtualTableView {
    let mut model = VirtualTable::new(vec![Column::new("id"), Column::new("name")]);
    model.set_rows(
        (0..1000)
            .map(|i| vec![format!("{i}"), format!("row-{i}")])
            .collect(),
    );
    VirtualTableView::new(model)
}

#[test]
fn materializes_only_the_window() {
    let view = big_table();
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();

    // 1000-row dataset, but only show 5 rows starting at 100.
    let (start, count) = VirtualTable::window_for(5, 100, view.with(|t| t.row_count()));
    view.show_window(&mut dom, start, count);

    assert_eq!(
        view.mounted_row_count(),
        5,
        "only the window's rows should be in the DOM, not all 1000"
    );

    let sheet = Stylesheet::new();
    let buf = render(&mut dom, &sheet, Rect::new(0, 0, 30, 10));
    let text = screen_text(&buf);

    // Header + windowed rows present; out-of-window rows absent.
    assert!(text.contains("name"), "header should render");
    assert!(text.contains("row-100"), "first windowed row should render");
    assert!(text.contains("row-104"), "last windowed row should render");
    assert!(
        !text.contains("row-0\n") && !text.contains("row-200"),
        "out-of-window rows must not render: {text:?}"
    );
}

#[test]
fn show_window_replaces_previous_rows() {
    let view = big_table();
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();

    view.show_window(&mut dom, 0, 5);
    assert_eq!(view.mounted_row_count(), 5);

    // Scroll down: a new window replaces the old rows (count stays bounded).
    view.show_window(&mut dom, 500, 5);
    assert_eq!(view.mounted_row_count(), 5);

    let buf = render(&mut dom, &Stylesheet::new(), Rect::new(0, 0, 30, 10));
    let text = screen_text(&buf);
    assert!(text.contains("row-500"), "new window should render");
    assert!(!text.contains("row-0\n"), "previous window should be gone");
}

#[test]
fn window_past_end_renders_header_only() {
    let view = big_table();
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();

    let (start, count) = VirtualTable::window_for(5, 5000, 1000);
    assert_eq!((start, count), (1000, 0));
    view.show_window(&mut dom, start, count);
    assert_eq!(view.mounted_row_count(), 0);

    let buf = render(&mut dom, &Stylesheet::new(), Rect::new(0, 0, 30, 5));
    assert!(screen_text(&buf).contains("name"), "header still renders");
}
