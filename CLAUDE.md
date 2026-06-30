# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

MyRSS is a terminal RSS/Atom feed reader with vim-like controls, written in Rust.
It is a fork of Clark Kampfe's `russ` (everything after commit e92fb5e is local work).
Design philosophy is **local-first and manual**: feeds never auto-refresh and entries
are never auto-marked read — the user is always in explicit control. The full feed
database lives locally in SQLite for offline reading.

## Features

- Vim-like navigation and modes (Normal / Editing / Command / Settings / ViewLlmLog)
- Feed management: subscribe, delete, refresh one/all, OPML import/export
- Entry filtering: read/unread/noteworthy toggles
- ASCII-art rendering of article images in the terminal
- Opt-in LLM article summarization (Gemini / OpenAI / Anthropic / Groq) with local
  caching and a per-day request limit
- Clipboard copy (`c`) and open-in-browser (`o`)

## Architecture

- **Store**: SQLite via `rusqlite` + an `r2d2` connection pool. Schema set up in
  `rss::initialize_db()`.
- **TUI**: `ratatui` for rendering, `crossterm` for raw-terminal control and input.
- **Concurrency**: main thread runs the input/event loop; a background IO thread
  handles feed refresh (parallel HTTP via the pool), LLM calls, and ASCII rendering.
  The two communicate over `std::sync::mpsc`. Shared state is `Arc<Mutex<AppImpl>>`.
- **HTTP**: synchronous `ureq` + `serde_json` only — see coding principles below.

Module map (`src/`): `app.rs` core state · `rss.rs` schema/CRUD/parsing ·
`ui.rs` rendering · `llm.rs` LLM providers · `io.rs` IO event loop ·
`ascii.rs` image download + SSRF checks + ASCII conversion · `cache.rs` summary/log
cache + rate limit · `settings.rs` config · `modes.rs` mode enums · `util.rs` list
helpers · `opml.rs` OPML import · `main.rs` CLI + terminal setup.

Paths: feed DB defaults to `~/.local/share/russ/feeds.db` (XDG-aware, `-d` to
override); LLM config at `~/.myrss/config.json`; summary cache at `~/.myrss/cache.db`.

## Build, Test & Run

- Build / run / test: standard `cargo build`, `cargo run`, `cargo test`.
- **Linux build deps** (also installed in CI): `libxcb-shape0-dev libxcb-xfixes0-dev`.
- The crate is split lib + bin (`lib.rs` + `main.rs`) so integration tests in `tests/`
  can exercise internal logic. Run one test with `cargo test <name>`.
- CI (`.github/workflows/rust.yml`) builds and tests on push/PR to `master`.

## Coding Principles (read before changing code)

- **Correctness and safety over speed.** `#![forbid(unsafe_code)]` is set — keep it.
- **Defense-in-depth, especially anything touching the network or untrusted input.**
  This app fetches arbitrary remote feeds, web pages, and images, so treat all of it
  as hostile. Preserve existing guardrails and add layers, don't remove them:
  - SSRF: validate every outbound URL before fetching (`ascii.rs::is_safe_url`) —
    http/https only, resolve host and block loopback/private/link-local/ULA ranges.
  - Resource limits: cap image downloads at 5 MB and dimensions at 4096×4096 to stop
    decompression/resource bombs. Apply the same mindset to any new fetch.
  - Avoid regex on untrusted input where a linear scan works (ReDoS) — see
    `ascii.rs::extract_image_urls`.
- **Input sanitization always.** Decode/escape HTML before display; wrap
  user/feed/article content in delimiters when sending to an LLM to resist prompt
  injection (see the `<article_text>` wrapping in `llm.rs`).
- **No hardcoded dynamic content.** Anything scraped, fetched, or user-fed must be
  extracted and processed at runtime — never baked into source or test fixtures.
- **Direct Rust HTTP layer.** Network calls (incl. LLM) use the existing `ureq` +
  `serde_json` stack. Do not pull in external runtimes (no Python, no async runtime,
  no heavy wrapper libs).
- **Idiomatic Rust.** Match the surrounding style; prefer `Result`/`anyhow` error
  propagation over panics in library paths.

## Testing Discipline

- **Regression test every bug — no exceptions.** When a bug is found — by you or
  the user — writing the regression test is part of the fix, not optional follow-up.
  Do not mark a bug fix complete until the test exists, fails on the unfixed code,
  and passes on the fixed code.  This applies even when the user does not explicitly
  ask for a test.
- **Cover all applicable levels.** For each bug, write tests at every level that
  meaningfully exercises the defect:
  - *Unit*: test the smallest pure function involved (extract one if needed).
  - *Integration*: exercise the subsystem end-to-end inside the process (e.g. render
    to a `TestBackend`, call through the public `AppImpl` API).
  - *E2E*: reproduce the exact environment that triggered the bug (terminal size,
    input sequence, etc.) and assert the observable symptom is gone.
  Put tests in `tests/` as integration tests where possible, otherwise as a unit test
  (with `#[cfg(test)]`) in the relevant module.
  (See `tests/scrolling_tests.rs`, `tests/summary_tests.rs`,
  `tests/ui_summary_truncation_tests.rs`.)
- **Test behavior, not lines.** Aim for meaningful behavioral coverage of what the
  code should do; do not chase line-coverage metrics.
- Run `cargo test` after every change to confirm it compiles and passes.

## Workflow

- Make small, logically focused commits — not mega-commits.
- For user-facing changes (UI, keybindings, visual states), implement and self-test,
  then **pause for the user to verify** before moving on.
