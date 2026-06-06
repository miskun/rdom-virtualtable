//! Keyboard-navigation + highlight contract: navigating the cursor writes
//! `data-active-*` attributes onto the materialized window, the highlight
//! follows the cursor across window shifts, and the default focus-gated
//! stylesheet only paints the cursor while the table is focused.

use rdom_tui::render::{Buffer, LayoutExt, PaintExt, Rect};
use rdom_tui::style::{CascadeExt, Stylesheet};
use rdom_tui::{Color, NodeId, TuiDom, TuiStyle};
use rdom_virtualtable::{
    Column, Nav, SelectionMode, VirtualTable, VirtualTableView, highlight_stylesheet,
};

fn grid(rows: usize, cols: usize) -> VirtualTableView {
    let columns = (0..cols).map(|c| Column::new(format!("c{c}"))).collect();
    let mut model = VirtualTable::new(columns);
    model.set_rows(
        (0..rows)
            .map(|r| (0..cols).map(|c| format!("r{r}c{c}").into()).collect())
            .collect(),
    );
    VirtualTableView::new(model)
}

/// Collect the `<tr>` node ids currently under `<tbody>`, in order.
fn tbody_rows(dom: &TuiDom, table: NodeId) -> Vec<NodeId> {
    for child in dom.node(table).children() {
        if child.node_name() == "tbody" {
            return child
                .children()
                .filter(|c| c.node_name() == "tr")
                .map(|c| c.id())
                .collect();
        }
    }
    Vec::new()
}

/// `<td>` node ids of a given `<tr>`, in column order.
fn row_cells(dom: &TuiDom, tr: NodeId) -> Vec<NodeId> {
    dom.node(tr)
        .children()
        .filter(|c| c.node_name() == "td")
        .map(|c| c.id())
        .collect()
}

fn header_cells(dom: &TuiDom, table: NodeId) -> Vec<NodeId> {
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

/// Cascade + layout + paint with `sheet`, then count cells painted with `bg`.
fn count_bg(dom: &mut TuiDom, sheet: &Stylesheet, viewport: Rect, bg: Color) -> usize {
    dom.cascade(sheet);
    dom.layout_dom(viewport);
    let mut buf = Buffer::empty(viewport);
    dom.paint_dom(&mut buf, viewport);
    let mut n = 0;
    for y in viewport.y..viewport.bottom() {
        for x in viewport.x..viewport.right() {
            if let Some(c) = buf.cell(x, y) {
                if c.bg == bg {
                    n += 1;
                }
            }
        }
    }
    n
}

/// The default highlight, as a real app wires it. As of rdom-tui 0.3.4 the UA
/// focus tint is scoped to interactive controls, so a focused `<table>` is not
/// washed — no `table:focus { background: reset }` workaround is needed; this
/// is just `highlight_stylesheet()`.
fn highlight_sheet() -> Stylesheet {
    highlight_stylesheet()
}

/// Mount a focused, navigated grid ready for paint assertions: cursor at row 1,
/// table focused, an 8-row window shown.
fn focused_navigated_grid() -> (TuiDom, NodeId) {
    let view = grid(20, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);
    view.navigate(&mut dom, Nav::Down); // cursor at row 1
    dom.set_focused(Some(table));
    (dom, table)
}

#[test]
fn highlight_marks_cursor_row_column_and_cell() {
    let view = grid(20, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);

    // Move to (row 2, col 1).
    view.navigate(&mut dom, Nav::Down);
    view.navigate(&mut dom, Nav::Down);
    view.navigate(&mut dom, Nav::Right);
    assert_eq!((view.cursor().row(), view.cursor().col()), (2, 1));

    let rows = tbody_rows(&dom, table);
    for (i, &tr) in rows.iter().enumerate() {
        let row_active = i == 2;
        assert_eq!(
            dom.has_attribute(tr, "data-active-row"),
            row_active,
            "tr {i} data-active-row"
        );
        for (c, td) in row_cells(&dom, tr).into_iter().enumerate() {
            assert_eq!(
                dom.has_attribute(td, "data-active-col"),
                c == 1,
                "tr {i} td {c} data-active-col"
            );
            assert_eq!(
                dom.has_attribute(td, "data-active-cell"),
                row_active && c == 1,
                "tr {i} td {c} data-active-cell"
            );
        }
    }

    // Header column under the cursor is flagged too.
    let headers = header_cells(&dom, table);
    assert!(dom.has_attribute(headers[1], "data-active-col"));
    assert!(!dom.has_attribute(headers[0], "data-active-col"));
}

#[test]
fn navigation_past_window_shifts_and_rehighlights() {
    let view = grid(50, 2);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(5);
    view.show_window(&mut dom, 0, 5);

    for _ in 0..10 {
        view.navigate(&mut dom, Nav::Down);
    }
    assert_eq!(view.cursor().row(), 10);

    // The window stayed bounded and shifted to keep the cursor visible.
    assert_eq!(view.mounted_row_count(), 5, "window stays bounded");
    let scroll = view.window_start();
    assert_eq!(scroll, 6, "window follows cursor (10 + 1 - 5)");

    // Exactly one materialized row carries the highlight — the cursor's.
    let rows = tbody_rows(&dom, table);
    let active: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|&(_, &tr)| dom.has_attribute(tr, "data-active-row"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        active,
        vec![10 - scroll],
        "highlight on the cursor's pool row"
    );
}

#[test]
fn highlight_is_focus_gated_at_paint() {
    // Same setup as `focused_navigated_grid` but we toggle focus by hand.
    let view = grid(20, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);
    view.navigate(&mut dom, Nav::Down); // cursor at row 1

    let sheet = highlight_sheet(); // defaults + table:focus tint reset
    let viewport = Rect::new(0, 0, 40, 12);
    let cursor_bg = Color::Rgb(0x2d, 0x2f, 0x31); // #2d2f31 — the cursor cell (gray, no selection)

    // Unfocused: the focus-gated rule must not paint the cursor.
    assert_eq!(
        count_bg(&mut dom, &sheet, viewport, cursor_bg),
        0,
        "no cursor bg when unfocused"
    );

    // Focused: the active cell paints with the cursor background.
    dom.set_focused(Some(table));
    assert!(
        count_bg(&mut dom, &sheet, viewport, cursor_bg) > 0,
        "cursor bg appears once the table is focused"
    );
}

#[test]
fn highlight_colors_are_opt_in_defaults() {
    // The `data-active-*` attributes are the contract; the colors are not.
    // With a bare sheet (no highlight rules) the cursor/line colors are never
    // painted — styling is purely the consumer's CSS.
    let (mut dom, _table) = focused_navigated_grid();
    let viewport = Rect::new(0, 0, 40, 12);
    let cell = Color::Rgb(0x2d, 0x2f, 0x31); // cursor cell (gray, no selection)
    let line = Color::Rgb(0x18, 0x1a, 0x1c);

    assert_eq!(
        count_bg(&mut dom, &Stylesheet::bare(), viewport, cell),
        0,
        "without highlight CSS, the cursor-cell color is never painted"
    );
    assert_eq!(
        count_bg(&mut dom, &Stylesheet::bare(), viewport, line),
        0,
        "without highlight CSS, the row/column tint is never painted"
    );
}

#[test]
fn consumer_css_overrides_default_colors() {
    // The defaults are wrapped in `:where()` → zero specificity, so even a
    // PLAIN low-specificity author rule overrides them — no `table:focus`
    // prefix, no specificity matching. `td[data-active-cell]` is (0,1,1),
    // which would have LOST to the un-wrapped `table:focus td[...]` (0,2,2)
    // default but beats the `:where()`-wrapped one. This is the browser-easy
    // override the `:where()` wiring buys us.
    let (mut dom, _table) = focused_navigated_grid();
    let viewport = Rect::new(0, 0, 40, 12);

    let custom = Color::Rgb(0x80, 0x00, 0x00); // a color our defaults never use
    let sheet = highlight_sheet()
        .rule("td[data-active-cell]", TuiStyle::new().bg(custom))
        .unwrap();

    assert!(
        count_bg(&mut dom, &sheet, viewport, custom) > 0,
        "a plain low-specificity author rule paints its own cursor color"
    );
    assert_eq!(
        count_bg(&mut dom, &sheet, viewport, Color::Rgb(0x2d, 0x2f, 0x31)),
        0,
        "the zero-specificity default cursor color is fully overridden"
    );
}

#[test]
fn focused_table_needs_no_focus_tint_reset() {
    // rdom-tui 0.3.4 scopes the UA focus tint to interactive controls, so a
    // focused `<table>` is no longer washed with the focus background — the
    // old `table:focus { background: reset }` workaround is now a no-op.
    // Proof: rendering with vs without the reset paints the SAME number of
    // focus-color cells (only the cursor cross-hair, no full-table wash).
    let (mut dom, _table) = focused_navigated_grid();
    let viewport = Rect::new(0, 0, 40, 12);
    let focus_color = Color::Rgb(0x2d, 0x2f, 0x31);

    let with_reset = highlight_stylesheet()
        .rule("table:focus", TuiStyle::new().bg(Color::Reset))
        .unwrap();

    let without = count_bg(&mut dom, &highlight_stylesheet(), viewport, focus_color);
    let with = count_bg(&mut dom, &with_reset, viewport, focus_color);
    assert_eq!(
        without, with,
        "the table:focus reset changes nothing in 0.3.4 — a focused table isn't tinted"
    );
}

// ── Selection (M2) ────────────────────────────────────────────────

#[test]
fn cell_selection_marks_the_rectangle() {
    let view = grid(10, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(10);
    view.show_window(&mut dom, 0, 10);
    view.set_selection_mode(SelectionMode::Cell);

    // From (0,0): Shift-Down, Down, Right → rect rows 0..2, cols 0..1.
    view.extend_selection(&mut dom, Nav::Down);
    view.extend_selection(&mut dom, Nav::Down);
    view.extend_selection(&mut dom, Nav::Right);

    let rows = tbody_rows(&dom, table);
    for (i, &tr) in rows.iter().enumerate() {
        for (c, td) in row_cells(&dom, tr).into_iter().enumerate() {
            let want = i <= 2 && c <= 1;
            assert_eq!(
                dom.has_attribute(td, "data-selected"),
                want,
                "cell (row {i}, col {c}) data-selected"
            );
        }
    }
}

#[test]
fn row_selection_marks_whole_rows() {
    let view = grid(10, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(10);
    view.show_window(&mut dom, 0, 10);
    view.set_selection_mode(SelectionMode::Row);

    // From row 0: Shift-Down twice → rows 0..2 selected, every column.
    view.extend_selection(&mut dom, Nav::Down);
    view.extend_selection(&mut dom, Nav::Down);

    let rows = tbody_rows(&dom, table);
    for (i, &tr) in rows.iter().enumerate() {
        let want = i <= 2;
        assert_eq!(
            dom.has_attribute(tr, "data-selected"),
            want,
            "row {i} <tr> data-selected"
        );
        for (c, td) in row_cells(&dom, tr).into_iter().enumerate() {
            assert_eq!(
                dom.has_attribute(td, "data-selected"),
                want,
                "row {i} col {c} <td> data-selected (whole row in Row mode)"
            );
        }
    }
}

#[test]
fn space_toggles_the_cursor_cell() {
    let view = grid(10, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(10);
    view.show_window(&mut dom, 0, 10);
    view.set_selection_mode(SelectionMode::Cell);

    view.navigate(&mut dom, Nav::Down); // cursor (1,0)
    view.navigate(&mut dom, Nav::Right); // cursor (1,1)
    view.toggle_selection(&mut dom);

    let td = row_cells(&dom, tbody_rows(&dom, table)[1])[1];
    assert!(
        dom.has_attribute(td, "data-selected"),
        "toggled cell selected"
    );
    view.toggle_selection(&mut dom);
    assert!(
        !dom.has_attribute(td, "data-selected"),
        "toggling again clears it"
    );
}

#[test]
fn select_all_then_clear() {
    let view = grid(8, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);
    view.set_selection_mode(SelectionMode::Cell);

    view.select_all(&mut dom);
    let all_selected = |dom: &TuiDom| {
        tbody_rows(dom, table).iter().all(|&tr| {
            row_cells(dom, tr)
                .iter()
                .all(|&td| dom.has_attribute(td, "data-selected"))
        })
    };
    assert!(all_selected(&dom), "Ctrl-A selects every cell");

    view.clear_selection(&mut dom);
    let none_selected = tbody_rows(&dom, table).iter().all(|&tr| {
        row_cells(&dom, tr)
            .iter()
            .all(|&td| !dom.has_attribute(td, "data-selected"))
    });
    assert!(none_selected, "Esc clears the selection");
}

#[test]
fn no_selection_attributes_when_mode_is_none() {
    // Default mode is None — extend/toggle/select-all are no-ops.
    let view = grid(8, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);

    view.extend_selection(&mut dom, Nav::Down);
    view.toggle_selection(&mut dom);
    view.select_all(&mut dom);

    let any = tbody_rows(&dom, table).iter().any(|&tr| {
        row_cells(&dom, tr)
            .iter()
            .any(|&td| dom.has_attribute(td, "data-selected"))
    });
    assert!(!any, "no data-selected when SelectionMode::None");
    assert!(!view.selection().is_active());
}

#[test]
fn selection_paints_when_focused() {
    let view = grid(20, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);
    view.set_selection_mode(SelectionMode::Cell);
    // Toggle (0,0), then move the cursor away (different row AND column) so the
    // toggled cell stays plainly selected — not on the cross-hair, which would
    // blend to #2b557e.
    view.toggle_selection(&mut dom); // toggle (0,0)
    view.navigate(&mut dom, Nav::Down);
    view.navigate(&mut dom, Nav::Down);
    view.navigate(&mut dom, Nav::Right); // cursor (2,1); (0,0) is plain-selected
    dom.set_focused(Some(table));

    let viewport = Rect::new(0, 0, 40, 12);
    let selection_blue = Color::Rgb(0x1e, 0x3a, 0x5f);
    assert!(
        count_bg(&mut dom, &highlight_stylesheet(), viewport, selection_blue) > 0,
        "a selected cell off the cursor's row/column paints the plain selection color"
    );
}

#[test]
fn selection_blends_with_the_cursor_crosshair() {
    let view = grid(20, 4);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);
    view.set_selection_mode(SelectionMode::Cell);
    // Rect (0,0)..(2,2); cursor ends at (2,2) → active row 2 + active col 2
    // intersect the selection.
    view.extend_selection(&mut dom, Nav::Down);
    view.extend_selection(&mut dom, Nav::Down);
    view.extend_selection(&mut dom, Nav::Right);
    view.extend_selection(&mut dom, Nav::Right);
    dom.set_focused(Some(table));

    let vp = Rect::new(0, 0, 40, 12);
    let plain = Color::Rgb(0x1e, 0x3a, 0x5f); // selection outside the cross-hair
    let blend = Color::Rgb(0x2b, 0x55, 0x7e); // selection ∩ active row/col
    assert!(
        count_bg(&mut dom, &highlight_stylesheet(), vp, blend) > 0,
        "selected cells in the active row/column use the blend (#2b557e)"
    );
    assert!(
        count_bg(&mut dom, &highlight_stylesheet(), vp, plain) > 0,
        "selected cells outside the cross-hair use the plain selection (#1e3a5f)"
    );
}

#[test]
fn cursor_cell_is_blue_only_when_selected() {
    let view = grid(20, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);
    view.set_selection_mode(SelectionMode::Cell);
    dom.set_focused(Some(table));

    let vp = Rect::new(0, 0, 40, 12);
    let gray = Color::Rgb(0x2d, 0x2f, 0x31);
    let blue = Color::Rgb(0x3a, 0x6e, 0xa5);

    // Cursor on an unselected cell → gray (no blue field around it).
    view.navigate(&mut dom, Nav::Down); // cursor (1,0), nothing selected
    assert!(
        count_bg(&mut dom, &highlight_stylesheet(), vp, gray) > 0,
        "an unselected cursor cell is gray"
    );
    assert_eq!(
        count_bg(&mut dom, &highlight_stylesheet(), vp, blue),
        0,
        "no blue cursor while nothing is selected"
    );

    // Extend so the cursor cell is itself selected → it turns blue.
    view.extend_selection(&mut dom, Nav::Down); // cursor (2,0), inside the range
    assert!(
        count_bg(&mut dom, &highlight_stylesheet(), vp, blue) > 0,
        "a selected cursor cell turns blue to fit the selection field"
    );
}

#[test]
fn plain_move_collapses_select_all_but_keeps_toggles() {
    // Best-practice: a plain (unmodified) arrow collapses the *transient*
    // selections — an in-progress range and a Ctrl-A select-all — but the
    // explicitly Space-toggled set survives until Esc.
    let view = grid(20, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);
    view.set_selection_mode(SelectionMode::Cell);

    // Ctrl-A selects everything; a plain arrow collapses it.
    view.select_all(&mut dom);
    assert!(view.is_cell_selected(5, 2), "Ctrl-A selects all");
    view.navigate(&mut dom, Nav::Down);
    assert!(
        !view.is_cell_selected(5, 2),
        "a plain move collapses Ctrl-A select-all"
    );

    // A Space-toggle is the explicit accumulate gesture — it survives moves.
    view.toggle_selection(&mut dom); // toggles the current cursor cell
    let c = view.cursor();
    assert!(
        view.is_cell_selected(c.row(), c.col()),
        "Space toggles the cursor cell"
    );
    view.navigate(&mut dom, Nav::Down);
    assert!(
        view.is_cell_selected(c.row(), c.col()),
        "the toggled cell survives a plain move"
    );
}

#[test]
fn space_commits_a_shift_range_and_builds_multiple_ranges() {
    // A live Shift-range + Space commits the whole rectangle into the sticky
    // set (and collapses the range), so Shift-select → Space → move →
    // Shift-select → Space accumulates multiple persistent ranges.
    let view = grid(20, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(12);
    view.show_window(&mut dom, 0, 12);
    view.set_selection_mode(SelectionMode::Cell);

    // Range A: Shift+Down (rect (0,0)..(1,0)), Space commits it.
    view.extend_selection(&mut dom, Nav::Down);
    view.toggle_selection(&mut dom);
    view.navigate(&mut dom, Nav::Down); // plain move away → A must persist
    assert!(view.is_cell_selected(0, 0), "committed range A persists");
    assert!(view.is_cell_selected(1, 0));

    // Range B: move to (4,1), Shift+Down (rect (4,1)..(5,1)), Space commits.
    view.navigate(&mut dom, Nav::Down); // (3,0)
    view.navigate(&mut dom, Nav::Down); // (4,0)
    view.navigate(&mut dom, Nav::Right); // (4,1)
    view.extend_selection(&mut dom, Nav::Down);
    view.toggle_selection(&mut dom);
    assert!(view.is_cell_selected(0, 0), "range A still held");
    assert!(view.is_cell_selected(4, 1), "range B held");
    assert!(view.is_cell_selected(5, 1));
    assert!(
        !view.is_cell_selected(3, 0),
        "the gap between ranges is unselected"
    );
}

#[test]
fn highlight_survives_focus_on_the_scroll_tbody() {
    // Regression for the "click past the last column kills the highlight" bug.
    // Clicking the empty body area moves focus from <table> to the focusable
    // scroll <tbody> (a descendant). The focus-gated highlight must still paint
    // — it's gated on `:focus-within`, not `:focus`, so focus anywhere in the
    // table region keeps the cross-hair visible.
    let view = grid(20, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    dom.node_mut(table).set_attribute("tabindex", "0").ok();
    view.set_viewport_rows(8);
    view.enable_scrollbar(&mut dom); // makes <tbody> a focusable scroll container
    view.show_window(&mut dom, 0, 8);
    view.navigate(&mut dom, Nav::Down); // cursor at row 1 → highlight attrs written

    // Focus the <tbody>, NOT the <table> — what the substrate does when you
    // click the scroll area past the last column.
    let tbody = dom
        .node(table)
        .children()
        .find(|c| c.node_name() == "tbody")
        .map(|c| c.id())
        .expect("table has a tbody");
    dom.set_focused(Some(tbody));

    // The cursor cell paints #2d2f31 when the highlight is active.
    let cursor_bg = Color::Rgb(0x2d, 0x2f, 0x31);
    let n = count_bg(
        &mut dom,
        &highlight_stylesheet(),
        Rect::new(0, 0, 40, 12),
        cursor_bg,
    );
    assert!(
        n > 0,
        "the cursor highlight must still paint when focus is on the scroll <tbody>, not the <table>"
    );
}

#[test]
fn selected_row_keys_dedupes_by_row() {
    let view = grid(20, 3);
    let mut dom = TuiDom::new();
    let root = dom.root();
    let table = view.mount(&mut dom);
    dom.append_child(root, table).unwrap();
    view.set_viewport_rows(8);
    view.show_window(&mut dom, 0, 8);
    view.set_selection_mode(SelectionMode::Cell);
    // Three toggled cells across two rows (row 0 in two columns, row 5 once).
    view.toggle_at(&mut dom, 0, 0);
    view.toggle_at(&mut dom, 0, 2);
    view.toggle_at(&mut dom, 5, 1);
    let mut keys: Vec<String> = view
        .selected_row_keys()
        .iter()
        .map(|k| k.to_string())
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["0", "5"],
        "two unique rows despite three toggled cells"
    );
}
