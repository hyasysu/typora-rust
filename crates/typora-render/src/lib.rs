use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use typora_md::{BlockId, MarkdownBlock, MarkdownBlockKind, MarkdownSnapshot, SourceSpan};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct LayoutCacheKey(pub u64);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RenderBlockKind {
    Heading {
        level: u8,
        title: String,
    },
    Paragraph {
        text: String,
    },
    List {
        text: String,
    },
    TaskList {
        text: String,
    },
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    Quote {
        text: String,
    },
    Rule,
    Table {
        text: String,
    },
    HtmlPlaceholder {
        text: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RenderBlock {
    pub id: BlockId,
    pub source_span: SourceSpan,
    pub kind: RenderBlockKind,
    pub layout_cache_key: LayoutCacheKey,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PreviewRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LaidOutBlock {
    pub block: RenderBlock,
    pub rect: PreviewRect,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PreviewLayout {
    pub blocks: Vec<LaidOutBlock>,
    pub total_height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutConfig {
    pub width: f32,
    pub font_size: f32,
    pub line_height: f32,
    pub block_gap: f32,
    pub padding: f32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            width: 720.0,
            font_size: 16.0,
            line_height: 24.0,
            block_gap: 12.0,
            padding: 24.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitPaneLayout {
    pub source: PreviewRect,
    pub preview: PreviewRect,
    pub status: PreviewRect,
}

pub struct BlockLayoutEngine {
    config: LayoutConfig,
}

impl BlockLayoutEngine {
    #[must_use]
    pub const fn new(config: LayoutConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn render_blocks(&self, snapshot: &MarkdownSnapshot) -> Vec<RenderBlock> {
        snapshot.blocks.iter().map(block_to_render).collect()
    }

    #[must_use]
    pub fn layout(&self, blocks: &[RenderBlock], viewport: Option<PreviewRect>) -> PreviewLayout {
        let mut y = self.config.padding;
        let mut laid_out = Vec::new();
        let width = self.config.width - self.config.padding * 2.0;
        let viewport_range = viewport.map(|rect| rect.y..rect.y + rect.height);

        for block in blocks {
            let height = self.estimate_height(block, width);
            let rect = PreviewRect {
                x: self.config.padding,
                y,
                width,
                height,
            };
            let visible = viewport_range
                .as_ref()
                .is_none_or(|range| rect.y + rect.height >= range.start && rect.y <= range.end);
            if visible {
                laid_out.push(LaidOutBlock {
                    block: block.clone(),
                    rect,
                });
            }
            y += height + self.config.block_gap;
        }

        PreviewLayout {
            blocks: laid_out,
            total_height: y + self.config.padding,
        }
    }

    fn estimate_height(&self, block: &RenderBlock, width: f32) -> f32 {
        let text = match &block.kind {
            RenderBlockKind::Heading { title, .. } => title,
            RenderBlockKind::Paragraph { text }
            | RenderBlockKind::List { text }
            | RenderBlockKind::TaskList { text }
            | RenderBlockKind::Quote { text }
            | RenderBlockKind::Table { text }
            | RenderBlockKind::HtmlPlaceholder { text } => text,
            RenderBlockKind::CodeBlock { code, .. } => code,
            RenderBlockKind::Rule => return 2.0,
        };

        let avg_char_width = self.config.font_size * 0.55;
        let chars_per_line = (width / avg_char_width).max(8.0) as usize;
        let logical_lines = text
            .lines()
            .map(|line| (line.chars().count() / chars_per_line).max(1))
            .sum::<usize>()
            .max(1);
        let multiplier = match block.kind {
            RenderBlockKind::Heading { level: 1, .. } => 1.7,
            RenderBlockKind::Heading { level: 2, .. } => 1.45,
            RenderBlockKind::Heading { .. } => 1.25,
            RenderBlockKind::CodeBlock { .. } => 1.05,
            _ => 1.0,
        };
        logical_lines as f32 * self.config.line_height * multiplier
    }
}

#[must_use]
pub fn block_to_render(block: &MarkdownBlock) -> RenderBlock {
    let kind = match &block.kind {
        MarkdownBlockKind::Heading { level, title } => RenderBlockKind::Heading {
            level: *level,
            title: title.clone(),
        },
        MarkdownBlockKind::Paragraph => RenderBlockKind::Paragraph {
            text: block.text.clone(),
        },
        MarkdownBlockKind::List => RenderBlockKind::List {
            text: block.text.clone(),
        },
        MarkdownBlockKind::TaskList => RenderBlockKind::TaskList {
            text: block.text.clone(),
        },
        MarkdownBlockKind::CodeBlock { language } => RenderBlockKind::CodeBlock {
            language: language.clone(),
            code: block.text.clone(),
        },
        MarkdownBlockKind::Quote => RenderBlockKind::Quote {
            text: block.text.clone(),
        },
        MarkdownBlockKind::Rule => RenderBlockKind::Rule,
        MarkdownBlockKind::Table => RenderBlockKind::Table {
            text: block.text.clone(),
        },
        MarkdownBlockKind::Html => RenderBlockKind::HtmlPlaceholder {
            text: block.text.clone(),
        },
    };

    RenderBlock {
        id: block.id,
        source_span: block.source_span,
        layout_cache_key: cache_key(block),
        kind,
    }
}

#[must_use]
pub fn compute_split_pane_layout(width: f32, height: f32) -> SplitPaneLayout {
    let status_height = 28.0;
    let content_height = (height - status_height).max(0.0);
    let left_width = (width * 0.5).floor();
    SplitPaneLayout {
        source: PreviewRect {
            x: 0.0,
            y: 0.0,
            width: left_width,
            height: content_height,
        },
        preview: PreviewRect {
            x: left_width,
            y: 0.0,
            width: (width - left_width).max(0.0),
            height: content_height,
        },
        status: PreviewRect {
            x: 0.0,
            y: content_height,
            width,
            height: status_height,
        },
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScrollSyncMap {
    entries: Vec<ScrollSyncEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScrollSyncEntry {
    pub block_id: BlockId,
    pub source_span: SourceSpan,
    pub preview_y: f32,
}

impl ScrollSyncMap {
    #[must_use]
    pub fn from_layout(layout: &PreviewLayout) -> Self {
        let entries = layout
            .blocks
            .iter()
            .map(|block| ScrollSyncEntry {
                block_id: block.block.id,
                source_span: block.block.source_span,
                preview_y: block.rect.y,
            })
            .collect();
        Self { entries }
    }

    #[must_use]
    pub fn preview_y_for_source_line(&self, line: usize) -> Option<f32> {
        self.entries
            .iter()
            .find(|entry| entry.source_span.contains_line(line))
            .or_else(|| {
                self.entries
                    .iter()
                    .rfind(|entry| entry.source_span.start_line <= line)
            })
            .map(|entry| entry.preview_y)
    }

    #[must_use]
    pub fn source_line_for_preview_y(&self, y: f32) -> Option<usize> {
        self.entries
            .iter()
            .rfind(|entry| entry.preview_y <= y)
            .or_else(|| self.entries.first())
            .map(|entry| entry.source_span.start_line)
    }
}

#[must_use]
pub fn plain_preview_text(blocks: &[RenderBlock]) -> String {
    blocks
        .iter()
        .map(plain_preview_block)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn plain_preview_block(block: &RenderBlock) -> String {
    match &block.kind {
        RenderBlockKind::Heading { title, .. } => title.clone(),
        RenderBlockKind::Paragraph { text } => strip_inline_markdown(text),
        RenderBlockKind::List { text } => text
            .lines()
            .map(|line| format!("- {}", strip_list_marker(line).trim()))
            .collect::<Vec<_>>()
            .join("\n"),
        RenderBlockKind::TaskList { text } => text
            .lines()
            .map(|line| strip_task_marker(line).to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        RenderBlockKind::CodeBlock { language, code } => {
            let code = strip_fence(code);
            match language {
                Some(language) => format!("Code ({language})\n{code}"),
                None => format!("Code\n{code}"),
            }
        }
        RenderBlockKind::Quote { text } => text
            .lines()
            .map(|line| strip_quote_marker(line).trim().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        RenderBlockKind::Rule => "----------------".to_string(),
        RenderBlockKind::Table { text } => text.clone(),
        RenderBlockKind::HtmlPlaceholder { text } => format!("[HTML]\n{text}"),
    }
}

fn strip_inline_markdown(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '`' | '*' | '_' => {}
            '[' => {
                let mut label = String::new();
                for next in chars.by_ref() {
                    if next == ']' {
                        break;
                    }
                    label.push(next);
                }
                if chars.peek() == Some(&'(') {
                    for next in chars.by_ref() {
                        if next == ')' {
                            break;
                        }
                    }
                }
                stripped.push_str(&label);
            }
            _ => stripped.push(ch),
        }
    }
    stripped
}

fn strip_list_marker(line: &str) -> &str {
    let trimmed = line.trim_start();
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest;
        }
    }
    if let Some((number, rest)) = trimmed.split_once('.')
        && !number.is_empty()
        && number.chars().all(|ch| ch.is_ascii_digit())
        && rest.starts_with(char::is_whitespace)
    {
        return rest.trim_start();
    }
    trimmed
}

fn strip_task_marker(line: &str) -> &str {
    let trimmed = line.trim_start();
    for prefix in ["- [ ] ", "* [ ] ", "+ [ ] "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest;
        }
    }
    for prefix in ["- [x] ", "- [X] ", "* [x] ", "* [X] ", "+ [x] ", "+ [X] "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest;
        }
    }
    trimmed
}

fn strip_quote_marker(line: &str) -> &str {
    line.trim_start()
        .strip_prefix('>')
        .map(str::trim_start)
        .unwrap_or_else(|| line.trim_start())
}

fn strip_fence(code: &str) -> String {
    let mut lines = code.lines();
    let first = lines.next();
    let mut body = match first {
        Some(line)
            if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") =>
        {
            lines.collect::<Vec<_>>()
        }
        Some(line) => {
            let mut collected = vec![line];
            collected.extend(lines);
            collected
        }
        None => Vec::new(),
    };
    if body.last().is_some_and(|line| {
        line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~")
    }) {
        body.pop();
    }
    body.join("\n")
}

fn cache_key(block: &MarkdownBlock) -> LayoutCacheKey {
    let mut hasher = DefaultHasher::new();
    block.id.hash(&mut hasher);
    block.source_span.hash(&mut hasher);
    block.text.hash(&mut hasher);
    LayoutCacheKey(hasher.finish())
}

#[cfg(test)]
mod tests {
    use typora_core::{DocVersion, DocumentId};
    use typora_md::MarkdownParser;

    use super::*;

    #[test]
    fn converts_snapshot_to_render_blocks() {
        let mut parser = MarkdownParser::new().unwrap();
        let snapshot = parser.parse(DocumentId(1), DocVersion(1), "# Title\n\nBody");
        let engine = BlockLayoutEngine::new(LayoutConfig::default());
        let blocks = engine.render_blocks(&snapshot);

        assert!(matches!(blocks[0].kind, RenderBlockKind::Heading { .. }));
        assert!(matches!(blocks[1].kind, RenderBlockKind::Paragraph { .. }));
    }

    #[test]
    fn viewport_layout_skips_distant_blocks() {
        let mut parser = MarkdownParser::new().unwrap();
        let snapshot = parser.parse(DocumentId(1), DocVersion(1), &"# H\n\ntext\n\n".repeat(100));
        let engine = BlockLayoutEngine::new(LayoutConfig::default());
        let blocks = engine.render_blocks(&snapshot);
        let layout = engine.layout(
            &blocks,
            Some(PreviewRect {
                x: 0.0,
                y: 0.0,
                width: 500.0,
                height: 300.0,
            }),
        );

        assert!(layout.blocks.len() < blocks.len());
        assert!(layout.total_height > 300.0);
    }

    #[test]
    fn scroll_sync_falls_back_to_nearest_previous_block() {
        let mut parser = MarkdownParser::new().unwrap();
        let snapshot = parser.parse(DocumentId(1), DocVersion(1), "# A\n\ntext\n\n# B");
        let engine = BlockLayoutEngine::new(LayoutConfig::default());
        let blocks = engine.render_blocks(&snapshot);
        let layout = engine.layout(&blocks, None);
        let sync = ScrollSyncMap::from_layout(&layout);

        assert_eq!(sync.source_line_for_preview_y(0.0), Some(0));
        assert!(sync.preview_y_for_source_line(4).is_some());
    }

    #[test]
    fn plain_preview_strips_markdown_control_syntax() {
        let mut parser = MarkdownParser::new().unwrap();
        let snapshot = parser.parse(
            DocumentId(1),
            DocVersion(1),
            "# Title\n\nA **bold** [link](https://example.com).\n\n- [x] done\n\n```rust\nfn main() {}\n```\n\n> quote\n",
        );
        let engine = BlockLayoutEngine::new(LayoutConfig::default());
        let blocks = engine.render_blocks(&snapshot);
        let preview = plain_preview_text(&blocks);

        assert!(preview.contains("Title"));
        assert!(preview.contains("A bold link."));
        assert!(preview.contains("done"));
        assert!(preview.contains("Code (rust)"));
        assert!(preview.contains("fn main() {}"));
        assert!(preview.contains("quote"));
        assert!(!preview.contains("```"));
        assert!(!preview.contains("**"));
        assert!(!preview.contains("](https://example.com)"));
    }
}
