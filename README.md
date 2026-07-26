# constellate

A terminal app for reading and connecting a folder of Markdown notes.

constellate indexes a directory of Markdown files and gives you a keyboard-driven
interface to browse them, open them in your own editor, and see which notes are
related. You can gather notes into a scratchpad and then link them together or
hand them to a local language model to draft a new note.

Your notes are only changed when you explicitly ask for it — by editing a file,
adding a link, or generating a new note. Everything else is read-only.

## Requirements

- A recent Rust toolchain, to build.
- Optional: a running [Ollama](https://ollama.com) server for semantic "related
  notes" and the language-model features. Without it, related notes are still
  found from links, tags, and shared words; pass `--no-embed` to skip Ollama
  entirely.

## Building

```
cargo build --release
```

The binary is written to `target/release/constellate`. During development you
can also run it directly with `cargo run --`.

## Running

```
constellate --vault ~/notes
```

Point `--vault` at the directory of Markdown files you want to browse. If you
leave it out, the current directory is used.

The interface has four panes: a list of files, a read-only preview of the
selected note, a list of related notes, and a scratchpad. The index is kept in a
`.constellate` folder inside your vault; delete it to reindex from scratch.

## Keys

| Key | Action |
| --- | --- |
| `j` / `k`, arrows | Move within the focused pane |
| `1`–`4`, `Tab` | Switch panes (files, preview, related, scratchpad) |
| `Enter` | Open the selected note in your editor; in the related pane, jump to that note |
| `e` | Open the current note in your editor |
| `/` | Search by filename, title, or content (`Esc` to leave, again to clear) |
| `y` | Copy a Markdown link to the current note |
| `a` / `x` | Add the current note to the scratchpad / remove the selected one |
| `l` | Link the scratchpad notes to each other |
| `s` | Send the scratchpad to the language model |
| `q` | Quit |

Editing opens the program named in `$EDITOR` (falling back to `$VISUAL`, then
`vi`). Links are ordinary Markdown links, `[title](path)`; `[[wikilinks]]` are
also recognised.

## Related notes and language-model features

With Ollama running, constellate computes an embedding for each note and adds
semantically similar notes to the related list. Pull the embedding model once:

```
ollama pull nomic-embed-text
```

The scratchpad's `s` command sends the collected notes to a chat model and writes
the response to a new note in your vault, tagged `#TODO` and linked back to the
sources. It uses `qwen2.5` by default:

```
ollama pull qwen2.5
```

Both models are configurable with `--embed-model` and `--llm-model`, and the
server address with `--ollama-url`. If you would rather not run Ollama for
embeddings, build with `--features fastembed` to embed notes in-process instead.

## Options

Run `constellate --help` for the full list. The common ones are `--vault`,
`--no-embed`, `--embed-model`, `--llm-model`, and `--ref-format` (how `y`
formats a copied link: `markdown`, `relative`, or `absolute`).
