//! The DOM-free table model: columns + row data + sort/reorder/window math.
//!
//! [`VirtualTable`] holds no DOM state and has no rdom-tui dependency in its
//! logic — it's the unit-tested core that [`VirtualTableView`](crate::VirtualTableView)
//! materializes a window of into a `<table>` subtree. Rows are [`Row`]s (a
//! [`RowKey`] + typed [`CellValue`] cells); this is the **in-memory convenience
//! mode** of the data layer — a windowed source feeds the view directly (via
//! `apply`) and never touches this type.

use std::cmp::Ordering;
use std::collections::HashSet;

use crate::data::{CellValue, Row, RowKey};

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

/// The table model: columns + row data. Holds no DOM state.
pub struct VirtualTable {
    columns: Vec<Column>,
    rows: Vec<Row>,
    /// Original insertion index of `rows[i]`, permuted alongside `rows` on every
    /// sort. Lets [`clear_sort`](Self::clear_sort) restore the as-inserted order
    /// (the "off" state of the asc → desc → off header-click cycle) without
    /// keeping a second copy of the row data.
    orig: Vec<u32>,
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
            orig: Vec::new(),
            sort: None,
            hidden: HashSet::new(),
        }
    }

    /// Replace all rows. Cells are anything that converts to [`CellValue`] (a
    /// bare `&str`/`String` becomes [`CellValue::Text`]). Each row is assigned a
    /// **synthetic stable [`RowKey`]** (its insertion index) that survives sort
    /// and filter, so an identity-keyed selection follows its row. Consumers with
    /// a real key use [`set_rows_keyed`](Self::set_rows_keyed).
    pub fn set_rows(&mut self, rows: Vec<Vec<CellValue>>) {
        self.orig = (0..rows.len() as u32).collect();
        self.rows = rows
            .into_iter()
            .enumerate()
            .map(|(i, cells)| Row::new(RowKey::from(i.to_string()), cells))
            .collect();
    }

    /// Replace all rows, each carrying a caller-supplied [`RowKey`]. For
    /// consumers that have a real identity (and want selection to survive a
    /// `set_rows_keyed` that reorders/replaces).
    pub fn set_rows_keyed(&mut self, rows: Vec<Row>) {
        self.orig = (0..rows.len() as u32).collect();
        self.rows = rows;
    }

    /// Append one row with a synthetic [`RowKey`]. The next original index is
    /// monotonic — a row pushed after a sort still restores to the end of the
    /// as-inserted order on `clear_sort`.
    pub fn push_row(&mut self, cells: Vec<CellValue>) {
        let id = self.orig.len() as u32;
        self.orig.push(id);
        self.rows
            .push(Row::new(RowKey::from(id.to_string()), cells));
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
    pub fn rows(&self) -> &[Row] {
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

    /// The currently-hidden columns as `(index, header label)` pairs, sorted by
    /// index. Out-of-range indices (recorded but past the column count) are
    /// skipped.
    pub fn hidden_columns(&self) -> Vec<(usize, &str)> {
        let mut indices: Vec<usize> = self
            .hidden
            .iter()
            .copied()
            .filter(|&i| i < self.columns.len())
            .collect();
        indices.sort_unstable();
        indices
            .into_iter()
            .map(|i| (i, self.columns[i].header.as_str()))
            .collect()
    }

    /// Set (or clear, with `None`) the explicit width of column `col`. Stored on
    /// the [`Column`], so it **follows the column through a reorder**. No-op for
    /// out-of-range `col`.
    pub fn set_column_width(&mut self, col: usize, width: Option<u16>) {
        if let Some(c) = self.columns.get_mut(col) {
            c.width = width;
        }
    }

    /// Sort the rows by `col` using the type-aware [`CellValue::sort_cmp`].
    /// Stable: equal keys keep their prior order. Records the sort. Out-of-range
    /// `col` compares `Empty` cells (a no-op ordering) and still records state.
    pub fn sort_by(&mut self, col: usize, dir: SortDir) {
        self.sort_by_with(col, dir, |a, b| a.sort_cmp(b));
    }

    /// Like [`sort_by`](Self::sort_by) but with a custom cell comparator — the
    /// sort hook. `cmp(a, b)` compares the two cells in column `col`; `dir`
    /// reverses it for descending.
    pub fn sort_by_with(
        &mut self,
        col: usize,
        dir: SortDir,
        cmp: impl Fn(&CellValue, &CellValue) -> Ordering,
    ) {
        // Pair each row with its original index so the permutation is tracked;
        // sort the pairs (stable), then split back out.
        let mut paired: Vec<(Row, u32)> = self.rows.drain(..).zip(self.orig.drain(..)).collect();
        paired.sort_by(|(a, _), (b, _)| {
            let ord = cmp(a.cell(col), b.cell(col));
            match dir {
                SortDir::Ascending => ord,
                SortDir::Descending => ord.reverse(),
            }
        });
        for (r, o) in paired {
            self.rows.push(r);
            self.orig.push(o);
        }
        self.sort = Some((col, dir));
    }

    /// Restore the as-inserted row order and clear the recorded sort — the
    /// "off" state of the asc → desc → off header-click cycle.
    pub fn clear_sort(&mut self) {
        let mut paired: Vec<(Row, u32)> = self.rows.drain(..).zip(self.orig.drain(..)).collect();
        paired.sort_by_key(|(_, o)| *o);
        for (r, o) in paired {
            self.rows.push(r);
            self.orig.push(o);
        }
        self.sort = None;
    }

    /// Move the column at `from` to index `to`, permuting the header and
    /// **every row's cell** by the same amount. The recorded sort column (and
    /// hidden flags) are remapped so they follow their column. No-op for
    /// out-of-range or equal indices.
    pub fn move_column(&mut self, from: usize, to: usize) {
        let n = self.columns.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let col = self.columns.remove(from);
        self.columns.insert(to, col);
        for row in &mut self.rows {
            if from < row.cells.len() {
                let cell = row.cells.remove(from);
                row.cells.insert(to.min(row.cells.len()), cell);
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
    /// — pure, so the view can remap the cursor column the same way.
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

    /// Compute the row window to materialize: `(start, count)` for a viewport
    /// that can show `viewport_rows` data rows, scrolled so the top visible row
    /// is `scroll_y`. Pure — the unit of testing for the virtualization math.
    pub fn window_for(viewport_rows: u16, scroll_y: usize, total: usize) -> (usize, usize) {
        let start = scroll_y.min(total);
        let count = (viewport_rows as usize).min(total - start);
        (start, count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Display strings of column 0 across the rows (in current order).
    fn col0(t: &VirtualTable) -> Vec<String> {
        t.rows().iter().map(|r| r.cell(0).display()).collect()
    }
    /// Display strings of a row's cells.
    fn cells(r: &Row) -> Vec<String> {
        r.cells.iter().map(|c| c.display()).collect()
    }
    fn headers(t: &VirtualTable) -> Vec<&str> {
        t.columns().iter().map(|c| c.header.as_str()).collect()
    }

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

    #[test]
    fn set_rows_assigns_stable_synthetic_keys() {
        let mut t = VirtualTable::new(vec![Column::new("a")]);
        t.set_rows(vec![vec!["b".into()], vec!["a".into()], vec!["c".into()]]);
        let keys_before: Vec<String> = t.rows().iter().map(|r| r.key.to_string()).collect();
        assert_eq!(keys_before, vec!["0", "1", "2"]);
        // A sort permutes rows but each keeps its key (selection can follow it).
        t.sort_by(0, SortDir::Ascending);
        let keyed: Vec<(String, String)> = t
            .rows()
            .iter()
            .map(|r| (r.cell(0).display(), r.key.to_string()))
            .collect();
        assert_eq!(
            keyed,
            vec![
                ("a".into(), "1".into()),
                ("b".into(), "0".into()),
                ("c".into(), "2".into())
            ]
        );
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
    fn clear_sort_restores_as_inserted_order() {
        let mut t = VirtualTable::new(vec![Column::new("a")]);
        t.set_rows(vec![
            vec!["banana".into()],
            vec!["apple".into()],
            vec!["cherry".into()],
        ]);
        t.sort_by(0, SortDir::Ascending);
        assert_eq!(col0(&t), vec!["apple", "banana", "cherry"]);
        t.clear_sort();
        assert_eq!(col0(&t), vec!["banana", "apple", "cherry"]);
        assert_eq!(t.sort_state(), None);
        t.sort_by(0, SortDir::Descending);
        assert_eq!(col0(&t), vec!["cherry", "banana", "apple"]);
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
        let mut t = VirtualTable::new(vec![Column::new("k"), Column::new("id")]);
        t.set_rows(vec![
            vec!["x".into(), "first".into()],
            vec!["x".into(), "second".into()],
            vec!["a".into(), "third".into()],
        ]);
        t.sort_by(0, SortDir::Ascending);
        let ids: Vec<String> = t.rows().iter().map(|r| r.cell(1).display()).collect();
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
        t.sort_by_with(0, SortDir::Ascending, |x, y| {
            x.display().len().cmp(&y.display().len())
        });
        assert_eq!(col0(&t), vec!["a", "bb", "ccc"]);
        assert_eq!(t.sort_state(), Some((0, SortDir::Ascending)));
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
        assert_eq!(cells(&t.rows()[0]), ["b0", "c0", "a0"]);
        assert_eq!(cells(&t.rows()[1]), ["b1", "c1", "a1"]);
        t.move_column(2, 0); // a → front: [a, b, c]
        assert_eq!(headers(&t), ["a", "b", "c"]);
        assert_eq!(cells(&t.rows()[0]), ["a0", "b0", "c0"]);
    }

    #[test]
    fn move_column_is_a_noop_for_invalid_or_equal_indices() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b")]);
        t.set_rows(vec![vec!["x".into(), "y".into()]]);
        t.move_column(0, 0);
        t.move_column(0, 9);
        t.move_column(9, 0);
        assert_eq!(headers(&t), ["a", "b"]);
        assert_eq!(cells(&t.rows()[0]), ["x", "y"]);
    }

    #[test]
    fn move_column_remaps_the_sort_column() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b"), Column::new("c")]);
        t.set_rows(vec![vec!["1".into(), "2".into(), "3".into()]]);
        t.sort_by(2, SortDir::Ascending);
        t.move_column(2, 0);
        assert_eq!(t.sort_state(), Some((0, SortDir::Ascending)));
    }

    #[test]
    fn remapped_index_tracks_a_move() {
        assert_eq!(VirtualTable::remapped_index(0, 2, 0), 2);
        assert_eq!(VirtualTable::remapped_index(0, 2, 1), 0);
        assert_eq!(VirtualTable::remapped_index(0, 2, 2), 1);
        assert_eq!(VirtualTable::remapped_index(0, 2, 3), 3);
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
        t.set_column_hidden(0, true);
        t.move_column(0, 2);
        assert!(t.is_column_hidden(2), "hidden index follows its column");
        assert!(!t.is_column_hidden(0));
    }

    #[test]
    fn hidden_columns_lists_index_and_label_sorted() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b"), Column::new("c")]);
        assert!(t.hidden_columns().is_empty());
        t.set_column_hidden(2, true);
        t.set_column_hidden(0, true);
        assert_eq!(t.hidden_columns(), vec![(0, "a"), (2, "c")]);
    }

    #[test]
    fn hidden_columns_skips_out_of_range_indices() {
        let mut t = VirtualTable::new(vec![Column::new("a")]);
        t.set_column_hidden(9, true);
        assert!(t.hidden_columns().is_empty());
    }

    #[test]
    fn column_width_follows_a_reorder() {
        let mut t = VirtualTable::new(vec![Column::new("a"), Column::new("b"), Column::new("c")]);
        t.set_column_width(1, Some(20));
        assert_eq!(t.columns()[1].width, Some(20));
        t.move_column(1, 0);
        assert_eq!(t.columns()[0].width, Some(20), "width follows the column");
        assert_eq!(t.columns()[1].width, None);
    }
}
