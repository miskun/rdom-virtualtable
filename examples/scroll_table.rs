//! A scrollable virtualized table over a 500-row dataset — only the
//! visible window of rows is ever materialized into the DOM.
//!
//! ```bash
//! cargo run --example scroll_table
//! ```
//!
//! - **`j` / `k`** or **↓ / ↑** — scroll one row
//! - **Ctrl-C** — quit
//!
//! Scrolling recomputes the row window (`VirtualTable::window_for`) and
//! re-materializes just that slice (`show_window`) — so the DOM holds a
//! bounded number of `<tr>` nodes no matter how large the dataset is.

use std::cell::Cell;
use std::io;
use std::rc::Rc;

use rdom_tui::{
    App, Direction, Display, Flow, ListenerOptions, NodeId, Padding, Size, Stylesheet, TuiDom,
    TuiNodeMutExt, TuiStyle, Value,
};
use rdom_virtualtable::{Column, VirtualTable, VirtualTableView};

const ROWS: usize = 500;
const VISIBLE: u16 = 14;

fn style(dom: &mut TuiDom, id: NodeId, s: TuiStyle) {
    dom.node_mut(id).set_inline_style(s);
}

fn flex_col() -> TuiStyle {
    let mut s = TuiStyle::new().direction(Direction::Column);
    s.display = Some(Value::Specified(Display::Block));
    s.flow = Some(Value::Specified(Flow::Flex));
    s
}

fn title_str(offset: usize) -> String {
    format!(
        "rows {}..{} of {ROWS}   ·   j/k or ↑/↓ scroll · Ctrl-C quit",
        offset + 1,
        (offset + VISIBLE as usize).min(ROWS),
    )
}

fn main() -> io::Result<()> {
    let mut model = VirtualTable::new(vec![
        Column::new("id"),
        Column::new("name"),
        Column::new("status"),
    ]);
    model.set_rows(
        (0..ROWS)
            .map(|i| {
                vec![
                    format!("{i:04}"),
                    format!("item-{i}"),
                    if i % 5 == 0 { "warn" } else { "ok" }.to_string(),
                ]
            })
            .collect(),
    );
    let view = VirtualTableView::new(model);

    let mut dom = TuiDom::new();
    let root = dom.root();
    style(&mut dom, root, flex_col());

    let container = dom.create_element("div");
    style(
        &mut dom,
        container,
        flex_col()
            .width(Size::Flex(1))
            .height(Size::Flex(1))
            .padding(Padding::all(1))
            .gap(1),
    );
    dom.append_child(root, container).unwrap();

    let title = dom.create_element("div");
    style(&mut dom, title, TuiStyle::new().height(Size::Fixed(1)));
    let title_text = dom.create_text_node(&title_str(0));
    dom.append_child(title, title_text).unwrap();
    dom.append_child(container, title).unwrap();

    let table = view.mount(&mut dom);
    dom.node_mut(table).set_attribute("tabindex", "0").ok(); // focusable for keys
    dom.append_child(container, table).unwrap();

    // Initial window.
    view.show_window(&mut dom, 0, VISIBLE as usize);
    dom.set_focused(Some(table));

    let scroll = Rc::new(Cell::new(0usize));
    let max_offset = ROWS.saturating_sub(VISIBLE as usize);

    let v = view.clone();
    let sc = scroll.clone();
    dom.add_event_listener(root, "keydown", ListenerOptions::default(), move |ctx| {
        let Some(key) = ctx.event.detail.as_keyboard() else {
            return;
        };
        let cur = sc.get();
        let next = match key.key.as_str() {
            "j" | "ArrowDown" => (cur + 1).min(max_offset),
            "k" | "ArrowUp" => cur.saturating_sub(1),
            _ => return,
        };
        if next != cur {
            sc.set(next);
            let (start, count) = VirtualTable::window_for(VISIBLE, next, ROWS);
            v.show_window(ctx.dom, start, count);
            let _ = ctx
                .dom
                .node_mut(title_text)
                .set_node_value(&title_str(next));
        }
        ctx.event.prevent_default();
        ctx.request_redraw();
    })
    .unwrap();

    App::new(dom, Stylesheet::new())?.run()
}
