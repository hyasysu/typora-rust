use std::cell::RefCell;
use std::env;
use std::path::PathBuf;
use std::rc::Rc;

use floem::kurbo::Point;
use floem::prelude::*;
use floem::text::FamilyOwned;
use floem::views::editor::text::{Document, SimpleStyling};
use floem::views::text_editor::text_editor;
use tracing_subscriber::EnvFilter;
use typora_core::{DocumentState, TextPosition};
use typora_md::{MarkdownParser, MarkdownSnapshot};
use typora_platform::{
    choose_markdown_open_path, choose_markdown_save_path, document_from_file, write_markdown_file,
};
use typora_render::{BlockLayoutEngine, LayoutConfig, plain_preview_text};

mod ty_document;
use ty_document::TyDocument;

const SOURCE_FONT_SIZE: f64 = 15.0;
const SOURCE_LINE_HEIGHT: f64 = 22.0;
const SOURCE_CHAR_WIDTH: f64 = 9.0;
const MAX_SOURCE_WRAP_COLUMNS: usize = 38;
const MAX_PREVIEW_WRAP_COLUMNS: usize = 42;
const DEFAULT_SOURCE_WRAP_COLUMNS: usize = MAX_SOURCE_WRAP_COLUMNS;
const DEFAULT_PREVIEW_WRAP_COLUMNS: usize = MAX_PREVIEW_WRAP_COLUMNS;
const MIN_WRAP_COLUMNS: usize = 16;
const PANE_PADDING: f64 = 16.0;
const PANE_HORIZONTAL_INSET: f64 = 52.0;

#[derive(Clone)]
struct AppModel {
    document: Rc<RefCell<DocumentState>>,
    snapshot: MarkdownSnapshot,
    preview_text: String,
}

impl AppModel {
    fn new(document: Rc<RefCell<DocumentState>>) -> Self {
        let (snapshot, preview_text) = Self::build_preview(&document.borrow());
        Self {
            document,
            snapshot,
            preview_text,
        }
    }

    fn from_document(document: DocumentState) -> Self {
        Self::new(Rc::new(RefCell::new(document)))
    }

    fn open_initial() -> Self {
        let maybe_path = env::args_os().nth(1).map(PathBuf::from);
        match maybe_path {
            Some(path) => match document_from_file(&path) {
                Ok(document) => Self::from_document(document),
                Err(err) => {
                    tracing::warn!(?path, ?err, "failed to open requested markdown file");
                    Self::sample()
                }
            },
            None => Self::sample(),
        }
    }

    fn sample() -> Self {
        Self::from_document(DocumentState::from_text(
            "# Typora Rust\n\nStart writing on the left; the preview model renders on the right.\n\n- Ropey document core\n- Comrak + tree-sitter snapshot\n- Floem native shell\n\n```rust\nfn main() {\n    println!(\"hello markdown\");\n}\n```\n",
        ))
    }

    fn doc_mut(&self) -> std::cell::RefMut<'_, DocumentState> {
        self.document.borrow_mut()
    }

    fn insert_sample_line(&mut self) {
        let mut doc = self.document.borrow_mut();
        doc.apply(typora_core::EditorCommand::MoveCursor(
            typora_core::CursorMove::DocumentEnd,
        ))
        .ok();
        doc.apply(typora_core::EditorCommand::InsertText(
            "\nNew paragraph from command bus.\n".to_string(),
        ))
        .ok();
        drop(doc);
        self.refresh_preview();
    }

    fn open_via_dialog(&mut self) {
        let Some(path) = choose_markdown_open_path() else {
            return;
        };
        match document_from_file(&path) {
            Ok(document) => {
                *self.doc_mut() = document;
                self.refresh_preview();
            }
            Err(err) => tracing::warn!(?path, ?err, "failed to open markdown file"),
        }
    }

    fn save_current(&mut self) {
        let path = match self
            .document
            .borrow()
            .path
            .clone()
            .or_else(choose_markdown_save_path)
        {
            Some(path) => path,
            None => return,
        };
        self.save_as(path);
    }

    fn save_as(&mut self, path: PathBuf) {
        let text = self.document.borrow().text();
        match write_markdown_file(&path, &text) {
            Ok(()) => self.document.borrow_mut().mark_saved_as(path),
            Err(err) => tracing::warn!(?err, "failed to save markdown file"),
        }
        self.refresh_preview();
    }

    fn refresh_preview(&mut self) {
        let (snapshot, preview_text) = Self::build_preview(&self.document.borrow());
        self.snapshot = snapshot;
        self.preview_text = preview_text;
    }

    fn build_preview(document: &DocumentState) -> (MarkdownSnapshot, String) {
        let mut parser =
            MarkdownParser::new().expect("tree-sitter markdown parser is available");
        let snapshot = parser.parse(document.id, document.version, &document.text());
        let preview_text = preview_text_from_snapshot(&snapshot);
        (snapshot, preview_text)
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()),
        )
        .init();
    unsafe { std::env::set_var("RUST_LOG", "debug"); }
    floem::launch(app_view);
}

fn app_view() -> impl IntoView {
    let model = RwSignal::new(AppModel::open_initial());
    let source_wrap_columns = RwSignal::new(DEFAULT_SOURCE_WRAP_COLUMNS);
    let preview_wrap_columns = RwSignal::new(DEFAULT_PREVIEW_WRAP_COLUMNS);
    let doc_changed = RwSignal::new(0u64);

    // Create TyDocument sharing the same document state
    let ty_doc = Rc::new(TyDocument::new(
        model.with(|m| m.document.clone()),
        doc_changed,
    ));

    // Build a styling with Latin + CJK font fallback
    let font_families: Vec<FamilyOwned> =
        FamilyOwned::parse_list("Menlo, PingFang SC").collect();
    let editor_styling = SimpleStyling::builder()
        .font_size(SOURCE_FONT_SIZE as usize)
        .line_height((SOURCE_LINE_HEIGHT / SOURCE_FONT_SIZE) as f32)
        .font_family(font_families)
        .build();

    // Create text_editor with our custom document
    let editor_view = text_editor(ty_doc.text())
        .use_doc(ty_doc.clone())
        .styling(editor_styling)
        .keyboard_navigable()
        .style(|style| {
            style
                .width_full()
                .height_full()
        });

    let source_pane = editor_view
        .on_resize(move |rect| {
            source_wrap_columns.set(wrap_columns_for_width(
                rect.width(),
                SOURCE_CHAR_WIDTH,
                MAX_SOURCE_WRAP_COLUMNS,
            ));
        })
        .style(|style| {
            style
                .min_width(0.0)
                .max_width_pct(50.0)
                .flex_basis(0.0)
                .flex_grow(1.0)
                .flex_shrink(1.0)
                .height_full()
                .padding(PANE_PADDING)
                .border(1.0)
        });

    let preview = move || {
        let _ = doc_changed.get();
        let wrap_columns = preview_wrap_columns.get();
        model.with(|model| {
            let doc = model.document.borrow();
            let preview_text = preview_text_for_document(&doc);
            soft_wrap_text(&preview_text, wrap_columns)
        })
    };

    let status = move || {
        let _ = doc_changed.get();
        model.with(|model| {
            let doc = model.document.borrow();
            let path = doc
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Untitled.md".to_string());
            format!(
                "{} | {} chars | version {}{}",
                path,
                doc.char_count(),
                doc.version.0,
                if doc.dirty { " | dirty" } else { "" }
            )
        })
    };

    v_stack((
        h_stack((
            button(text("Open")).on_click_stop(move |_| {
                model.update(AppModel::open_via_dialog);
                doc_changed.update(|c| *c += 1);
            }),
            button(text("Save")).on_click_stop(move |_| {
                model.update(AppModel::save_current);
            }),
            button(text("Insert paragraph")).on_click_stop(move |_| {
                model.update(|model| model.insert_sample_line());
                doc_changed.update(|c| *c += 1);
            }),
            label(|| "Rust native Markdown editor MVP"),
        ))
        .style(|style| style.gap(12.0).padding(10.0)),
        h_stack((
            source_pane,
            container(scroll(label(preview).style(|style| {
                style
                    .font_size(16.0)
                    .line_height(1.5)
                    .width_full()
                    .max_width_full()
            })))
            .on_resize(move |rect| {
                preview_wrap_columns.set(wrap_columns_for_width(
                    rect.width(),
                    8.8,
                    MAX_PREVIEW_WRAP_COLUMNS,
                ));
            })
            .style(|style| {
                style
                    .min_width(0.0)
                    .max_width_pct(50.0)
                    .flex_basis(0.0)
                    .flex_grow(1.0)
                    .flex_shrink(1.0)
                    .height_full()
                    .padding(PANE_PADDING)
                    .border(1.0)
            }),
        ))
        .style(|style| style.height_full().width_full()),
        label(status).style(|style| style.padding(8.0)),
    ))
    .style(|style| style.size_full())
}

fn preview_text_for_document(doc: &DocumentState) -> String {
    let mut parser =
        MarkdownParser::new().expect("tree-sitter markdown parser is available");
    let snapshot = parser.parse(doc.id, doc.version, &doc.text());
    let engine = BlockLayoutEngine::new(LayoutConfig::default());
    let blocks = engine.render_blocks(&snapshot);
    plain_preview_text(&blocks)
}

fn preview_text_from_snapshot(snapshot: &MarkdownSnapshot) -> String {
    let engine = BlockLayoutEngine::new(LayoutConfig::default());
    let blocks = engine.render_blocks(snapshot);
    plain_preview_text(&blocks)
}

fn wrap_columns_for_width(width: f64, char_width: f64, max_columns: usize) -> usize {
    let columns = ((width - PANE_HORIZONTAL_INSET).max(char_width * MIN_WRAP_COLUMNS as f64)
        / char_width)
        .floor() as usize;
    columns.clamp(MIN_WRAP_COLUMNS, max_columns)
}

fn soft_wrap_text(text: &str, wrap_columns: usize) -> String {
    let wrap_columns = wrap_columns.max(1);
    let mut wrapped = String::with_capacity(text.len() + text.len() / wrap_columns);
    for (line_index, line) in text.lines().enumerate() {
        if line_index > 0 {
            wrapped.push('\n');
        }
        soft_wrap_line_into(line, wrap_columns, &mut wrapped);
    }
    if text.ends_with('\n') {
        wrapped.push('\n');
    }
    wrapped
}

fn soft_wrap_line_into(line: &str, wrap_columns: usize, out: &mut String) {
    if line.is_empty() {
        return;
    }
    let mut col = 0;
    for ch in line.chars() {
        if col == wrap_columns {
            out.push('\n');
            col = 0;
        }
        out.push(ch);
        col += 1;
    }
}

#[allow(dead_code)]
fn point_inside_source_text(point: Point) -> Point {
    Point::new(
        (point.x - PANE_PADDING).max(0.0),
        (point.y - PANE_PADDING).max(0.0),
    )
}

#[allow(dead_code)]
fn text_position_from_local_point(
    document: &DocumentState,
    point: Point,
    wrap_columns: usize,
) -> TextPosition {
    let visual_row = (point.y.max(0.0) / SOURCE_LINE_HEIGHT).floor() as usize;
    let visual_col = (point.x.max(0.0) / SOURCE_CHAR_WIDTH).round() as usize;
    visual_position_to_text_position(document, visual_row, visual_col, wrap_columns)
}

#[allow(dead_code)]
fn cursor_visual_position(document: &DocumentState, wrap_columns: usize) -> (usize, usize) {
    let position = document.char_to_position(document.selection.cursor());
    text_position_to_visual_position(document, position, wrap_columns)
}

#[allow(dead_code)]
fn text_position_to_visual_position(
    document: &DocumentState,
    position: TextPosition,
    wrap_columns: usize,
) -> (usize, usize) {
    let wrap_columns = wrap_columns.max(1);
    let line = position
        .line
        .min(document.line_count().saturating_sub(1));
    let visual_row_offset = (0..line)
        .map(|line| visual_rows_for_len(document.line_visible_char_count(line), wrap_columns))
        .sum::<usize>();
    let character = position
        .character
        .min(document.line_visible_char_count(line));
    (
        visual_row_offset + character / wrap_columns,
        character % wrap_columns,
    )
}

#[allow(dead_code)]
fn visual_position_to_text_position(
    document: &DocumentState,
    visual_row: usize,
    visual_col: usize,
    wrap_columns: usize,
) -> TextPosition {
    let wrap_columns = wrap_columns.max(1);
    let mut row_base = 0;
    for line in 0..document.line_count() {
        let line_len = document.line_visible_char_count(line);
        let visual_rows = visual_rows_for_len(line_len, wrap_columns);
        if visual_row < row_base + visual_rows {
            let wrapped_row = visual_row - row_base;
            let character = wrapped_row * wrap_columns + visual_col;
            return TextPosition {
                line,
                character: character.min(line_len),
            };
        }
        row_base += visual_rows;
    }

    let last_line = document.line_count().saturating_sub(1);
    TextPosition {
        line: last_line,
        character: visual_col.min(document.line_visible_char_count(last_line)),
    }
}

fn visual_rows_for_len(char_len: usize, wrap_columns: usize) -> usize {
    char_len.div_ceil(wrap_columns).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_position_uses_local_view_coordinates() {
        let document = DocumentState::from_text("abcd\n中文🙂\nlast");

        assert_eq!(
            text_position_from_local_point(
                &document,
                Point::new(0.0, 0.0),
                DEFAULT_SOURCE_WRAP_COLUMNS
            ),
            TextPosition {
                line: 0,
                character: 0
            }
        );
        assert_eq!(
            text_position_from_local_point(
                &document,
                Point::new(SOURCE_CHAR_WIDTH * 99.0, SOURCE_LINE_HEIGHT * 1.2),
                DEFAULT_SOURCE_WRAP_COLUMNS
            ),
            TextPosition {
                line: 1,
                character: 3
            }
        );
        assert_eq!(
            text_position_from_local_point(
                &document,
                Point::new(0.0, SOURCE_LINE_HEIGHT * 99.0),
                DEFAULT_SOURCE_WRAP_COLUMNS
            ),
            TextPosition {
                line: 2,
                character: 0
            }
        );
    }

    #[test]
    fn source_display_soft_wraps_long_lines() {
        let long_line = "a".repeat(DEFAULT_SOURCE_WRAP_COLUMNS + 5);
        let model = AppModel::from_document(DocumentState::from_text(&long_line));
        let display =
            soft_wrap_text(&model.document.borrow().text(), DEFAULT_SOURCE_WRAP_COLUMNS);

        assert!(display.contains('\n'));
        assert!(
            display
                .lines()
                .all(|line| line.chars().count() <= DEFAULT_SOURCE_WRAP_COLUMNS)
        );
        assert!(!display.contains('|'));
    }

    #[test]
    fn pane_wrap_columns_are_capped_for_native_label_rendering() {
        assert_eq!(
            wrap_columns_for_width(10_000.0, SOURCE_CHAR_WIDTH, MAX_SOURCE_WRAP_COLUMNS),
            MAX_SOURCE_WRAP_COLUMNS
        );
        assert_eq!(
            wrap_columns_for_width(10_000.0, 8.8, MAX_PREVIEW_WRAP_COLUMNS),
            MAX_PREVIEW_WRAP_COLUMNS
        );
        assert_eq!(
            wrap_columns_for_width(1.0, SOURCE_CHAR_WIDTH, MAX_SOURCE_WRAP_COLUMNS),
            MIN_WRAP_COLUMNS
        );
    }

    #[test]
    fn source_pane_hit_testing_accounts_for_padding() {
        let point = point_inside_source_text(Point::new(
            PANE_PADDING + SOURCE_CHAR_WIDTH * 3.0,
            PANE_PADDING,
        ));
        assert!((point.x - SOURCE_CHAR_WIDTH * 3.0).abs() < 1e-9);
        assert_eq!(point.y, 0.0);
        assert_eq!(point_inside_source_text(Point::ZERO), Point::ZERO);
    }

    #[test]
    fn sample_source_wraps_before_the_pane_divider() {
        let model = AppModel::sample();
        let display = soft_wrap_text(&model.document.borrow().text(), DEFAULT_SOURCE_WRAP_COLUMNS);

        assert!(display.contains("the preview\n model renders"));
        assert!(
            display
                .lines()
                .all(|line| line.chars().count() <= MAX_SOURCE_WRAP_COLUMNS)
        );
    }

    #[test]
    fn mouse_position_accounts_for_soft_wrapped_rows() {
        let long_line = "a".repeat(DEFAULT_SOURCE_WRAP_COLUMNS + 10);
        let document = DocumentState::from_text(&format!("{long_line}\nsecond"));

        assert_eq!(
            visual_position_to_text_position(&document, 1, 3, DEFAULT_SOURCE_WRAP_COLUMNS),
            TextPosition {
                line: 0,
                character: DEFAULT_SOURCE_WRAP_COLUMNS + 3
            }
        );
        assert_eq!(
            visual_position_to_text_position(&document, 2, 2, DEFAULT_SOURCE_WRAP_COLUMNS),
            TextPosition {
                line: 1,
                character: 2
            }
        );
    }

    #[test]
    fn caret_overlay_position_does_not_mutate_display_text() {
        let mut document = DocumentState::from_text("abcd\nefgh");
        document
            .apply(typora_core::EditorCommand::MoveCursor(
                typora_core::CursorMove::ToPosition(TextPosition {
                    line: 1,
                    character: 2,
                }),
            ))
            .unwrap();
        let model = AppModel::from_document(document.clone());

        assert_eq!(
            cursor_visual_position(&document, DEFAULT_SOURCE_WRAP_COLUMNS),
            (1, 2)
        );
        assert_eq!(model.document.borrow().text(), "abcd\nefgh");
    }

    #[test]
    fn backspace_deletes_character_left_of_visual_caret() {
        let mut document = DocumentState::from_text("abcd");
        document
            .apply(typora_core::EditorCommand::MoveCursor(
                typora_core::CursorMove::ToPosition(TextPosition {
                    line: 0,
                    character: 2,
                }),
            ))
            .unwrap();

        assert_eq!(
            cursor_visual_position(&document, DEFAULT_SOURCE_WRAP_COLUMNS),
            (0, 2)
        );

        document
            .apply(typora_core::EditorCommand::DeleteBackward)
            .unwrap();

        assert_eq!(document.text(), "acd");
        assert_eq!(
            cursor_visual_position(&document, DEFAULT_SOURCE_WRAP_COLUMNS),
            (0, 1)
        );
    }

    #[test]
    fn app_preview_drops_raw_markdown_fences() {
        let model = AppModel::from_document(DocumentState::from_text(
            "# Title\n\nA **bold** paragraph.\n\n```rust\nfn main() {}\n```\n",
        ));

        assert!(model.preview_text.contains("Title"));
        assert!(model.preview_text.contains("A bold paragraph."));
        assert!(model.preview_text.contains("Code (rust)"));
        assert!(!model.preview_text.contains("```"));
        assert!(!model.preview_text.contains("**"));
    }
}
