# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Status: Phases 1–3 implemented

Phases 1–3 are built and working; Phase 5 (optional web enrichment) and the optional `fastembed` backend (part of Phase 4) are not yet started. What exists today: CLI (`--vault`, `--db`, `--ref-format`, `--no-embed`, `--embed-model`, `--ollama-url`), recursive Markdown scanner (excludes `.constellate/` and hidden dirs), frontmatter/wikilink/tag/heading-chunk parser, SQLite index with incremental content-hashing, a synchronous ratatui 3-pane TUI (list + search + read-only preview + related-notes pane), the cheap related-notes engine (`related.rs`), **semantic embeddings via Ollama** (`embed/`, computed on a background worker thread, `worker.rs`) with brute-force cosine similarity **merged into** the related-notes list, **clipboard yank** via `arboard`, `$EDITOR` suspend/restore, a debounced `notify` watcher, and a file logger (`logging.rs`) for background-worker events. `cargo test` covers parsing, the store round-trip, relatedness, merge, and semantic similarity (16 tests). The rest of this file still describes the full intended design from `plan.md`; treat anything beyond the above as not-yet-built.

> **Dependency note:** `rusqlite` is pinned to `0.37` on purpose — 0.38+ pulls `libsqlite3-sys` ≥0.38, whose build script uses the still-unstable `cfg_select!` and won't compile on the current toolchain. Don't bump it without re-checking that.

## What this is

`constellate` is a terminal-native knowledge-discovery tool in **Rust**. It indexes a local Markdown vault, computes vector embeddings locally, finds semantically similar notes as you navigate, and enriches the active note with Wikipedia/web lookups — all inside a `ratatui` TUI. The three journeys the design optimizes for: **browse & search** the vault, **edit** notes by handing off to `$EDITOR`, and **discover & reference** related notes (including yanking a note reference to the system clipboard).

Constellate is **not** a text editor — editing is delegated to the user's `$EDITOR`. A single static binary is a nice-to-have, not a hard requirement.

## Planned architecture

A **synchronous** ratatui render loop plus a background worker thread; slow work reports back over a channel (see `plan.md` for the full diagram and phased roadmap). Deliberately **no `tokio`** and **no `sqlite-vec`** — concurrency is a worker thread, and brute-force cosine in Rust is fast enough at personal-vault scale.

- **Vault Engine** — `walkdir` discovers `.md` files (excluding the `.constellate/` state dir); `pulldown-cmark` parses frontmatter, `[[wikilinks]]`, tags, and headings, splitting documents into heading-grouped chunks; `notify` + `notify-debouncer-full` coalesces save events and re-indexes only affected notes.
- **Related Notes Engine** — layered, cheapest first: shared `[[wikilinks]]`, tag overlap, title/keyword overlap, then (Phase 3+) semantic cosine similarity merged into the same ranked list. Useful from Phase 2 without any model.
- **Storage** — `<vault>/.constellate/index.db` via `rusqlite` (bundled). Tables: `notes`, `chunks` (with `content_hash`), `links`, `tags`, `embeddings` (chunk_id, `f32` BLOB), `meta` (active backend/model/dimension → rebuild on mismatch).
- **Vector Search (`embed/`, implemented)** — pluggable `Embedder` trait: `OllamaEmbedder` (**primary**, `reqwest` blocking to a local Ollama server, default `nomic-embed-text`). Chunk vectors are stored as BLOBs; `SemanticIndex` aggregates them to a normalized per-note mean vector and does brute-force cosine in Rust, above a similarity threshold. Results are scaled and **merged** into the cheap related list via `related::merge`. Cached embeddings power semantic search even with `--no-embed` (offline); the `meta.embed_model` row gates a rebuild when the model changes. A feature-gated in-process `FastEmbedder` is planned but not yet built.
  - **Embedding robustness (learned the hard way):** embedding models have a fixed context window (nomic-embed-text = 2048 tokens); over-long input returns HTTP 500. `OllamaEmbedder` truncates input to `MAX_INPUT_CHARS` (4000) to stay under it. Failures are classified via `EmbedError`: `Unreachable` (backend down → stop this session, show "unavailable") vs `Skip` (one bad input → log it and continue). **The worker must never abort the whole batch on a single chunk failure** — that was the original bug. Skips/failures are written to `<vault>/.constellate/constellate.log` (there is no stderr logging while the TUI owns the terminal).
- **Editor Integration** — `e`/`Enter` suspends the TUI (leave alternate screen, disable raw mode), spawns `$EDITOR` as a foreground child via `std::process::Command`, then restores the TUI and re-indexes the edited file on exit.
- **TUI** — `ratatui` + `crossterm` 3-pane layout: file tree/search (left), **read-only** note renderer (center), context inspector with related notes + optional web summaries (right). `y` yanks a note reference (relative path / absolute / `[[wikilink]]`, configurable) to the system clipboard via `arboard`.
- **External Knowledge Engine (Phase 5, optional/experimental)** — key-noun extraction → Wikipedia REST summary on the worker thread. Off by default; may be cut.

Current module layout: `src/{main.rs, cli.rs, config.rs, vault/{mod,scanner,chunker}, db/{mod,store}, related.rs, embed/{mod,ollama}, worker.rs, editor.rs, clipboard.rs, logging.rs, ui/{mod,app}, watch.rs}`. Not yet present: `embed/fastembed.rs`, `external/wikipedia.rs`.

## Key architectural constraints

- **Deliver use cases before ML.** Browsing, editing, and clipboard references (Phases 1–2) need no embeddings and are the shippable core. Embeddings *augment* an already-working tool; don't let them block it.
- **Local-first, not necessarily single-binary.** Core indexing/search must work without a network. Ollama and bundled native libs are acceptable runtime deps. The Ollama backend and web enrichment are the only network features and must stay optional.
- **Synchronous UI, work off-thread.** Keep the render loop synchronous and single-owner of the terminal; dispatch HTTP/embedding to the worker thread and receive results over a channel. Do **not** reintroduce `tokio`.
- **Related notes start cheap** — link graph + tags + keyword overlap before any embedding; semantic similarity is an additive layer.
- **Editing is delegated to `$EDITOR`** — the center pane is read-only and never needs a text-input mode. Don't build an in-app editor.
- **Incremental indexing** — per-chunk `content_hash` means filesystem/editor events re-embed only changed chunks, never a full rebuild.
- **Embedding backends must be interchangeable** behind the `Embedder` trait; switching backend/model changes vector dimension, so the `meta` table gates a rebuild.

## Commands

- `cargo build` / `cargo build --release` — build
- `cargo run -- --vault ~/Notes` — run the TUI against a vault (defaults to the current dir)
- `cargo run -- --vault ~/Notes --no-embed` — run without Ollama (cheap relatedness + any cached embeddings)
- `cargo run -- --vault ~/Notes --embed-model nomic-embed-text --ollama-url http://localhost:11434` — embedding backend options
- `cargo test` — run all tests
- `cargo test <name>` — run a single test by name substring
- `cargo clippy` — lint
- `cargo fmt` — format

The index lives at `<vault>/.constellate/index.db`. Inspect it with `sqlite3 <vault>/.constellate/index.db`. Background-worker events (embedding skips/failures) are logged to `<vault>/.constellate/constellate.log` — check there first when embeddings/related notes misbehave.

### Keybindings (in-app)

`j`/`k` or `↑`/`↓` move the selection, `/` search (Enter applies, Esc clears), `e`/`Enter` open the selected note in `$EDITOR`, `y` copy a reference to the selected note to the clipboard (format set by `--ref-format`), `q` quit.
