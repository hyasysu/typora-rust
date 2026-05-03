use std::ops::Range;

use comrak::{Options, markdown_to_html};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tree_sitter::Parser;
use typora_core::{DocVersion, DocumentId};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct BlockId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start_line: usize,
    pub end_line: usize,
    pub start_char: usize,
    pub end_char: usize,
}

impl SourceSpan {
    #[must_use]
    pub const fn contains_line(self, line: usize) -> bool {
        self.start_line <= line && line <= self.end_line
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MarkdownBlockKind {
    Heading { level: u8, title: String },
    Paragraph,
    List,
    TaskList,
    CodeBlock { language: Option<String> },
    Quote,
    Rule,
    Table,
    Html,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MarkdownBlock {
    pub id: BlockId,
    pub kind: MarkdownBlockKind,
    pub source_span: SourceSpan,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeadingEntry {
    pub level: u8,
    pub title: String,
    pub source_span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MarkdownSnapshot {
    pub document: DocumentId,
    pub version: DocVersion,
    pub blocks: Vec<MarkdownBlock>,
    pub outline: Vec<HeadingEntry>,
    pub html: String,
    pub tree_root_kind: String,
    pub parser_had_error: bool,
}

#[derive(Debug, Error)]
pub enum MarkdownError {
    #[error("failed to configure tree-sitter markdown parser")]
    TreeSitterLanguage(#[from] tree_sitter::LanguageError),
}

pub struct MarkdownParser {
    parser: Parser,
}

impl MarkdownParser {
    pub fn new() -> Result<Self, MarkdownError> {
        let mut parser = Parser::new();
        parser.set_language(tree_sitter_markdown::language())?;
        Ok(Self { parser })
    }

    pub fn parse(
        &mut self,
        document: DocumentId,
        version: DocVersion,
        text: &str,
    ) -> MarkdownSnapshot {
        let tree = self.parser.parse(text, None);
        let (tree_root_kind, parser_had_error) = tree
            .as_ref()
            .map(|tree| {
                let root = tree.root_node();
                (root.kind().to_string(), root.has_error())
            })
            .unwrap_or_else(|| ("document".to_string(), true));

        let html = markdown_to_html(text, &markdown_options());
        let blocks = scan_blocks(text);
        let outline = blocks
            .iter()
            .filter_map(|block| match &block.kind {
                MarkdownBlockKind::Heading { level, title } => Some(HeadingEntry {
                    level: *level,
                    title: title.clone(),
                    source_span: block.source_span,
                }),
                _ => None,
            })
            .collect();

        MarkdownSnapshot {
            document,
            version,
            blocks,
            outline,
            html,
            tree_root_kind,
            parser_had_error,
        }
    }
}

fn markdown_options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.footnotes = true;
    options
}

#[derive(Clone, Debug)]
struct SourceLine {
    number: usize,
    start_char: usize,
    end_char: usize,
    text: String,
}

fn scan_blocks(text: &str) -> Vec<MarkdownBlock> {
    let lines = source_lines(text);
    let mut blocks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        if lines[i].text.trim().is_empty() {
            i += 1;
            continue;
        }

        let start = i;
        let trimmed = lines[i].text.trim_start();
        let kind = if let Some((level, title)) = heading(trimmed) {
            i += 1;
            MarkdownBlockKind::Heading { level, title }
        } else if let Some(language) = fence_language(trimmed) {
            i += 1;
            while i < lines.len() && !is_closing_fence(lines[i].text.trim_start()) {
                i += 1;
            }
            if i < lines.len() {
                i += 1;
            }
            MarkdownBlockKind::CodeBlock { language }
        } else if is_rule(trimmed) {
            i += 1;
            MarkdownBlockKind::Rule
        } else if is_table_start(&lines, i) {
            i += 2;
            while i < lines.len() && lines[i].text.contains('|') && !lines[i].text.trim().is_empty()
            {
                i += 1;
            }
            MarkdownBlockKind::Table
        } else if trimmed.starts_with('>') {
            i = consume_while(&lines, i, |line| line.trim_start().starts_with('>'));
            MarkdownBlockKind::Quote
        } else if is_task_list(trimmed) {
            i = consume_while(&lines, i, |line| is_task_list(line.trim_start()));
            MarkdownBlockKind::TaskList
        } else if is_list(trimmed) {
            i = consume_while(&lines, i, |line| is_list(line.trim_start()));
            MarkdownBlockKind::List
        } else if trimmed.starts_with('<') {
            i = consume_until_blank(&lines, i);
            MarkdownBlockKind::Html
        } else {
            i = consume_paragraph(&lines, i);
            MarkdownBlockKind::Paragraph
        };

        let end = i.saturating_sub(1);
        let text = lines[start..=end]
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        blocks.push(MarkdownBlock {
            id: BlockId(blocks.len() as u64 + 1),
            kind,
            source_span: span_for_lines(&lines[start..=end]),
            text,
        });
    }

    blocks
}

fn source_lines(text: &str) -> Vec<SourceLine> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut char_offset = 0;
    for (number, raw) in text.split_inclusive('\n').enumerate() {
        let line_text = raw.trim_end_matches(['\r', '\n']).to_string();
        let line_chars = raw.chars().count();
        lines.push(SourceLine {
            number,
            start_char: char_offset,
            end_char: char_offset + line_chars,
            text: line_text,
        });
        char_offset += line_chars;
    }

    if !text.ends_with('\n') && lines.is_empty() {
        lines.push(SourceLine {
            number: 0,
            start_char: 0,
            end_char: text.chars().count(),
            text: text.to_string(),
        });
    }

    lines
}

fn span_for_lines(lines: &[SourceLine]) -> SourceSpan {
    let first = lines.first().expect("block has at least one line");
    let last = lines.last().expect("block has at least one line");
    SourceSpan {
        start_line: first.number,
        end_line: last.number,
        start_char: first.start_char,
        end_char: last.end_char,
    }
}

fn heading(trimmed: &str) -> Option<(u8, String)> {
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &trimmed[level..];
    rest.starts_with(char::is_whitespace).then(|| {
        (
            level as u8,
            rest.trim().trim_end_matches('#').trim().to_string(),
        )
    })
}

fn fence_language(trimmed: &str) -> Option<Option<String>> {
    let fence = if trimmed.starts_with("```") {
        "```"
    } else if trimmed.starts_with("~~~") {
        "~~~"
    } else {
        return None;
    };
    let language = trimmed
        .trim_start_matches(fence)
        .split_whitespace()
        .next()
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    Some(language)
}

fn is_closing_fence(trimmed: &str) -> bool {
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

fn is_rule(trimmed: &str) -> bool {
    let chars: Vec<_> = trimmed.chars().filter(|ch| !ch.is_whitespace()).collect();
    chars.len() >= 3
        && chars
            .iter()
            .all(|ch| matches!(ch, '-' | '_' | '*') && *ch == chars[0])
}

fn is_table_start(lines: &[SourceLine], index: usize) -> bool {
    let Some(next) = lines.get(index + 1) else {
        return false;
    };
    lines[index].text.contains('|') && is_table_separator(next.text.trim())
}

fn is_table_separator(trimmed: &str) -> bool {
    trimmed.contains('|')
        && trimmed
            .chars()
            .all(|ch| matches!(ch, '|' | '-' | ':' | ' ' | '\t'))
        && trimmed.contains('-')
}

fn is_task_list(trimmed: &str) -> bool {
    ["- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] ", "* [X] "]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

fn is_list(trimmed: &str) -> bool {
    if ["- ", "* ", "+ "]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
    {
        return true;
    }

    let Some((digits, rest)) = trimmed.split_once('.') else {
        return false;
    };
    !digits.is_empty()
        && digits.chars().all(|ch| ch.is_ascii_digit())
        && rest.starts_with(char::is_whitespace)
}

fn consume_while(
    lines: &[SourceLine],
    mut index: usize,
    predicate: impl Fn(&str) -> bool,
) -> usize {
    while index < lines.len()
        && !lines[index].text.trim().is_empty()
        && predicate(lines[index].text.as_str())
    {
        index += 1;
    }
    index
}

fn consume_until_blank(lines: &[SourceLine], mut index: usize) -> usize {
    while index < lines.len() && !lines[index].text.trim().is_empty() {
        index += 1;
    }
    index
}

fn consume_paragraph(lines: &[SourceLine], mut index: usize) -> usize {
    while index < lines.len() {
        let trimmed = lines[index].text.trim_start();
        if trimmed.is_empty() || starts_block(trimmed) {
            break;
        }
        index += 1;
    }
    index
}

fn starts_block(trimmed: &str) -> bool {
    heading(trimmed).is_some()
        || fence_language(trimmed).is_some()
        || is_rule(trimmed)
        || trimmed.starts_with('>')
        || is_task_list(trimmed)
        || is_list(trimmed)
        || trimmed.starts_with('<')
}

#[must_use]
pub fn block_for_line(blocks: &[MarkdownBlock], line: usize) -> Option<&MarkdownBlock> {
    blocks
        .iter()
        .find(|block| block.source_span.contains_line(line))
        .or_else(|| {
            blocks
                .iter()
                .rfind(|block| block.source_span.start_line <= line)
        })
}

#[must_use]
pub fn dirty_line_range(old_text: &str, new_text: &str) -> Range<usize> {
    let old_lines: Vec<_> = old_text.lines().collect();
    let new_lines: Vec<_> = new_text.lines().collect();
    let common_prefix = old_lines
        .iter()
        .zip(new_lines.iter())
        .take_while(|(old, new)| old == new)
        .count();
    let common_suffix = old_lines[common_prefix..]
        .iter()
        .rev()
        .zip(new_lines[common_prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    common_prefix..new_lines.len().saturating_sub(common_suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_extracts_outline_and_gfm_html() {
        let mut parser = MarkdownParser::new().unwrap();
        let snapshot = parser.parse(
            DocumentId(1),
            DocVersion(1),
            "# Title\n\n- [x] done\n\n| A | B |\n| - | - |\n| 1 | 2 |\n",
        );

        assert_eq!(snapshot.outline.len(), 1);
        assert!(snapshot.html.contains("<table>"));
        assert!(
            snapshot
                .blocks
                .iter()
                .any(|block| matches!(block.kind, MarkdownBlockKind::TaskList))
        );
    }

    #[test]
    fn block_scanner_tracks_code_fences_and_source_lines() {
        let blocks = scan_blocks("# T\n\n```rust\nfn main() {}\n```\n\ntext");
        assert!(matches!(
            blocks[1].kind,
            MarkdownBlockKind::CodeBlock {
                language: Some(ref lang)
            } if lang == "rust"
        ));
        assert_eq!(blocks[1].source_span.start_line, 2);
        assert_eq!(blocks[1].source_span.end_line, 4);
    }

    #[test]
    fn block_for_line_uses_nearest_previous_block() {
        let blocks = scan_blocks("# A\n\nparagraph\n\n# B");
        let block = block_for_line(&blocks, 3).unwrap();
        assert_eq!(block.source_span.start_line, 2);
    }

    #[test]
    fn dirty_range_finds_changed_middle() {
        assert_eq!(dirty_line_range("a\nb\nc", "a\nB\nc"), 1..2);
    }
}
