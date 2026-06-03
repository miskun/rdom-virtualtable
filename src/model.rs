//! The DOM-free table model: columns + row data + sort/reorder/window math.
//!
//! [`VirtualTable`] holds no DOM state and has no rdom-tui dependency in its
//! logic — it's the unit-tested core that [`VirtualTableView`](crate::VirtualTableView)
//! materializes a window of into a `<table>` subtree.

use std::collections::HashSet;

/// A table column: a header label and an optional fixed width (otherwise
/// the column auto-sizes to its widest cell).
#[derive(Clone, Debug)]
pub struct Column {
    pub header: String,
    pub width: Option<u16>,
}

impl Column {
    pub fn new(header: impl Into<String>) -> Self {
        Self {
            header: header.into(),
            width: None,
        }
    }

    pub fn with_width(mut self, width: u16) -> Self {
        self.width = Some(width);
        self
    }
}

/// Sort direction for [`VirtualTable::sort_by`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    Ascending,
    Descending,
}

impl SortDir {
    /// The opposite direction — handy for toggling a header.
    pub fn flipped(self) -> Self {
        match self {
            SortDir::Ascending => SortDir::Descending,
            SortDir::Descending => SortDir::Ascending,
        }
    }
}

/// Default cell comparator: numeric when *both* cells parse as numbers
/// (so `"2" < "10"`), lexicographic otherwise. Override per-sort with
/// [`VirtualTable::sort_by_with`].
fn default_cell_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.cmp(b),
    }
}

/// The cell text at `col`, or `""` if the row is short.
fn cell_at(row: &[String], col: usize) -> &str {
    row.get(col).map(String::as_str).unwrap_or("")
}

/// The table model: columns + row data. Holds no DOM state.
pub struct VirtualTable {
    columns: Vec<Column>,
    rows: Vec<Vec<String>>,
    /// Current sort `(column, direction)`, or `None` if unsorted.
    sort: Option<(usize, SortDir)>,
    /// Column indices currently hidden from display.
    hidden: HashSet<usize>,
}

impl VirtualTable {
    pub fn new(columns: Vec<Column>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
            sort: None,
            hidden: HashSet::new(),
        }
    }

    pub fn set_rows(&mut self, rows: Vec<Vec<String>>) {
        self.rows = rows;
    }

    pub fn push_row(&mut self, row: Vec<String>) {
        self.rows.push(row);
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// Read-only view of the row data (in current sort order).
    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    /// Current sort `(column, direction)`, or `None` if unsorted.
    pub fn sort_state(&self) -> Option<(usize, SortDir)> {
        self.sort
    }

    /// Hide or show the column at index `col` (no-op for out-of-range `col`,
    /// but the index is still tracked so a later `set_rows`/reorder is
    /// consistent). Reflected by the view as a `display: none` cell attribute.
    pub fn set_column_hidden(&mut self, col: usize, hidden: bool) {
        if hidden {
            self.hidden.insert(col);
        } else {
            self.hidden.remove(&col);
        }
    }

    /// Whether the column at `col` is currently hidden.
    pub fn is_column_hidden(&self, col: usize) -> bool {
        self.hidden.contains(&col)
    }

    /// Sort the rows by `col` using the [`default_cell_cmp`] comparator
    /// (numeric-aware, else lexicographic). Stable: equal keys keep their
    /// prior order. Records the sort so [`sort_state`](Self::sort_state)
    /// reflects it. Out-of-range `col` sorts by the empty string (a no-op
    /// ordering) and still records the state.
    pub fn sort_by(&mut self, col: usize, dir: SortDir) {
        self.sort_by_with(col, dir, default_cell_cmp);
    }

    /// Like [`sort_by`](Self::sort_by) but with a custom cell comparator —
    /// the sort hook. `cmp(a, b)` compares the two cells' text in column
    /// `col`; `dir` reverses it for descending.
    pub fn sort_by_with(
        &mut self,
        col: usize,
        dir: SortDir,
        cmp: impl Fn(&str, &str) -> std::cmp::Ordering,
    ) {
        self.rows.sort_by(|a, b| {
            let ord = cmp(cell_at(a, col), cell_at(b, col));
            match dir {
                SortDir::Ascending => ord,
                SortDir::Descending => ord.reverse(),
            }
        });
        self.sort = Some((col, dir));
    }

    /// Move the column at `from` to index `to`, permuting the header and
    /// **every row's cell** by the same amount so the model stays consistent.
    /// The recorded sort column (if any) is remapped so the sort follows its
    /// column. No-op for out-of-range or equal indices.
    pub fn move_column(&mut self, from: usize, to: usize) {
        let n = self.columns.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let col = self.columns.remove(from);
        self.columns.insert(to, col);
        for row in &mut self.rows {
            if from < row.len() {
                let cell = row.remove(from);
                row.insert(to.min(row.len()), cell);
            }
        }
        if let Some((c, dir)) = self.sort {
            self.sort = Some((Self::remapped_index(from, to, c), dir));
        }
        if !self.hidden.is_empty() {
            self.hidden = self
                .hidden
                .iter()
                .map(|&c| Self::remapped_index(from, to, c))
                .collect();
        }
    }

    /// Where index `i` lands after [`move_column(from, to)`](Self::move_column)
    /// — pure, so the view can remap the cursor column the same way. `from`
    /// maps to `to`; indices between shift by one toward the vacated slot;
    /// everything outside `[from, to]` is unchanged.
    pub fn remapped_index(from: usize, to: usize, i: usize) -> usize {
        if i == from {
            to
        } else if from < to {
            if i > from && i <= to { i - 1 } else { i }
        } else if i >= to && i < from {
            i + 1
        } else {
            i
        }
    }

    /// Compute the row window to materialize: `(start, count)` for a
    /// viewport that can show `viewport_rows` data rows, scrolled so the
    /// top visible row is `scroll_y`. Pure — the unit of testing for the
    /// virtualization math.
    pub fn window_for(viewport_rows: u16, scroll_y: usize, total: usize) -> (usize, usize) {
        let start = scroll_y.min(total);
        let count = (viewport_rows as usize).min(total - start);
        (start, count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_at_top() {
        assert_eq!(VirtualTable::window_for(10, 0, 100), (0, 10));
    }

    #[test]
    fn window_near_end_clamps_count() {
        assert_eq!(VirtualTable::window_for(10, 95, 100), (95, 5));
    }

    #[test]
    fn window_past_end_is_empty() {
        assert_eq!(VirtualTable::window_for(10, 200, 100), (100, 0));
    }

    #[test]
    fn window_smaller_dataset_than_viewport() {
        assert_eq!(VirtualTable::window_for(50, 0, 7), (0, 7));
    }

    #[test]
    fn model_row_bookkeeping() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b")]);
        assert!(t.is_empty());
        t.push_row(vec!["1".into(), "2".into()]);
        t.set_rows(vec![
            vec!["x".into(), "y".into()],
            vec!["p".into(), "q".into()],
        ]);
        assert_eq!(t.row_count(), 2);
        assert_eq!(t.columns().len(), 2);
    }

    fn col0(t: &VirtualTable) -> Vec<&str> {
        t.rows().iter().map(|r| r[0].as_str()).collect()
    }

    #[test]
    fn sort_by_orders_rows_both_directions_and_records_state() {
        let mut t = VirtualTable::new(vec![Column::new("a")]);
        t.set_rows(vec![
            vec!["banana".into()],
            vec!["apple".into()],
            vec!["cherry".into()],
        ]);
        t.sort_by(0, SortDir::Ascending);
        assert_eq!(col0(&t), vec!["apple", "banana", "cherry"]);
        assert_eq!(t.sort_state(), Some((0, SortDir::Ascending)));
        t.sort_by(0, SortDir::Descending);
        assert_eq!(col0(&t), vec!["cherry", "banana", "apple"]);
        assert_eq!(t.sort_state(), Some((0, SortDir::Descending)));
    }

    #[test]
    fn sort_is_numeric_aware() {
        // Lexical sort would give 1, 10, 2; numeric gives 1, 2, 10.
        let mut t = VirtualTable::new(vec![Column::new("n")]);
        t.set_rows(vec![vec!["10".into()], vec!["2".into()], vec!["1".into()]]);
        t.sort_by(0, SortDir::Ascending);
        assert_eq!(col0(&t), vec!["1", "2", "10"]);
    }

    #[test]
    fn sort_is_stable_for_equal_keys() {
        // Equal sort keys keep their original relative order (stable sort).
        let mut t = VirtualTable::new(vec![Column::new("k"), Column::new("id")]);
        t.set_rows(vec![
            vec!["x".into(), "first".into()],
            vec!["x".into(), "second".into()],
            vec!["a".into(), "third".into()],
        ]);
        t.sort_by(0, SortDir::Ascending);
        let ids: Vec<&str> = t.rows().iter().map(|r| r[1].as_str()).collect();
        assert_eq!(ids, vec!["third", "first", "second"]);
    }

    #[test]
    fn sort_by_with_uses_a_custom_comparator() {
        let mut t = VirtualTable::new(vec![Column::new("a")]);
        t.set_rows(vec![
            vec!["bb".into()],
            vec!["a".into()],
            vec!["ccc".into()],
        ]);
        t.sort_by_with(0, SortDir::Ascending, |x, y| x.len().cmp(&y.len()));
        assert_eq!(col0(&t), vec!["a", "bb", "ccc"]);
        assert_eq!(t.sort_state(), Some((0, SortDir::Ascending)));
    }

    fn headers(t: &VirtualTable) -> Vec<&str> {
        t.columns().iter().map(|c| c.header.as_str()).collect()
    }

    #[test]
    fn move_column_permutes_columns_and_every_row() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b"), Column::new("c")]);
        t.set_rows(vec![
            vec!["a0".into(), "b0".into(), "c0".into()],
            vec!["a1".into(), "b1".into(), "c1".into()],
        ]);
        t.move_column(0, 2); // a → end: [b, c, a]
        assert_eq!(headers(&t), ["b", "c", "a"]);
        assert_eq!(t.rows()[0], ["b0", "c0", "a0"]);
        assert_eq!(t.rows()[1], ["b1", "c1", "a1"]);

        t.move_column(2, 0); // a → front: [a, b, c]
        assert_eq!(headers(&t), ["a", "b", "c"]);
        assert_eq!(t.rows()[0], ["a0", "b0", "c0"]);
    }

    #[test]
    fn move_column_is_a_noop_for_invalid_or_equal_indices() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b")]);
        t.set_rows(vec![vec!["x".into(), "y".into()]]);
        t.move_column(0, 0); // equal
        t.move_column(0, 9); // out of range
        t.move_column(9, 0);
        assert_eq!(headers(&t), ["a", "b"]);
        assert_eq!(t.rows()[0], ["x", "y"]);
    }

    #[test]
    fn move_column_remaps_the_sort_column() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b"), Column::new("c")]);
        t.set_rows(vec![vec!["1".into(), "2".into(), "3".into()]]);
        t.sort_by(2, SortDir::Ascending); // sort column c (index 2)
        t.move_column(2, 0); // c moves to the front → sort follows to index 0
        assert_eq!(t.sort_state(), Some((0, SortDir::Ascending)));
    }

    #[test]
    fn remapped_index_tracks_a_move() {
        // move 0 → 2: 0↦2, 1↦0, 2↦1, 3 unchanged
        assert_eq!(VirtualTable::remapped_index(0, 2, 0), 2);
        assert_eq!(VirtualTable::remapped_index(0, 2, 1), 0);
        assert_eq!(VirtualTable::remapped_index(0, 2, 2), 1);
        assert_eq!(VirtualTable::remapped_index(0, 2, 3), 3);
        // move 2 → 0: 2↦0, 0↦1, 1↦2, 3 unchanged
        assert_eq!(VirtualTable::remapped_index(2, 0, 2), 0);
        assert_eq!(VirtualTable::remapped_index(2, 0, 0), 1);
        assert_eq!(VirtualTable::remapped_index(2, 0, 1), 2);
        assert_eq!(VirtualTable::remapped_index(2, 0, 3), 3);
    }

    #[test]
    fn hidden_columns_set_and_query() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b")]);
        assert!(!t.is_column_hidden(1));
        t.set_column_hidden(1, true);
        assert!(t.is_column_hidden(1));
        t.set_column_hidden(1, false);
        assert!(!t.is_column_hidden(1));
    }

    #[test]
    fn hidden_columns_follow_a_reorder() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b"), Column::new("c")]);
        t.set_column_hidden(0, true); // hide column a (index 0)
        t.move_column(0, 2); // a → index 2, so its hidden flag follows
        assert!(t.is_column_hidden(2), "hidden index follows its column");
        assert!(!t.is_column_hidden(0));
    }
}
