//! Pure keyboard-navigation model for a virtualized grid.
//!
//! This module holds no DOM state — it is the unit-tested heart of table
//! navigation. [`VirtualTableView`](crate::VirtualTableView) owns a
//! [`GridCursor`], applies [`Nav`] moves to it, and reflects the result onto
//! the materialized row window as `data-active-row` / `data-active-col` /
//! `data-active-cell` attributes that CSS can target.

/// A navigation intent — the abstract move a key press maps to.
///
/// [`nav_for_key`] turns a `KeyboardEvent.key` into one of these, and
/// [`GridCursor::navigate`] applies it. Keeping the intent separate from the
/// key string lets consumers build their own keymaps over the same moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    /// Jump to the first row (web `Home` / vim `g`).
    Top,
    /// Jump to the last row (web `End` / vim `G`).
    Bottom,
    PageUp,
    PageDown,
}

/// A logical cursor over a virtual grid: the active `(row, col)` cell plus a
/// vertical `scroll` offset (the top visible data row).
///
/// Pure and `Copy` — holds no DOM. `row`/`col` are logical indices over the
/// *whole* dataset, not the materialized window. Movement is always clamped to
/// the grid bounds, so a cursor can never address a cell that doesn't exist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GridCursor {
    row: usize,
    col: usize,
    scroll: usize,
}

impl GridCursor {
    /// A cursor at the origin with no scroll.
    pub fn new() -> Self {
        Self::default()
    }

    /// The active row (logical index over the full dataset).
    pub fn row(&self) -> usize {
        self.row
    }

    /// The active column.
    pub fn col(&self) -> usize {
        self.col
    }

    /// The top visible data row (window start, before any overscan).
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Place the cursor at `(row, col)`, clamped to a `rows × cols` grid.
    /// Leaves `scroll` untouched — pair with [`follow`](Self::follow).
    #[must_use]
    pub fn at(mut self, row: usize, col: usize, rows: usize, cols: usize) -> Self {
        self.row = row.min(rows.saturating_sub(1));
        self.col = col.min(cols.saturating_sub(1));
        self
    }

    /// Move the active cell per `nav`, clamped to a `rows × cols` grid.
    ///
    /// `page` is the row step for `PageUp`/`PageDown` (typically the visible
    /// row count). Scroll is left untouched — call [`follow`](Self::follow)
    /// afterwards to keep the cursor on screen.
    #[must_use]
    pub fn navigate(mut self, nav: Nav, rows: usize, cols: usize, page: usize) -> Self {
        let last_row = rows.saturating_sub(1);
        let last_col = cols.saturating_sub(1);
        match nav {
            Nav::Up => self.row = self.row.saturating_sub(1),
            Nav::Down => self.row = (self.row + 1).min(last_row),
            Nav::Left => self.col = self.col.saturating_sub(1),
            Nav::Right => self.col = (self.col + 1).min(last_col),
            Nav::Top => self.row = 0,
            Nav::Bottom => self.row = last_row,
            Nav::PageUp => self.row = self.row.saturating_sub(page),
            Nav::PageDown => self.row = (self.row + page).min(last_row),
        }
        // Re-clamp in case the grid shrank since the cursor last moved.
        self.row = self.row.min(last_row);
        self.col = self.col.min(last_col);
        self
    }

    /// Adjust `scroll` so the active row sits within the visible band
    /// `[scroll, scroll + viewport_rows)`, then clamp so the final page is
    /// never over-scrolled. A `viewport_rows` of 0 is a no-op.
    #[must_use]
    pub fn follow(mut self, viewport_rows: usize, rows: usize) -> Self {
        if viewport_rows == 0 {
            return self;
        }
        if self.row < self.scroll {
            self.scroll = self.row;
        } else if self.row >= self.scroll + viewport_rows {
            self.scroll = self.row + 1 - viewport_rows;
        }
        let max_scroll = rows.saturating_sub(viewport_rows);
        self.scroll = self.scroll.min(max_scroll);
        self
    }
}

/// Map a `KeyboardEvent.key` (plus the shift modifier) to a [`Nav`].
///
/// Returns `None` for keys the grid doesn't handle, so the caller can let them
/// propagate. Covers the arrow keys, vim `hjkl`, `g`/`G` and `Home`/`End` for
/// first/last row, and `PageUp`/`PageDown`. Shift is reserved for range
/// selection (a later milestone) and currently yields `None`.
pub fn nav_for_key(key: &str, shift: bool) -> Option<Nav> {
    if shift {
        return None;
    }
    Some(match key {
        "ArrowUp" | "k" => Nav::Up,
        "ArrowDown" | "j" => Nav::Down,
        "ArrowLeft" | "h" => Nav::Left,
        "ArrowRight" | "l" => Nav::Right,
        "Home" | "g" => Nav::Top,
        "End" | "G" => Nav::Bottom,
        "PageUp" => Nav::PageUp,
        "PageDown" => Nav::PageDown,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn down_increments_row_and_clamps_at_last() {
        let c = GridCursor::new().navigate(Nav::Down, 3, 2, 5);
        assert_eq!((c.row(), c.col()), (1, 0));
        // Already at the last row stays put.
        let c = c.at(2, 0, 3, 2).navigate(Nav::Down, 3, 2, 5);
        assert_eq!(c.row(), 2);
    }

    #[test]
    fn up_saturates_at_zero() {
        let c = GridCursor::new().navigate(Nav::Up, 3, 2, 5);
        assert_eq!(c.row(), 0);
    }

    #[test]
    fn left_right_clamp_to_columns() {
        let c = GridCursor::new()
            .navigate(Nav::Right, 3, 2, 5)
            .navigate(Nav::Right, 3, 2, 5); // clamp at col 1
        assert_eq!(c.col(), 1);
        let c = c.navigate(Nav::Left, 3, 2, 5).navigate(Nav::Left, 3, 2, 5);
        assert_eq!(c.col(), 0);
    }

    #[test]
    fn top_and_bottom_jump_to_ends() {
        let c = GridCursor::new()
            .at(5, 0, 100, 2)
            .navigate(Nav::Bottom, 100, 2, 10);
        assert_eq!(c.row(), 99);
        let c = c.navigate(Nav::Top, 100, 2, 10);
        assert_eq!(c.row(), 0);
    }

    #[test]
    fn page_moves_by_page_and_clamps() {
        let c = GridCursor::new().navigate(Nav::PageDown, 100, 2, 10);
        assert_eq!(c.row(), 10);
        let c = c.at(3, 0, 100, 2).navigate(Nav::PageUp, 100, 2, 10);
        assert_eq!(c.row(), 0); // 3 - 10 saturates
    }

    #[test]
    fn follow_scrolls_when_cursor_leaves_window() {
        // Cursor below window pushes scroll down.
        let c = GridCursor::new().at(20, 0, 100, 2).follow(10, 100);
        assert_eq!(c.scroll(), 11); // 20 + 1 - 10
        // Cursor above window pulls scroll up.
        let c = c.at(5, 0, 100, 2).follow(10, 100);
        assert_eq!(c.scroll(), 5);
        // Cursor inside window leaves scroll alone.
        let c = c.at(8, 0, 100, 2).follow(10, 100);
        assert_eq!(c.scroll(), 5);
    }

    #[test]
    fn follow_clamps_scroll_to_last_page() {
        let c = GridCursor::new().at(99, 0, 100, 2).follow(10, 100);
        // max_scroll = 100 - 10 = 90, not 99 + 1 - 10 = 90 — coincide here.
        assert_eq!(c.scroll(), 90);
        // Tiny dataset never scrolls.
        let c = GridCursor::new().at(2, 0, 3, 2).follow(10, 3);
        assert_eq!(c.scroll(), 0);
    }

    #[test]
    fn navigate_reclamps_when_grid_shrinks() {
        // Cursor was deep; grid is now tiny — a no-op move re-clamps it.
        let c = GridCursor::new().at(50, 5, 100, 8);
        let c = c.navigate(Nav::Up, 3, 2, 5);
        assert!(c.row() < 3 && c.col() < 2, "got {:?}", (c.row(), c.col()));
    }

    #[test]
    fn keymap_arrows_and_vim() {
        assert_eq!(nav_for_key("ArrowUp", false), Some(Nav::Up));
        assert_eq!(nav_for_key("k", false), Some(Nav::Up));
        assert_eq!(nav_for_key("ArrowDown", false), Some(Nav::Down));
        assert_eq!(nav_for_key("j", false), Some(Nav::Down));
        assert_eq!(nav_for_key("ArrowLeft", false), Some(Nav::Left));
        assert_eq!(nav_for_key("l", false), Some(Nav::Right));
        assert_eq!(nav_for_key("g", false), Some(Nav::Top));
        assert_eq!(nav_for_key("G", false), Some(Nav::Bottom));
        assert_eq!(nav_for_key("Home", false), Some(Nav::Top));
        assert_eq!(nav_for_key("End", false), Some(Nav::Bottom));
        assert_eq!(nav_for_key("PageDown", false), Some(Nav::PageDown));
    }

    #[test]
    fn keymap_ignores_unknown_and_shift() {
        assert_eq!(nav_for_key("a", false), None);
        assert_eq!(nav_for_key("Enter", false), None);
        // Shift is reserved for range selection.
        assert_eq!(nav_for_key("ArrowDown", true), None);
    }
}
