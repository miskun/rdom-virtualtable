//! A **windowed** table over a 100,000-row dataset that is *never resident*:
//! the table asks for a window via [`VirtualTableView::on_window_change`], a
//! synthetic source fulfils it with [`VirtualTableView::apply`], and only the
//! visible slice (plus a prefetch margin) is ever materialized or generated.
//!
//! ```bash
//! cargo run --example windowed_table
//! ```
//!
//! - **↑ / ↓** or **k / j** — move the row cursor (the window follows)
//! - **PageUp / PageDown**, **g / G** / **Home / End** — jump
//! - **⇧ + arrows** — extend a selection · **Space** toggle · **Ctrl-A** all · **Esc** clear
//! - **s** — sort the cursor's column (asc ⇄ desc); the sort is *requested* from
//!   the source, which returns the window in the new order
//! - **u** — simulate a live update to the cursor's row (an `Upsert`, no refetch)
//! - **mouse** — wheel scrolls, click selects, drag rubber-bands (autoscroll at the edge)
//! - **Ctrl-C** — quit
//!
//! ## The bridge
//!
//! `on_window_change` hands a [`WindowRequest`] (epoch + range + sort) but no
//! DOM, so the request is *enqueued*; the consumer fulfils it where the DOM is
//! in hand. Here that's synchronous — a "pump" drains the queue right after each
//! interaction and `apply`s a `Resync`. A real consumer (a SQL / streaming
//! backend) runs the query on a background runtime and `apply`s via the App's
//! inject queue; the only change is *where* the rows come from. The **epoch**
//! the table stamps and the consumer echoes back makes out-of-order / stale
//! fulfilments safe.

use std::cell::RefCell;
use std::io;
use std::ops::Range;
use std::rc::Rc;

use rdom_tui::{
    App, Direction, Display, Flow, ListenerOptions, NodeId, Padding, Size, TuiDom, TuiNodeMutExt,
    TuiStyle, Value,
};
use rdom_virtualtable::{
    CellValue, Column, Delta, Row, SelectionMode, SortDir, SortSpec, StatusLevel, VirtualTable,
    VirtualTableView, WindowRequest, highlight_stylesheet,
};

const TOTAL: usize = 100_000;
const VISIBLE: u16 = 16;

/// Per-interaction shared queue of pending window requests.
type Pending = Rc<RefCell<Vec<WindowRequest>>>;

/// The synthetic source: the row at sorted position `pos` is
/// `[id, item-<id>, status]`. Sort-aware (descending reverses the id order), so
/// a header sort round-trips through the request's [`SortSpec`].
fn synth(range: Range<usize>, sort: &[SortSpec]) -> Vec<Row> {
    let desc = matches!(sort.first().map(|s| s.dir), Some(SortDir::Descending));
    range
        .map(|pos| {
            let id = if desc { TOTAL - 1 - pos } else { pos };
            let level = match id % 7 {
                0 => StatusLevel::Error,
                1 | 2 => StatusLevel::Warn,
                _ => StatusLevel::Ok,
            };
            Row::new(
                format!("k{id}"),
                vec![
                    CellValue::from(id.to_string()),
                    CellValue::from(format!("item-{id}")),
                    CellValue::Status {
                        text: match level {
                            StatusLevel::Ok => "Running",
                            StatusLevel::Warn => "Pending",
                            StatusLevel::Error => "Failed",
                            StatusLevel::Info => "Info",
                        }
                        .into(),
                        level,
                    },
                ],
            )
        })
        .collect()
}

/// The consumer bridge: fulfil every queued request with a `Resync`, echoing the
/// epoch the table stamped (so a stale request is dropped on arrival).
fn pump(view: &VirtualTableView, dom: &mut TuiDom, pending: &Pending) {
    let reqs: Vec<WindowRequest> = pending.borrow_mut().drain(..).collect();
    for req in reqs {
        let start = req.range.start;
        let rows = synth(req.range.clone(), &req.sort);
        view.apply(dom, req.epoch, Delta::Resync { start, rows });
    }
}

fn style(dom: &mut TuiDom, id: NodeId, s: TuiStyle) {
    dom.node_mut(id).set_inline_style(s);
}

fn flex_col() -> TuiStyle {
    let mut s = TuiStyle::new().direction(Direction::Column);
    s.display = Some(Value::Specified(Display::Block));
    s.flow = Some(Value::Specified(Flow::Flex));
    s
}

fn main() -> io::Result<()> {
    let view = VirtualTableView::new(VirtualTable::new(vec![
        Column::new("id"),
        Column::new("name"),
        Column::new("status"),
    ]));

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
    let title_text = dom.create_text_node(&format!(
        "{TOTAL} rows, windowed — only ~{VISIBLE} are ever materialized  ·  \
         ↑↓ move · PgUp/PgDn jump · s sort · u live-update · ⇧+arrows select · Ctrl-C quit"
    ));
    dom.append_child(title, title_text).unwrap();
    dom.append_child(container, title).unwrap();

    let table = view.mount(&mut dom);
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    dom.append_child(container, table).unwrap();

    view.set_viewport_rows(VISIBLE);

    // The windowed source: enqueue each request; the pump fulfils it.
    let pending: Pending = Rc::new(RefCell::new(Vec::new()));
    let p = pending.clone();
    view.on_window_change(move |req| p.borrow_mut().push(req));

    // Total → drives the scrollbar extent + fires the initial window request.
    view.set_total(&mut dom, TOTAL);

    // Interaction: keyboard nav + selection, native scrollbar, mouse.
    view.install_nav(&mut dom, table, VISIBLE);
    view.enable_scrollbar(&mut dom);
    view.install_mouse(&mut dom);
    view.set_selection_mode(SelectionMode::Cell);

    // `u` simulates a live update to the cursor's row — an `Upsert` the source
    // pushes directly, with no refetch (the row is already in the window).
    let vu = view.clone();
    dom.add_event_listener(table, "keydown", ListenerOptions::default(), move |ctx| {
        let Some(kbd) = ctx.event.detail.as_keyboard() else {
            return;
        };
        if kbd.key != "u" {
            return;
        }
        let row = vu.cursor().row();
        if let Some(rk) = vu.row_key_at(row) {
            let ep = vu.window_epoch();
            vu.apply(
                ctx.dom,
                ep,
                Delta::Upsert {
                    rows: vec![Row::new(
                        rk,
                        vec![
                            CellValue::from(row.to_string()),
                            CellValue::from(format!("item-{row} (updated)")),
                            CellValue::Status {
                                text: "Updated".into(),
                                level: StatusLevel::Info,
                            },
                        ],
                    )],
                },
            );
        }
        ctx.request_redraw();
    })
    .unwrap();

    // The pump: after any event that can move the window (or sort), drain the
    // queued requests and fulfil them. Registered AFTER the view's own handlers,
    // so it runs once those have enqueued. Covers keyboard, wheel, and mouse.
    for ev in ["keydown", "scroll", "mousedown", "mouseup", "click"] {
        let v = view.clone();
        let pend = pending.clone();
        dom.add_event_listener(table, ev, ListenerOptions::default(), move |ctx| {
            pump(&v, ctx.dom, &pend);
            ctx.request_redraw();
        })
        .unwrap();
    }

    // Fulfil the initial window request before the first paint.
    pump(&view, &mut dom, &pending);
    dom.set_focused(Some(table));

    App::new(dom, highlight_stylesheet())?.run()
}
