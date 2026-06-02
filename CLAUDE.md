# rdom-virtualtable Agent Guide

This file defines how AI agents work in this repository. `AGENTS.md` is a symlink to it.

`rdom-virtualtable` is a **virtualized table** for [rdom](https://github.com/miskun/rdom) — a native
`<table>` that materializes only the visible row window. It is a **downstream consumer** of the rdom
substrate (charts live in the separate `rdom-charts` crate; this crate is table-only and focused).

## Where to look first

- `STATE.md` — roadmap, decisions, open risks. Read before starting.
- `README.md` — what's shipped vs planned.
- The code: `src/virtual_table.rs` has the model + windowing; tests document the contracts.

## Non-negotiable principles

- **Public API only.** Build strictly on `rdom-tui`'s published surface — `create_element` /
  `create_text_node` / `append_child` / `drop_subtree`, the native `<table>` family +
  `table::size_columns`, node sizing via `TuiStyle`, runtime event listeners. Never reach into rdom
  internals; if something needs a new hook, that's a change request against rdom.
- **Genuine virtualization.** Only the visible window of rows is ever in the DOM. `show_window`
  must `drop_subtree` the previous window (detach alone leaks arena slots) and re-sync column
  widths. Pure windowing math (`window_for`) stays separate and unit-tested.
- **Theme-agnostic.** Speak `rdom_tui` types directly; no app-specific theme abstraction.

## Engineering rules

- **TDD always** — failing test first, then the smallest change. Pure logic (windowing, model
  bookkeeping) gets unit tests; rendering gets headless integration tests (`cascade → layout_dom →
  Buffer::empty → paint_dom`, inspect cells / count materialized rows). Docs-only changes excepted.
- **Real fixes only** — no lint-silencing to dodge the gate; no silent fallbacks.
- **Code and docs move together** — update `STATE.md` with meaningful decisions in the same commit.

## The gate (before every commit destined for push)

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Doc-only commits skip the test pass. Fix and re-run on failure — no `fix: drop unused …` follow-ups.
After push, the working tree must be clean (a `/clear`-safe entry point).

## Milestone review gates

At the end of a milestone, run the Grumpy Chief Architect + Grumpy Chief Product/API passes and
record findings in `STATE.md` before starting the next.
