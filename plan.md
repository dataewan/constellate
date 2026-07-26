# Technical Implementation Plan: Terminal Knowledge Management & Discovery Engine in Rust

## Executive Summary

This document outlines the architecture, technology stack, data flow, and phased development roadmap for building a terminal-native knowledge discovery application in **Rust**. The tool indexes local Markdown notes, surfaces related notes as you navigate, lets you edit them in your own `$EDITOR`, and — later — augments relatedness with local vector embeddings and optional web/Wikipedia enrichment, all inside a **Ratatui** Terminal User Interface (TUI).

The three primary user journeys the design optimizes for:

1. **Browse & search** the whole vault quickly from the keyboard.
2. **Edit** any note by handing off to your own `$EDITOR`, then seeing the index refresh automatically.
3. **Discover & reference** — see related notes for the current note and yank a reference to any note's filename into the system clipboard.

### Guiding principles (why the plan is sequenced the way it is)

- **Deliver the use cases first, ML later.** Browsing, editing, and clipboard references need no embeddings. They are built and shippable in Phases 1–2 as a genuinely useful tool; embeddings *augment* an already-working product.
- **Related notes start cheap.** Before any model is involved, relatedness is computed from signals we already parse — shared `[[wikilinks]]`, shared tags, and title/keyword overlap. These are instant and deterministic. Embeddings later add a semantic layer on top.
- **Keep it small.** For a personal vault (thousands of notes → tens of thousands of chunks), brute-force cosine similarity in Rust is sub-millisecond. No dedicated vector database is needed for v1.
- **Local-first, minimal dependencies.** Ollama is the primary embedding backend (one HTTP call, no native build pain). A pure-Rust in-process backend (`fastembed`) is an optional, feature-gated alternative. A single static binary is a nice-to-have, not a requirement.
- **Don't over-async.** The TUI render loop is synchronous. Slow work (HTTP, embedding) runs on a background worker thread and reports back over a channel, so the UI never blocks and there's no async runtime to reason about.

---

## 🏗️ System Architecture

```
┌───────────────────────┐        ┌──────────────────────────────────────┐
│  Markdown Vault Dir    │        │            RUST APPLICATION           │
│  (*.md Files)          │        │                                       │
└───────────┬───────────┘        │   ┌────────────────────────────────┐  │
            │                     │   │   Ratatui / Crossterm TUI      │  │
            ▼                     │   │   (synchronous render loop)    │  │
┌───────────────────────┐        │   └───────────────┬────────────────┘  │
│ File Watcher           │ ─────> │                   │  events           │
│ (notify + debouncer)   │        │           ┌───────▼────────┐          │
└───────────────────────┘        │           │  App State /   │          │
                                 │           │  Event Engine  │          │
                                 │           └──┬─────────┬───┘          │
                                 │              │         │ channel      │
                                 │   ┌──────────▼──┐   ┌──▼───────────┐  │
                                 │   │ Vault + DB  │   │ Background    │  │
                                 │   │ (index,     │   │ Worker Thread │  │
                                 │   │  search,    │   │ (embeddings,  │  │
                                 │   │  related)   │   │  HTTP fetch)  │  │
                                 │   └─────────────┘   └──────┬───────┘  │
                                 └────────────────────────────┼──────────┘
                                                              │
                        suspend TUI ──> $EDITOR (child)       ▼
                                                    Ollama / Wikipedia
```

The render loop owns the terminal and stays synchronous. When a note is opened for editing, the TUI suspends (leaves the alternate screen, disables raw mode), spawns `$EDITOR` as a foreground child, waits, then restores and re-indexes the touched file. Embedding and network calls are dispatched to a background worker thread and results arrive back over a channel, so the UI never blocks.

---

## 🛠️ Technology Stack

| Domain | Crate / Library | Purpose |
| :--- | :--- | :--- |
| **Terminal UI** | `ratatui` + `crossterm` | Rich terminal layout, widgets, key bindings, ANSI rendering; synchronous event loop. |
| **CLI & Flags** | `clap` | Command-line argument parsing (e.g., `--vault ~/Notes`). |
| **Markdown Engine** | `pulldown-cmark` | Fast parsing for splitting notes by headings and extracting frontmatter, links, and tags. |
| **File Watching** | `notify` + `notify-debouncer-full` + `walkdir` | Recursive traversal and debounced filesystem events for live re-indexing. |
| **Index / Storage** | `rusqlite` (bundled) | Embedded SQLite storing note metadata, chunks, and embeddings as blobs. No vector extension in v1. |
| **Embeddings (primary)** | Ollama via `reqwest` (blocking) | Local Ollama server `/api/embeddings` (e.g. `nomic-embed-text`, 768-dim), called from the worker thread. |
| **Embeddings (optional)** | `fastembed` (feature-gated) | In-process ONNX embeddings (`all-MiniLM-L6-v2`, 384-dim) for a no-daemon setup. Off by default. |
| **HTTP Client** | `reqwest` (blocking, `json`) | Ollama, and optional Wikipedia/web enrichment. Runs on the worker thread — no async runtime. |
| **Editor Handoff** | `std::process::Command` | Suspend the TUI and launch `$EDITOR`; no extra crate required. |
| **Clipboard** | `arboard` | Cross-platform system clipboard for yanking note references. |
| **Serialization** | `serde` + `serde_json` | Config, frontmatter, and HTTP payloads. |

> **Deliberately omitted:** `tokio` (the loop is synchronous; concurrency is a worker thread + channel) and `sqlite-vec` (brute-force cosine in Rust is fast enough at personal-vault scale). Both can be revisited if a real need is measured.

---

## 📐 Subsystem Specifications

### 1. Vault Engine (File Scanner & Chunker)

- **Traversal:** `walkdir` recursively discovers all `.md` files in the vault path, **excluding** the app's own `.constellate/` state directory.
- **Parsing:** `pulldown-cmark` extracts YAML frontmatter, `[[wikilinks]]`, tags, and headings. Long documents are split into semantic "chunks" grouped by Markdown headers (`#`, `##`, `###`), capped at a few hundred tokens each.
- **Live Sync:** `notify` + `notify-debouncer-full` coalesces rapid save events (editors fire several per write) and re-indexes only affected documents. Editor-driven edits flow through this same path, plus an explicit re-index on `$EDITOR` exit for immediacy.
- **Search Indexes:** Maintains a fuzzy title index and a full-text token index so global search (`/`) matches across the whole vault before any embeddings exist.

### 2. Related Notes Engine

Relatedness is layered, cheapest signals first:

- **Link graph:** notes connected via shared or reciprocal `[[wikilinks]]`.
- **Tag overlap:** notes sharing frontmatter/inline tags.
- **Keyword/title overlap:** token overlap on titles and headings.
- **Semantic (added in Phase 3):** cosine similarity over chunk embeddings, merged into the same ranked list with a similarity score.

This means the right-hand "related notes" panel is useful from Phase 2, and embeddings enrich rather than gate it.

### 3. Storage (`.constellate/index.db`)

- Lives in `<vault>/.constellate/` (portable with the vault; excluded from the scan).
- **`notes`** — `file_path`, `title`, `frontmatter`, `updated_at`.
- **`chunks`** — `id`, `note_path`, `header`, `content`, `content_hash`, `updated_at`.
- **`links` / `tags`** — extracted references for the cheap related-notes signals.
- **`embeddings`** — `chunk_id`, `vector` (BLOB of `f32`s). Populated only once an embedding backend is enabled.
- **`meta`** — active embedding backend + model + vector dimension. On mismatch (switching backend/model), embeddings are rebuilt rather than mixing incompatible vectors.
- **Incremental updates:** `content_hash` per chunk means only changed chunks are re-embedded on a file event, never the whole vault.

### 4. Vector Search (Phase 3+)

- **Pluggable backend:** an `Embedder` trait abstracts embedding generation:
  - `OllamaEmbedder` (primary) — POSTs chunk text to a local Ollama server's `/api/embeddings` from the worker thread.
  - `FastEmbedder` (optional, `--features fastembed`) — in-process quantized ONNX `all-MiniLM-L6-v2`.
- **Similarity:** brute-force cosine similarity in Rust over blob-decoded vectors; top *K* (*K* = 5) chunks aggregated to related notes. No `sqlite-vec` dependency in v1.
- **Non-blocking:** embedding a (re)indexed file runs on the worker thread; the UI shows a lightweight "indexing…" indicator and updates when results arrive over the channel.

### 5. Editor Integration

- **Launch:** on `e` (or `Enter` on the file tree), open the selected note in `$EDITOR` (falling back to `$VISUAL`, then `vi`/`nano`).
- **Terminal Handoff:** before spawning, leave the alternate screen, disable raw mode, and show the cursor. Spawn the editor as a foreground child inheriting stdio and block on it. On exit, re-enter the alternate screen, re-enable raw mode, and force a full redraw.
- **Refresh:** after the editor exits, synchronously re-index the edited file so related notes and the preview update immediately; the debounced watcher remains the fallback for external edits.

### 6. Terminal User Interface (TUI)

- **3-Pane Split View:**
  - **Left (30%):** vault file tree / global fuzzy search input (`/`).
  - **Center (45%):** **read-only** note renderer with highlighted headers. Editing is delegated to `$EDITOR`, so this pane never needs a text-input mode.
  - **Right (25%):** context inspector — top: related notes with the signal/score; bottom (Phase 5, optional): Wikipedia/web summaries.
- **Clipboard / References:** `y` yanks a reference to the selected note into the system clipboard via `arboard`. Format is configurable (default: path relative to the vault root; alternatives: absolute path, or `[[wikilink]]`). A status-line message confirms what was copied.
- **Key Bindings:**
  - `j` / `k` or `↑` / `↓`: move selection in the active pane.
  - `Tab` / `Shift+Tab`: cycle focus across panes.
  - `Enter`: load the selected related note into the center pane (or open in `$EDITOR` from the file tree).
  - `e`: open the current/selected note in `$EDITOR`.
  - `y`: yank a reference to the selected note into the system clipboard.
  - `/`: toggle global fuzzy search.
  - `w`: (Phase 5, optional) trigger external web/Wikipedia lookup for the highlighted topic.
  - `q`: quit.

### 7. External Knowledge Engine (Phase 5 — optional / experimental)

> This is the lowest-value feature relative to the core use cases and the most fragile (noisy entity extraction; Tavily needs an API key; DuckDuckGo has no official API). It is **off by default** and may be cut. Kept last so it never blocks the useful parts.

- **Entity Extraction:** strip stop words; extract key nouns / heading keywords from the current note.
- **Wikipedia API:** query `https://en.wikipedia.org/api/rest_v1/page/summary/{term}` from the worker thread; render summary cards in the inspector.

---

## 🗓️ Phased Development Roadmap

### Phase 1: Core CLI, Browsing & Editing — done — *delivers use cases 1 & 2*

- [x] Initialize Rust workspace with `clap`, `pulldown-cmark`, `walkdir`, `ratatui`, `crossterm`.
- [x] Build the recursive crawler + heading-based chunker; exclude `.constellate/`.
- [x] Implement the SQLite schema (`notes`, `chunks`, `links`, `tags`, `meta`) and incremental hashing.
- [x] Build the file tree + full-text/title search so the vault is fully browsable.
- [x] Implement the `$EDITOR` suspend/launch/restore flow and re-index on exit.
- [x] Add `notify` + `notify-debouncer-full` for external edits.

### Phase 2: References & Cheap Related Notes — done — *delivers use case 3*

- [x] Extract and store `[[wikilinks]]` and tags during parsing.
- [x] Implement the Related Notes Engine (link graph + tag + keyword overlap) and render it in the right pane.
- [x] Add `arboard` clipboard yank with configurable reference formats + status-line confirmation.

**→ At this point all three use cases are met. Ship and dogfood before continuing.**

### Phase 3: Local Semantic Embeddings (Ollama) — done

- [x] Define the `Embedder` trait; implement `OllamaEmbedder` (worker thread, `reqwest` blocking).
- [x] Store embeddings as blobs; implement brute-force cosine similarity in Rust.
- [x] Wire the background worker + channel so (re)indexing never blocks the UI.
- [x] Merge semantic results into the related-notes list; gate rebuilds via the `meta` table.

### Phase 4: Optional In-Process Backend & Polish

- [ ] Add the feature-gated `FastEmbedder` (ONNX) for a no-daemon setup.
- [ ] Cross-platform build/test on Linux, macOS, and Windows (note the `arboard`/X11 clipboard-on-exit quirk on Linux).

### Phase 5: External Enrichment (Optional / Experimental)

- [ ] Implement the Wikipedia fetcher on the worker thread, off by default.
- [ ] Render response cards in the inspector; evaluate whether it earns its keep.

---

## 📦 Sample Project Structure & Config

```
constellate/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── cli.rs
│   ├── config.rs           # vault path, embedding backend, reference format
│   ├── vault/
│   │   ├── mod.rs
│   │   ├── scanner.rs
│   │   └── chunker.rs
│   ├── db/
│   │   ├── mod.rs
│   │   └── store.rs         # SQLite: notes, chunks, links, tags, embeddings, meta
│   ├── related.rs          # link graph + tag/keyword overlap (+ semantic in Phase 3)
│   ├── embed/
│   │   ├── mod.rs          # Embedder trait + brute-force cosine
│   │   ├── ollama.rs
│   │   └── fastembed.rs    # feature = "fastembed"
│   ├── worker.rs           # background thread: embeddings + HTTP, channel-based
│   ├── editor.rs           # $EDITOR suspend/launch/restore
│   ├── clipboard.rs        # arboard reference yank
│   ├── external/
│   │   ├── mod.rs          # optional / experimental
│   │   └── wikipedia.rs
│   └── ui/
│       ├── mod.rs
│       ├── layout.rs
│       └── app.rs
```

### Initial `Cargo.toml`

```toml
[package]
name = "constellate"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
ratatui = "0.28"
crossterm = "0.28"
pulldown-cmark = "0.12"
walkdir = "2"
notify = "6"
notify-debouncer-full = "0.3"
rusqlite = { version = "0.31", features = ["bundled"] }
reqwest = { version = "0.12", default-features = false, features = ["blocking", "json", "rustls-tls"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
arboard = "3"

# Optional in-process ONNX embedding backend (no external service).
fastembed = { version = "4", optional = true }

[features]
default = []
fastembed = ["dep:fastembed"]
```

> Versions are indicative — being greenfield, pin to the latest releases at implementation time.
