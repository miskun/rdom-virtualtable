# SPEC — windowed, live data source for `rdom-virtualtable`

**Status:** draft **v2** (2026-06-06). v2 folds in the grumpy-architect + grumpy-API review (the three blockers B1–B3 + the non-blockers). Ready to start P1.

## 1. Context & the decision that shapes everything

`rdom-virtualtable` today is **display-only** virtualization over an **all-resident** model (`VirtualTable { rows: Vec<Vec<String>> }`): the DOM materializes only the visible window, but every row is in memory and addressed **positionally**. That's fine for small/static data; it cannot serve the target use case.

**Target use case** (the redesigned lens TUI): up to ~100k rows, **sorted/filtered/windowed**, **live-updating in real time**, where fetching all rows or re-fetching per-row data repeatedly would hammer the upstream API. The backend is **Observatory** — SQL + live subscriptions over a warm Arrow cache.

**The load-bearing decision:** *Observatory is the data engine.* It already provides, server-side over the warm cache:

- `query(Query { filter, sort, range, projection }) → Arrow RecordBatch` — windowed, filtered, sorted, sliced (sub-ms warm).
- `subscribe(Query) → Stream<Delta>` — `Resync { rows }` (sorted snapshot) then `Upsert { keys, rows }` / `Remove { keys }`.
- Total count via a count query; row identity via `schema.primary_key`.

So we **do not rebuild a data engine in the TUI** (no client-side cache+proxy, no `TableIndex`, no client sort/filter over 100k, no prefetch/dedup machinery). That would re-solve, slowly and client-side, what Observatory solves server-side. lens-k8s-tui's `lens-table` engine existed *only* because raw kube has no SQL/sort/window — Observatory removes that reason.

**Two unknowns, resolved as project decisions:**

1. **Observatory maintains the windowed-sorted-filtered live view** and emits the `Upsert`/`Remove` that keep the visible window correct (rows entering/leaving/moving under the active sort). → The table just **applies deltas by key**; it never maintains window membership or ordering itself.
2. **"Side-loaded" columns are just columns.** Observatory folds expensive per-row data (e.g. metrics) into the query result. → **No `SideLoadState` / debounce / LRU machinery** in the table. Out of scope entirely.

## 2. Non-goals (explicit anti-scope)

- No data engine, cache, or index in `rdom-virtualtable`.
- No client-side sort/filter *evaluation* over the full dataset (sort/filter are *requested*; Observatory executes them). Client-side sort stays only for the in-memory convenience mode (§7).
- No `arrow`, `tokio`, or `observatory` dependency in `rdom-virtualtable`. The component stays generic and headless-testable.
- No side-load subsystem.
- No async inside the table crate (rdom-tui handlers are sync).

## 3. Architecture & crate boundaries

```
Observatory (async engine, Arrow)         ← owns query/subscribe/sort/filter/window/total/live
        │  query() → RecordBatch ;  subscribe() → Stream<Delta>
        ▼
Consumer adapter (in the lens TUI app, OR a thin `rdom-virtualtable-observatory` crate)
   - runs Observatory on a tokio runtime (background)
   - maps Arrow RecordBatch → Vec<Row> (Arrow DataType → CellValue)
   - bridges to the UI thread via rdom-tui's AppHandle inject queue
        │  view.apply(epoch, Delta) / set_total(..)   (echoing the epoch it was handed)
        ▼
rdom-virtualtable (GENERIC, sync, no arrow/tokio/observatory dep)
   - RowKey, CellValue, Row, Delta            (data model)
   - a sync "window buffer" the renderer reads, guarded by a window epoch
   - on_window_change(WindowRequest) callback (table requests) + invalidate()
   - rendering/interaction we already shipped: selection, sort UI, column ops,
     drag-autoscroll, right-pinned actions  — cursor by index, selection by RowKey
        │
        ▼
rdom-tui (substrate)
```

**Why generic:** `rdom-virtualtable` is to a data backend what `rdom-tui` is to an app — a substrate. Binding it to Observatory would make it single-purpose and couple a UI component to Arrow. The Observatory adapter is small and belongs with the consumer (or a clearly-separate binding crate), never in the component.

## 4. Data model (in `rdom-virtualtable`)

```rust
/// Opaque, cheap-to-clone stable row identity. The consumer constructs it from
/// the source's primary key (e.g. Observatory `schema.primary_key` →
/// "_source_id\u{1}namespace\u{1}name"). The table treats it as opaque.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RowKey(Arc<str>);

impl From<&str> for RowKey { /* ... */ }
impl From<String> for RowKey { /* ... */ }

/// A typed cell value. `Text` is the default/fallback. Starter set; extend as
/// real columns demand (Progress/Badge/Link/etc. are deferred). Type drives
/// rich rendering AND in-memory-mode sort comparison.
#[derive(Clone)]
pub enum CellValue {
    Empty,
    Text(String),
    Number(f64),                    // unit-less; formatting is the renderer's job
    Bytes(u64),
    Duration(std::time::Duration),  // e.g. Age
    Status { text: String, level: StatusLevel },
}
pub enum StatusLevel { Ok, Warn, Error, Info }

// N4 ergonomics: a bare string is a Text cell, so existing call sites survive.
impl From<&str> for CellValue { /* -> Text */ }
impl From<String> for CellValue { /* -> Text */ }

/// One row: identity + cells in the table's column order.
#[derive(Clone)]
pub struct Row {
    pub key: RowKey,
    pub cells: Vec<CellValue>,
}

/// A change to the windowed view — mirrors Observatory's `Delta` 1:1 (N5), so
/// the adapter is a straight map. Every variant carries the `epoch` it was
/// produced for (B1).
pub enum Delta {
    /// Full snapshot for `start..start+rows.len()` (Observatory `Resync`).
    Resync { start: usize, rows: Vec<Row> },
    /// Rows changed/entered the window — replace/insert by `RowKey`.
    Upsert { rows: Vec<Row> },
    /// Rows left the window — drop by `RowKey`.
    Remove { keys: Vec<RowKey> },
}
```

## 5. The table data API (push model, sync, epoch-guarded)

The table owns a **window buffer**: the rows for the currently requested range, addressable by position and by `RowKey`, plus the known total. The consumer fills it; the table renders from it and asks for more when the window moves.

```rust
impl VirtualTableView {
    /// Total rows in the (filtered) result — drives the scrollbar extent.
    /// Clamps the scroll position if `total` shrank below it.
    pub fn set_total(&self, dom: &mut TuiDom, total: usize);

    /// Apply a delta for window `epoch`. **Pushes whose epoch ≠ the current
    /// window epoch are dropped silently** (B1) — this is what makes
    /// out-of-order async results and late deltas from a torn-down subscription
    /// safe. `Resync` replaces the buffer for its range; `Upsert`/`Remove` patch
    /// by `RowKey`. `Upsert` for a key not in the current window is ignored, and
    /// any apply before the first `Resync` of an epoch is a no-op (N2).
    /// Marks the view dirty; the App's `draw_if_dirty` coalesces the paint —
    /// N applies in one tick repaint once (N6).
    pub fn apply(&self, dom: &mut TuiDom, epoch: u64, delta: Delta);

    /// Registered once. The table calls it whenever the visible range, sort, or
    /// an `invalidate()` changes what must be shown — carrying a fresh `epoch`.
    /// The consumer re-queries + re-subscribes Observatory for `range`/`sort`
    /// and pushes `apply(epoch, …)` back, echoing the epoch. Debounced by the
    /// table on viewport-settle so a fast scroll fires one request, not one per
    /// row.
    pub fn on_window_change(&self, cb: impl FnMut(WindowRequest) + 'static);

    /// Bump the epoch + re-fire `on_window_change` for the current range. The
    /// consumer calls this when *its* filter changes (the table has no filter
    /// UI — N1). Also the hook for "refresh now".
    pub fn invalidate(&self, dom: &mut TuiDom);
}

pub struct WindowRequest {
    pub epoch: u64,            // echo this back in `apply` (B1)
    pub range: Range<usize>,   // visible window + prefetch margin (table-owned policy)
    pub sort: Vec<SortSpec>,   // table owns the sort UI → it sends the sort
    // NO filter here (N1): filter is consumer-owned; the consumer already knows
    // it and applies it to its Observatory query. The table only learns "things
    // changed, re-request" via `invalidate()`.
}

pub struct SortSpec { pub column: ColumnKey, pub dir: SortDir }
```

- **Epoch (B1):** the table holds a monotonic `window_epoch`, bumped on every `on_window_change` (scroll-settle, sort change, `invalidate`). The consumer threads the handed `epoch` back through `apply`. The table ignores any `apply` whose epoch is not current. One concept closes all four races (stale `set_window` clobber, out-of-order results, late old-subscription delta, Resync/Upsert interleave).
- **Placeholders:** positions in the visible range without a row render a "loading" shimmer cell style — never blank, never stalled.
- **Prefetch margin:** the table requests a range slightly larger than visible (policy, e.g. ±50%) so adjacent scroll is shimmer-free; Observatory's warm cache makes the extra cheap.

## 6. Delta & window semantics (the contract with Observatory)

Because Observatory maintains the live windowed-sorted-filtered view:

- A subscription is **per visible window** (`range` = requested window+margin, plus `sort`/filter). Its first event is `Resync` → `apply(epoch, Resync{start, rows})`.
- As upstream changes, Observatory emits `Upsert`/`Remove` **that keep the window correct**: a row whose sort key moves it out of the window arrives as `Remove`; its replacement as `Upsert`; an in-place change as `Upsert` for the same key. The table applies them verbatim by `RowKey`; it never recomputes ordering or membership.
- `total` changes arrive via `set_total` (consumer derives from a count query/subscription).
- **On window change** the consumer drops the old subscription and opens a new one for the new range; the **epoch guard** makes any straggler deltas from the old one harmless. Warm-cache `Resync` is sub-ms.

The table is a **pure projection of "current window + deltas for the current epoch."**

## 7. In-memory convenience mode (kept, with honest limits)

The existing in-memory `VirtualTable` stays as a **built-in window filler**: `set_rows(..)` (now `Vec<Vec<CellValue>>`, with `&str`/`String` → `Text` so call sites survive), client-side sort/filter, and the table fills its own buffer + bumps the epoch internally on scroll/sort. Default for the `scroll_table` example, tests, and simple apps.

- **RowKey assignment (N4):** in-memory rows have no natural key, so the filler assigns a **stable synthetic key per row at `set_rows`** (a monotonic id, like today's `orig`), surviving sort/filter. Consumers with a real key can supply one.

### Capability matrix (N3 — they are rendering peers, not capability peers)

| Capability | In-memory mode | Windowed (Observatory) mode |
|---|---|---|
| Render / virtualize / drag-autoscroll | ✅ | ✅ |
| Sort / filter | client-side (over `CellValue`) | **requested** → Observatory executes |
| Live updates | via `set_rows`/upsert helpers | `apply(Delta)` stream |
| Select-all | enumerable key set | **predicate** ("all matching", §8) |
| Cursor nav past loaded data | always present | requests window, placeholder until it lands |

## 8. Identity: cursor = position, selection = identity

The grumpy review's B2/B3 resolution. **Two different concepts, deliberately separated:**

- **Cursor = absolute index** in the current view. It drives keyboard nav, scroll math, and the `SCROLL-SINGLE-OWNER` reveal. It *exposes* the `RowKey` of the row currently at that index **when that row is loaded** (for "act on the cursored row"). On a live re-sort the cursor **stays at its index** (predictable for keyboard users; chasing a resource that may leave the window is impossible anyway). Nav past the buffered range → the table requests that window and shows a placeholder; the cursor highlights once the row arrives. (B3)
- **Selection = identity**, and has two forms (B2):
  - an explicit `HashSet<RowKey>` (click / Shift-range / Ctrl-toggle, over loaded rows), which **survives scroll, re-sort, and live updates** — a selected pod stays selected as its row moves; and
  - a **predicate mode** for `Ctrl-A`: `all: bool` + `except: HashSet<RowKey>` = "everything matching the current filter, minus these." This is the only sane "select all" over a windowed 100k set. Bulk actions consult the predicate + ask the source (the consumer) to enumerate server-side.

`GridSelection` grows to `{ explicit: HashSet<RowKey>, all: bool, except: HashSet<RowKey> }`; `is_selected(key)` = `all && !except.contains(key) || explicit.contains(key)`. Works identically for both modes.

The interaction layer above this line (drag-autoscroll, sort UI, column ops, right-pinned actions) is unchanged — it just reads cursor-by-index and selection-by-key.

## 9. Async boundary

Observatory is async (tokio: `query().await`, `subscribe → Stream`); rdom-tui handlers are sync. The **consumer** runs Observatory on a runtime and bridges results to the table via rdom-tui's **`AppHandle` cross-thread inject queue** (already exists, already used for this class of thing). The table crate has **no async**: every `apply`/`set_total` call happens on the UI thread inside an injected callback. Fully testable headless with synchronous pushes.

## 10. Migration from the current model

1. `CellValue` replaces `String` cells (breaking; pre-1.0 OK; `&str`/`String → Text` softens it).
2. `RowKey` introduced; cursor stays index-based, **selection re-keyed to `RowKey`** + predicate mode.
3. Window buffer + epoch introduced; `show_window` reads the buffer; in-memory `VirtualTable` becomes a filler.
4. `apply(Delta)` / `set_total` / `on_window_change` / `invalidate` added.
5. Tests migrated (string cells → `CellValue::Text`; positional selection asserts → `RowKey`).

## 11. Phased plan

- **P0** — this spec → review gate. **Done (v2 incorporates the gate findings).**
- **P1** — `RowKey`, `CellValue`, `Row`, `Delta`; the window buffer + epoch + placeholder rendering; in-memory filler parity (all current tests green on the new model, `&str`→`Text` shims). Selection → `{explicit, all, except}`; cursor index-based with `RowKey` exposure.
- **P2** — push API (`apply`/`set_total`) + `on_window_change` (debounced, epoch-stamped) + `invalidate` + prefetch margin + **per-tick delta coalescing** (moved up from P4 per N6).
- **P3** — reference Observatory adapter (lens TUI or `rdom-virtualtable-observatory`): Arrow→`CellValue`, `RowKey` from PK, query/subscribe → `apply`, `AppHandle` bridge. End-to-end against Observatory fixtures.
- **P4** — persistence callbacks (sort/order/width/hidden) for the consumer to save UI state. **Done:** `TableState` / `ColumnState` (header-keyed), `table_state()` snapshot, `on_state_change(cb)` fired on every layout mutation (sort/clear_sort/reorder/width/hide), and `restore_state(dom, &state)` re-applying a saved layout with the callback suppressed.

## 12. Open questions / risks (remaining after v2)

- **`RowKey` representation** — single opaque `Arc<str>` (current choice) vs composite. `Arc<str>` chosen for O(1) clone/hash; composite only if a consumer needs structured keys.
- **`CellValue` starter set** — confirm Text/Number/Bytes/Duration/Status covers the lens columns; defer Progress/Badge/Link until a column needs them.
- **Prefetch margin policy** — fixed ±N vs ±fraction; tune against Observatory latency in P3.
- **Count freshness** — count query cadence vs a count subscription; how stale the scrollbar thumb may be between updates.
- **Re-subscribe churn on scroll** — confirm Observatory is happy with frequent per-window re-subscribes (warm cache says yes; validate in P3). The epoch guard already makes correctness independent of this.

## 13. Review gate — CLEARED (v2)

v1 ran a grumpy-architect + grumpy-API pass. Blockers resolved in v2:

- **B1 (epoch token)** — §5: every `apply` carries the window `epoch`; stale epochs dropped. Closes the out-of-order / late-delta / Resync-interleave races.
- **B2 (windowed selection)** — §8: selection = explicit key set **or** predicate (`all` + `except`); `Ctrl-A` is predicate-based.
- **B3 (cursor model)** — §8: cursor = absolute index (nav/scroll), exposes `RowKey` when loaded; selection = identity. Removes the "RowKey + derived index" contradiction.

Non-blockers folded in: N1 (filter dropped from `WindowRequest`; consumer-owned + `invalidate`), N2 (apply-before-Resync / unknown-key = no-op), N3 (capability matrix), N4 (`From<&str>/String`; in-memory `RowKey` assignment), N5 (unified `apply(Delta)` mirroring Observatory), N6 (apply marks dirty + coalesces; real-time coalescing moved to P2).
