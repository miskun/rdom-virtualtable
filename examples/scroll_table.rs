//! A navigable virtualized table over a 500-row dataset — only the visible
//! window of rows is ever materialized into the DOM, and a keyboard cursor
//! moves over the full dataset with the highlight following it.
//!
//! ```bash
//! cargo run --example scroll_table
//! ```
//!
//! - **↑ / ↓** or **k / j** — move the row cursor
//! - **← / →** or **h / l** — move the column cursor
//! - **g / G** or **Home / End** — first / last row
//! - **PageUp / PageDown** — jump a page
//! - **x** — hide the cursor's column · **c** (or click the `…` chip) — column chooser
//!   (checklist of all columns; **↑ / ↓** move, **Enter / Space** toggle, **Esc** close)
//! - **Ctrl-C** — quit
//!
//! [`VirtualTableView::install_nav`] wires the keymap: it moves a logical
//! cursor, re-materializes the window when the cursor scrolls past it
//! (`VirtualTable::window_for` + `show_window`), and writes `data-active-*`
//! attributes that [`highlight_stylesheet`] turns into a cross-hair
//! highlight — focus-gated, so it only shows while the table is focused.

use std::io;

use rdom_tui::{
    App, Direction, Display, Flow, ListenerOptions, NodeId, Padding, Size, TuiDom, TuiNodeMutExt,
    TuiStyle, Value,
};
use rdom_virtualtable::{
    Column, SelectionMode, VirtualTable, VirtualTableView, highlight_stylesheet,
};

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

fn title_str(row: usize, col: usize) -> String {
    format!(
        "row {row} · col {col} / {ROWS}  ·  ↑→↓← move · Shift+↑→↓← select · Space toggle · \
         s sort · < > move · x hide · c columns · +/- resize · Ctrl-A all · Esc clear · Ctrl-C quit",
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
    let title_text = dom.create_text_node(&title_str(0, 0));
    dom.append_child(title, title_text).unwrap();
    dom.append_child(container, title).unwrap();

    let table = view.mount(&mut dom);
    dom.node_mut(table).set_attribute("tabindex", "0").ok(); // focusable for keys
    dom.append_child(container, table).unwrap();

    // Initial window, then wire keyboard navigation (drives the window +
    // the cursor highlight from here on).
    view.show_window(&mut dom, 0, VISIBLE as usize);
    view.install_nav(&mut dom, table, VISIBLE);
    // Native vertical scrollbar: the <tbody> scrolls, the thumb reflects all
    // 500 rows (spacer rows), and the mouse wheel / drag scrolls decoupled from
    // the cursor. Keyboard nav still scrolls the view to keep the cursor visible.
    view.enable_scrollbar(&mut dom);
    // Opt into the column-actions column: a persistent `…` header chip whose
    // dropdown is a checklist of every column (native checkboxes) — check to
    // show, uncheck to hide. Open it with `c` or a chip click.
    view.enable_column_actions(&mut dom);
    // Opt into cell selection — Shift+arrows extend a rectangle, Space toggles,
    // Ctrl-A selects all, Esc clears. (Try `SelectionMode::Row` for whole-row
    // selection, or leave it `None` to disable.)
    view.set_selection_mode(SelectionMode::Cell);
    dom.set_focused(Some(table));

    // `s` sorts the cursor's column (toggles asc⇄desc); `<` / `>` move the
    // cursor's column left / right (one key ± Shift — friendlier than `[`/`]`,
    // which need AltGr on many non-US layouts). `install_nav` leaves these keys
    // unhandled, so this listener picks them up.
    let vs = view.clone();
    dom.add_event_listener(table, "keydown", ListenerOptions::default(), move |ctx| {
        let Some(kbd) = ctx.event.detail.as_keyboard() else {
            return;
        };
        let col = vs.cursor().col();
        let cols = vs.with(|t| t.columns().len());
        match kbd.key.as_str() {
            "s" => vs.toggle_sort(ctx.dom, col),
            "<" if col > 0 => vs.move_column(ctx.dom, col, col - 1),
            ">" if col + 1 < cols => vs.move_column(ctx.dom, col, col + 1),
            "x" => {
                let hidden = vs.with(|t| t.is_column_hidden(col));
                vs.set_column_hidden(ctx.dom, col, !hidden);
            }
            // `c` (columns) opens the column chooser — the chip's mouse click
            // does the same. Esc (via install_nav) closes it. Guard against
            // Ctrl-C (quit), which also arrives as key "c".
            "c" if !kbd.modifiers.ctrl && !kbd.modifiers.meta => vs.toggle_column_menu(ctx.dom),
            "+" | "=" => {
                let w = vs.column_width(ctx.dom, col).unwrap_or(8);
                vs.set_column_width(ctx.dom, col, Some(w.saturating_add(1)));
            }
            "-" => {
                let w = vs.column_width(ctx.dom, col).unwrap_or(8);
                vs.set_column_width(ctx.dom, col, Some(w.saturating_sub(1).max(1)));
            }
            _ => return,
        }
        ctx.request_redraw();
    })
    .unwrap();

    // Live cursor read-out in the title. Bubbles to root after the table's
    // nav handler has moved the cursor, so it sees the post-move position.
    let v = view.clone();
    dom.add_event_listener(root, "keydown", ListenerOptions::default(), move |ctx| {
        let c = v.cursor();
        let _ = ctx
            .dom
            .node_mut(title_text)
            .set_node_value(&title_str(c.row(), c.col()));
        ctx.request_redraw();
    })
    .unwrap();

    // Just the cursor-highlight rules. As of rdom-tui 0.3.4 the UA focus tint
    // is scoped to interactive controls, so a focused `<table>` is no longer
    // washed gray — no `table:focus { background: reset }` workaround needed.
    App::new(dom, highlight_stylesheet())?.run()
}
