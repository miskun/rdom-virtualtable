//! The window buffer the renderer reads (`SPEC_DATA_SOURCE.md` §5).
//!
//! The table never materializes more than a *window* of rows. The
//! [`WindowBuffer`] holds exactly those rows — addressable by absolute index
//! and by [`RowKey`] — plus the known `total` (the full filtered result size,
//! which drives the scrollbar extent) and a monotonic **window epoch**.
//!
//! Two fillers write it:
//! - the in-memory [`VirtualTable`](crate::VirtualTable) (default), which copies
//!   its resident slice into the buffer on every `show_window`; and
//! - a windowed source (Observatory), which pushes `apply(epoch, Delta)` — added
//!   in P2; [`set_window`](WindowBuffer::set_window) is the `Resync` primitive it
//!   builds on.
//!
//! A slot the buffer doesn't have a row for is a **placeholder** (`None`): the
//! renderer paints a "loading" cell rather than a blank or a stale row. In the
//! in-memory mode the filler always covers the visible window, so placeholders
//! only appear in windowed mode when a scroll outruns the fetch.
//!
//! Pure data — no DOM. The epoch lives here but the *drop-stale-pushes* policy
//! that uses it lives at the push API (P2); this module only stores + bumps it.

use std::collections::HashMap;
use std::ops::Range;

use crate::data::{Row, RowKey};
use crate::model::SortDir;

/// A request from the table for the consumer to (re)fetch a window
/// (`SPEC_DATA_SOURCE.md` §5). Carries the `epoch` to echo back through
/// [`apply`](crate::VirtualTableView::apply) (so stale results drop), the
/// absolute `range` to fetch (the visible window plus a prefetch margin), and
/// the current `sort` the table owns. No filter — that's consumer-owned (N1);
/// the table only signals "things changed, re-request" via
/// [`invalidate`](crate::VirtualTableView::invalidate).
#[derive(Clone, Debug)]
pub struct WindowRequest {
    pub epoch: u64,
    pub range: Range<usize>,
    pub sort: Vec<SortSpec>,
}

/// One sort key in a [`WindowRequest`]: a column identified by its **header**
/// (stable across column reorder, unlike the positional index) and a direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortSpec {
    pub column: String,
    pub dir: SortDir,
}

/// The rows currently materializable, by index and by key, plus the total and
/// the window epoch. See the module docs.
#[derive(Clone, Debug, Default)]
pub struct WindowBuffer {
    /// First absolute index the buffer covers.
    start: usize,
    /// Rows for `start .. start + slots.len()`; `None` is a placeholder.
    slots: Vec<Option<Row>>,
    /// `RowKey` → absolute index, for in-place `Upsert`/`Remove` patching (P2)
    /// and identity → position lookups. Only keys of loaded (`Some`) slots.
    by_key: HashMap<RowKey, usize>,
    /// Total rows in the (filtered) result — drives the scrollbar extent. May
    /// exceed the buffered span (that's the whole point of windowing).
    total: usize,
    /// Monotonic window epoch; bumped whenever what-must-be-shown changes.
    epoch: u64,
}

impl WindowBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Total rows in the (filtered) result.
    pub fn total(&self) -> usize {
        self.total
    }

    /// Set the known total. Independent of the buffered span.
    pub fn set_total(&mut self, total: usize) {
        self.total = total;
    }

    /// The current window epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Bump the epoch and return the new value. The table calls this whenever
    /// the visible range / sort / an invalidate changes what must be shown; the
    /// consumer echoes the returned epoch back through `apply` (P2) so stale
    /// pushes can be dropped.
    pub fn bump_epoch(&mut self) -> u64 {
        self.epoch += 1;
        self.epoch
    }

    /// Number of buffered slots (loaded or placeholder).
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Replace the buffered window with `rows`, covering
    /// `start .. start + rows.len()`. This is the `Resync` primitive: it drops
    /// the old coverage entirely and rebuilds the key index. (The in-memory
    /// filler calls this every `show_window`; P2's `apply(_, Resync)` delegates
    /// here.)
    pub fn set_window(&mut self, start: usize, rows: Vec<Row>) {
        self.by_key.clear();
        for (i, row) in rows.iter().enumerate() {
            self.by_key.insert(row.key.clone(), start + i);
        }
        self.start = start;
        self.slots = rows.into_iter().map(Some).collect();
    }

    /// Patch a row in place by identity — the `Upsert` primitive. Replaces the
    /// slot of `row.key` if it's currently in the window; returns `false`
    /// (ignored) for a key not in the window (`SPEC_DATA_SOURCE.md` §5 N2 — a
    /// row enters via `Resync`, not `Upsert`). An `Upsert` before any `Resync`
    /// is therefore a no-op (the key index is empty).
    pub fn upsert(&mut self, row: Row) -> bool {
        let Some(&idx) = self.by_key.get(&row.key) else {
            return false;
        };
        let Some(i) = idx.checked_sub(self.start) else {
            return false;
        };
        self.by_key.insert(row.key.clone(), idx);
        self.slots[i] = Some(row);
        true
    }

    /// Drop a row by identity — the `Remove` primitive. Its slot becomes a
    /// placeholder (the row left the window; a following `Resync` refills the
    /// shifted window). Returns `false` if the key wasn't in the window.
    pub fn remove(&mut self, key: &RowKey) -> bool {
        let Some(idx) = self.by_key.remove(key) else {
            return false;
        };
        if let Some(i) = idx.checked_sub(self.start) {
            if let Some(slot) = self.slots.get_mut(i) {
                *slot = None;
            }
        }
        true
    }

    /// The row at absolute `index`, or `None` if the index is outside the
    /// buffered span or its slot is a placeholder.
    pub fn row_at(&self, index: usize) -> Option<&Row> {
        let i = index.checked_sub(self.start)?;
        self.slots.get(i)?.as_ref()
    }

    /// The identity at absolute `index` (loaded slots only).
    pub fn key_at(&self, index: usize) -> Option<&RowKey> {
        self.row_at(index).map(|r| &r.key)
    }

    /// Is `index` a loaded (non-placeholder) row within the buffered span?
    pub fn is_loaded(&self, index: usize) -> bool {
        self.row_at(index).is_some()
    }

    /// Clear the buffer (keeps the epoch + total). Used when a re-query is in
    /// flight and the old rows must not paint as current.
    pub fn clear_rows(&mut self) {
        self.slots.clear();
        self.by_key.clear();
        self.start = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::CellValue;

    fn row(key: &str, v: &str) -> Row {
        Row::new(key, vec![CellValue::from(v)])
    }

    #[test]
    fn set_window_indexes_by_absolute_position() {
        let mut b = WindowBuffer::new();
        b.set_window(10, vec![row("a", "A"), row("b", "B"), row("c", "C")]);
        assert_eq!(b.len(), 3);
        assert_eq!(b.row_at(10).map(|r| r.key.clone()), Some("a".into()));
        assert_eq!(b.row_at(12).map(|r| r.key.clone()), Some("c".into()));
        assert_eq!(b.key_at(11), Some(&"b".into()));
        assert_eq!(b.row_at(9), None, "below the start offset");
    }

    #[test]
    fn indices_outside_the_window_are_placeholders() {
        let mut b = WindowBuffer::new();
        b.set_window(5, vec![row("a", "A"), row("b", "B")]); // covers 5..7
        assert!(!b.is_loaded(4), "below the window");
        assert!(b.is_loaded(5));
        assert!(b.is_loaded(6));
        assert!(!b.is_loaded(7), "above the window");
        assert_eq!(b.row_at(4), None);
        assert_eq!(b.key_at(99), None);
    }

    #[test]
    fn set_window_replaces_old_coverage_and_reindexes() {
        let mut b = WindowBuffer::new();
        b.set_window(0, vec![row("a", "A"), row("b", "B")]);
        b.set_window(100, vec![row("x", "X")]);
        assert_eq!(b.len(), 1);
        assert_eq!(b.row_at(100).map(|r| r.key.clone()), Some("x".into()));
        assert!(!b.is_loaded(0), "old window no longer covered");
    }

    #[test]
    fn total_is_independent_of_buffered_span() {
        let mut b = WindowBuffer::new();
        b.set_total(100_000);
        b.set_window(0, vec![row("a", "A")]);
        assert_eq!(b.total(), 100_000);
        assert_eq!(b.len(), 1, "only one row buffered for a 100k total");
    }

    #[test]
    fn epoch_is_monotonic() {
        let mut b = WindowBuffer::new();
        assert_eq!(b.epoch(), 0);
        assert_eq!(b.bump_epoch(), 1);
        assert_eq!(b.bump_epoch(), 2);
        assert_eq!(b.epoch(), 2);
    }

    #[test]
    fn upsert_patches_in_place_and_ignores_unknown_keys() {
        let mut b = WindowBuffer::new();
        b.set_window(0, vec![row("a", "A"), row("b", "B")]);
        assert!(b.upsert(row("b", "B2")), "key in window → patched");
        assert_eq!(b.row_at(1).map(|r| r.cell(0).display()), Some("B2".into()));
        assert!(!b.upsert(row("z", "Z")), "key not in window → ignored (N2)");
        assert_eq!(b.len(), 2, "ignored upsert adds no slot");
    }

    #[test]
    fn upsert_before_resync_is_a_noop() {
        let mut b = WindowBuffer::new();
        assert!(!b.upsert(row("a", "A")), "no Resync yet → nothing to patch");
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn remove_makes_the_slot_a_placeholder() {
        let mut b = WindowBuffer::new();
        b.set_window(10, vec![row("a", "A"), row("b", "B"), row("c", "C")]);
        assert!(b.remove(&"b".into()));
        assert!(!b.is_loaded(11), "removed slot is now a placeholder");
        assert!(b.is_loaded(10), "neighbours untouched");
        assert!(b.is_loaded(12));
        assert!(!b.remove(&"b".into()), "second remove is a no-op");
    }

    #[test]
    fn clear_rows_keeps_total_and_epoch() {
        let mut b = WindowBuffer::new();
        b.set_total(50);
        b.bump_epoch();
        b.set_window(0, vec![row("a", "A")]);
        b.clear_rows();
        assert_eq!(b.len(), 0);
        assert_eq!(b.total(), 50, "total survives a row clear");
        assert_eq!(b.epoch(), 1, "epoch survives a row clear");
        assert!(!b.is_loaded(0), "no rows after clear");
    }
}
