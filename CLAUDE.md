# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo fmt --all                      # Format all crates
cargo check --workspace --all-targets # Type check all crates
cargo clippy --workspace --all-targets -- -D warnings  # Lint (CI only runs on macOS)
cargo test --workspace --all-targets # Run all tests
cargo run -p typora-app              # Run the app
cargo run -p typora-app -- path/to/file.md  # Run with a file
```

Benchmarks exist in `typora-core` and `typora-render` via `criterion`.

## Architecture

**typora-app** — Floem application shell, window management, and command wiring. Entry point and UI orchestration.

**typora-core** — Core document model using Ropey for the text buffer. Handles selection, editing commands, and undo/redo state.

**typora-md** — Markdown parsing via Comrak and tree-sitter. Builds and maintains a block index over the parsed document.

**typora-render** — Renders parsed markdown blocks to the preview pane. Uses Parley for text layout and Taffy for panel layout.

**typora-platform** — Platform-specific functionality: file I/O, native dialogs (rfd), file watching (notify), and front matter parsing (serde_yaml_ng, toml).

## Tech Stack

- UI framework: Floem with winit
- Text buffer: Ropey
- Markdown parser: Comrak + tree-sitter-markdown
- Text layout: Parley
- Panel layout: Taffy
- File watching: notify
- Native dialogs: rfd

## Notes

- Rust edition 2024 is used across all crates
- CI runs format and clippy checks only on macOS
- The MVP establishes editable document core, parser/render snapshots, file handling, scroll sync model, and a native Floem shell with source/preview two-pane layout