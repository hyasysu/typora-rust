# Typora Rust

Rust 原生性能型 Markdown 编辑器实验项目，目标是 macOS 优先、跨平台可演进的双栏源码预览 MVP。

## 技术栈

- UI/window：Floem + winit via framework
- Buffer：Ropey
- Parser：Comrak + tree-sitter Markdown
- Render：初期使用 Floem renderer，核心抽象预留 Vello/wgpu editor surface
- Text/layout：Parley 优先，渲染 crate 中保留可替换文本布局边界
- App layout：自定义双栏布局，后续接入 Taffy 管理复杂面板
- Files：notify、rfd、serde_yaml_ng、toml
- Packaging：cargo-packager + GitHub Actions per OS

## Workspace

```text
crates/
  typora-app       # Floem app shell and command wiring
  typora-core      # Ropey document model, selection, commands, undo/redo
  typora-md        # Comrak/tree-sitter parser snapshot and block index
  typora-render    # RenderBlock, preview layout, scroll sync
  typora-platform  # File IO, dialogs, file watching, front matter
```

## Development

```bash
cargo fmt --all
cargo test --workspace --all-targets
cargo run -p typora-app
cargo run -p typora-app -- path/to/file.md
```

## MVP scope

Current implementation establishes the editable document core, parser/render snapshots, file handling, scroll sync model, and a native Floem shell with a two-pane source/preview view. The source pane already routes keyboard and IME commit events through `typora-core`; the next milestone is replacing the label-based drawing with a dedicated `EditorView` paint surface, mouse selection, clipboard paste, and viewport line virtualization.
