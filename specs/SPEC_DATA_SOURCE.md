# SPEC — windowed, live data source for `rdom-virtualtable`

**Status:** draft v1 (2026-06-06). Contract-first; no code until this passes a grumpy-architect + grumpy-API review gate (see end).

## 1. Context & the decision that shapes everything

`rdom-virtualtable` today is **display-only** virtualization over an **all-resident** model (`VirtualTable { rows: Vec<Vec<String>> }`): the DOM materializes only the visible window, but every row is in memory and addressed **positionally**. That's fine for small/static data; it cannot serve the target use case.

**Target use case** (the redesigned lens TUI): up to ~100k rows, **sorted/filtered/windowed**, **live-updating in real time**, where fetching all rows or re-fetching per-row data repeatedly would hammer the upstream API. The backend is **Observatory** — SQL + live subscriptions over a warm Arrow cache.

**The load-bearing decision:** *Observatory is the data engine.* It already provides, server-side over the warm cache:

- `query(Query { filter, sort, range, projection }) → Arrow RecordBatch` — windowed, filtered, sorted, sliced (sub-ms warm).
- `subscribe(Query) → Stream<Delta>` — `Resync { rows }` (sorted snapshot) then `Upsert { keys, rows }` / `Remove { keys }`.
- Total count via a count query; row identity via `schema.primary_key`.

So we **do not rebuild a data engine in the TUI** (no client-side cache+proxy, no `TableIndex`, no client sort/filter over 100k, no prefetch/dedup machinery). That would re-solve, slowly and client-side, what Observatory solves server-side. lens-k8s-tui's `lens-table` engine existed *only* because raw kube has no SQL/sort/window — Observatory removes that reason.

**Two unknowns, now resolved (project decisions):**

1. **Observatory maintains the windowed-sorted-filtered live view** and emits the `Upsert`/`Remove` that keep the visible window correct (rows entering/leaving/moving under the active sort). → The table just **applies deltas by key**; it never maintains window membership itself.
2. **"Side-loaded" columns are just columns.** Observatory folds expensive per-row data (e.g. metrics) into the query result. → **No `SideLoadState` / debounce / LRU machinery** in the table. Out of scope entirely.

## 2. Non-goals (explicit anti-scope)

- No data engine, cache, or index in `rdom-virtualtable`.
- No client-side sort or filter *evaluation* over the full dataset (sort/filter are *requested*; Observatory executes them). Client-side sort stays only for the in-memory convenience mode (§7).
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
        │  view.set_window(..) / apply_upsert(..) / apply_remove(..) / set_total(..)
        ▼
rdom-virtualtable (GENERIC, sync, no arrow/tokio/observatory dep)
   - RowKey, CellValue, Row  (data model)
   - a sync "window buffer" the renderer reads
   - push API (consumer fills) + on_window_change callback (table requests)
   - rendering/interaction we already shipped: selection, sort UI, column ops,
     drag-autoscroll, right-pinned actions  — re-keyed by RowKey
        │
        ▼
rdom-tui (substrate)
```

**Why generic:** `rdom-virtualtable` is to a data backend what `rdom-tui` is to an app — a substrate. Binding it to Observatory would make it single-purpose and couple a UI component to Arrow. The Observatory adapter is small and belongs with the consumer (or a clearly-separate binding crate), never in the component.

## 4. Data model (in `rdom-virtualtable`)

```rust
/// Opaque, cheap-to-clone stable row identity. The consumer constructs it from
/// the source's primary key (e.g. Observatory `schema.primary_key` →
/// "_source_id\u{1}namespace\u{1}name"). The table treats it as an opaque key.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RowKey(Arc<str>);   // exact repr TBD; Arc<str> for O(1) clone + hash

/// A typed cell value. `Text` is the default/fallback. Starter set; extend as
/// real columns demand (Progress/Badge/Link/etc. are deferred). Type drives
/// rich rendering AND in-memory-mode sort comparison.
#[derive(Clone)]
pub enum CellValue {
    Empty,
    Text(String),
    Number(f64),            // unit-less; formatting is the consumer's/renderer's job
    Bytes(u64),
    Duration(std::time::Duration),  // e.g. Age
    Status { text: String, level: StatusLevel },
}
pub enum StatusLevel { Ok, Warn, Error, Info }

/// One row: identity + cells in column order.
#[derive(Clone)]
pub struct Row {
    pub key: RowKey,
    pub cells: Vec<CellValue>,   // index aligns with the table's column order
    // status/version intentionally omitted from v1 — fold status into a cell.
}
```

Columns keep the existing `Column` type (header + width), extended later with a stable column key + type hint if needed. **Cells are typed (`CellValue`), default `Text`** — replacing today's `String` cells.

## 5. The table data API (push model, sync)

The table owns a **window buffer**: the rows for the currently materialized range, indexed by position *and* keyed by `RowKey`, plus the known total. The consumer fills it; the table renders from it and asks for more when the window moves.

```rust
impl VirtualTableView {
    /// Total rows in the (filtered) result — drives the scrollbar extent.
    pub fn set_total(&self, dom: &mut TuiDom, total: usize);

    /// Replace the buffer for the visible range with `rows` (a Resync result for
    /// `start..start+rows.len()`). Rows outside the range render as placeholders.
    pub fn set_window(&self, dom: &mut TuiDom, start: usize, rows: Vec<Row>);

    /// Live deltas (Observatory keeps the window correct, so these are scoped to
    /// it). Upsert: replace/insert the row with that key in the visible buffer
    /// and re-render its cells. Remove: drop it. Keyed by RowKey — O(1).
    pub fn apply_upsert(&self, dom: &mut TuiDom, rows: Vec<Row>);
    pub fn apply_remove(&self, dom: &mut TuiDom, keys: &[RowKey]);

    /// The table calls this whenever the visible range changes (scroll, resize,
    /// sort/filter change). The consumer re-queries + re-subscribes Observatory
    /// for the new range and pushes set_window/apply_* back. Debounced by the
    /// table on viewport-settle so a fast scroll doesn't fire a query per row.
    pub fn on_window_change(&self, cb: impl FnMut(WindowRequest) + 'static);
}

pub struct WindowRequest {
    pub range: Range<usize>,       // visible window + a prefetch margin
    pub sort: Vec<SortSpec>,       // current sort (table → consumer → Observatory)
    pub filter: Option<FilterSpec>,// current filter
}
```

- **Placeholders:** any position in the visible range without a row yet renders a "loading" shimmer (a cell style). No blank gaps; no stalls.
- **Prefetch margin:** the table requests a range slightly larger than strictly visible (e.g. ±50%) so adjacent scroll is shimmer-free. Observatory's warm cache makes the extra cheap; the *table* owns the margin policy, the consumer just serves the requested range.
- **Sort/filter UI** (header click, filter bar) updates the table's `sort`/`filter` state and fires `on_window_change` with the new specs → consumer re-subscribes → `Resync` → `set_window`.

## 6. Delta & window semantics (the contract with Observatory)

Because Observatory maintains the live windowed-sorted-filtered view:

- A subscription is **per visible window** (`range` = the requested window+margin). Its first event is `Resync` (sorted snapshot of that window) → `set_window`.
- As upstream changes, Observatory emits `Upsert`/`Remove` **that keep the window correct**: a row whose sort key moves it out of the window arrives as `Remove`; the row that takes its place arrives as `Upsert`; an in-place change is an `Upsert` for the same key. → the table applies them verbatim by `RowKey`; it never recomputes ordering or membership.
- `total` changes (rows added/removed to the filtered set) arrive via `set_total` (consumer derives from a count subscription/query).
- **On window change** the consumer drops the old subscription and opens a new one for the new range. Warm-cache `Resync` is sub-ms, so this is cheap.

This keeps the table a **pure projection of "current window + deltas"** — the thin client the whole design hinges on.

## 7. In-memory convenience mode (kept)

Not every consumer has Observatory. The existing in-memory `VirtualTable` stays as a **built-in window filler**: `set_rows(..)` + client-side sort/filter (the current `sort_by` etc., now over `CellValue`), and the table fills its own window buffer from it on scroll (today's `show_window` path). This is the default for the `scroll_table` example, tests, and simple apps. Same rendering, same buffer — just self-filled instead of consumer-pushed.

## 8. Identity: cursor & selection by `RowKey`

Positional identity cannot survive live re-sorting or windowing. Therefore:

- **Cursor** points at a `RowKey` (the active *resource*), with a derived absolute index when it's within the window (for scroll math + `SCROLL-SINGLE-OWNER` reveal). Keyboard nav past the window edge requests the next window from the consumer.
- **Selection** is a set of `RowKey`s — survives scroll, re-sort, and live updates (a selected pod stays selected when its row moves).
- This is a migration of the existing selection/cursor (today positional) onto `RowKey`. The interaction layer (drag-autoscroll, sort UI, column ops, right-pinned actions) is unchanged above this line.

## 9. Async boundary

Observatory is async (tokio); rdom-tui handlers are sync. The **consumer** runs Observatory on a runtime and bridges results to the table via rdom-tui's **`AppHandle` cross-thread inject queue** (already exists, already used for this class of thing). The table crate has **no async**: every `set_window`/`apply_*` call happens on the UI thread inside an injected callback. Fully testable headless with synchronous pushes.

## 10. Migration from the current model

1. `CellValue` replaces `String` cells (breaking; pre-1.0 OK).
2. `RowKey` introduced; cursor/selection re-keyed.
3. Window buffer introduced; `show_window` reads it; in-memory `VirtualTable` becomes a filler.
4. Push API + `on_window_change` added.
5. Existing tests migrated (string cells → `CellValue::Text`; positional asserts → `RowKey`).

## 11. Phased plan

- **P0** — this spec → review gate.
- **P1** — `RowKey`, `CellValue`, `Row`; the window buffer + placeholder rendering; in-memory filler parity (all current tests green on the new model). No API break for the example beyond `String`→`CellValue`.
- **P2** — push API (`set_total`/`set_window`/`apply_upsert`/`apply_remove`) + `on_window_change` (debounced) + prefetch margin. Cursor/selection on `RowKey`.
- **P3** — reference Observatory adapter (in the lens TUI or a `rdom-virtualtable-observatory` crate): Arrow→`CellValue`, query/subscribe, `AppHandle` bridge. End-to-end against Observatory's storybook/fixtures.
- **P4** — persistence callbacks (sort/order/width/hidden) for the consumer to save UI state.

## 12. Open questions / risks

- **`RowKey` representation** — `Arc<str>` vs a composite `SmallVec<Arc<str>>`. Lean toward a single opaque `Arc<str>` the consumer builds from the PK tuple.
- **`CellValue` starter set** — confirm the minimum that covers the lens columns (Text/Number/Bytes/Duration/Status). Defer Progress/Badge/Link until a column needs them.
- **Prefetch margin policy** — fixed ±N rows vs ±fraction; tune against Observatory latency.
- **Count freshness** — count query cadence vs a count subscription; how stale the scrollbar may be.
- **Re-subscribe churn on scroll** — confirm Observatory is happy with frequent per-window re-subscribes (warm cache says yes; validate).

## 13. Review gates (run before P1)

**Grumpy chief architect:** Is the table genuinely a thin projection (no engine creep)? Are the crate boundaries clean (no arrow/tokio/observatory leak into the component)? Is `RowKey`/`CellValue` the right contract? Does the push API + `on_window_change` cover live + windowed + sort/filter without the table ever owning ordering? Is the in-memory mode a true peer, not a fork?

**Grumpy chief API:** Can a consumer wire Observatory to this without surprises? Is the push API minimal and hard to misuse (e.g. set_window vs apply_* ordering)? Is the migration (String→CellValue, positional→RowKey) tolerable pre-1.0? Does it stay browser/DOM-faithful where it overlaps rdom-tui?
