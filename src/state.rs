//! Persistable table UI state: a snapshot of the column layout (order, widths,
//! hidden) + the active sort, keyed by column **header** so it survives reorders
//! and round-trips across sessions.
//!
//! The table emits a [`TableState`] whenever the layout changes (via
//! [`on_state_change`](crate::VirtualTableView::on_state_change)) so a consumer
//! can persist it, and re-applies one with
//! [`restore_state`](crate::VirtualTableView::restore_state) on the next launch.
//!
//! Plain data — no `serde` dependency in-crate. The fields are public, so a
//! consumer serializes them however it likes (map to its own `#[derive(Serialize)]`
//! shape, or enable a feature downstream).

use crate::window::SortSpec;

/// One column's persistable layout: its identity (`header`), explicit `width`
/// (`None` = content-auto), and whether it's `hidden`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnState {
    pub header: String,
    pub width: Option<u16>,
    pub hidden: bool,
}

/// A snapshot of the table's persistable UI state: the columns in **display
/// order**, each with its width + hidden flag, plus the active sort (keyed by
/// header). Produced by [`table_state`](crate::VirtualTableView::table_state)
/// and consumed by [`restore_state`](crate::VirtualTableView::restore_state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableState {
    pub columns: Vec<ColumnState>,
    pub sort: Option<SortSpec>,
}
