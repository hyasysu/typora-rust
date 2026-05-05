use std::env;
use std::path::PathBuf;

use floem::action::{set_ime_allowed, set_ime_cursor_area};
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::{Point, Size};
use floem::Clipboard;
use floem::peniko::Color;
use floem::pointer::PointerButton;
use floem::prelude::*;
use floem::style::CursorStyle;
use tracing_subscriber::EnvFilter;
use typora_core::{CommandOutcome, CursorMove, DocumentState, EditorCommand, TextPosition};
use typora_md::{MarkdownParser, MarkdownSnapshot};
use typora_platform::{
    choose_markdown_open_path, choose_markdown_save_path, document_from_file, write_markdown_file,
};
use typora_render::{BlockLayoutEngine, LayoutConfig, plain_preview_text};

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
    document: DocumentState,
    snapshot: MarkdownSnapshot,
    preview_text: String,
    ime_preedit: Option<String>,
}

impl AppModel {
    fn from_document(document: DocumentState) -> Self {
        let mut parser = MarkdownParser::new().expect("tree-sitter markdown parser is available");
        let snapshot = parser.parse(document.id, document.version, &document.text());
        let preview_text = preview_text(&snapshot);
        Self {
            document,
            snapshot,
            preview_text,
            ime_preedit: None,
        }
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

    fn insert_sample_line(&mut self) {
        self.document
            .apply(EditorCommand::MoveCursor(CursorMove::DocumentEnd))
            .expect("document end is in range");
        self.document
            .apply(EditorCommand::InsertText(
                "\nNew paragraph from command bus.\n".to_string(),
            ))
            .expect("insert command is valid");
        self.refresh_snapshot();
    }

    fn apply_editor_command(&mut self, command: EditorCommand) {
        // Debug logging to file
        let debug_log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/Users/hujinxian/hya/code/typora-rust2/logs/editor.log")
            .ok();
        if let Some(mut file) = debug_log {
            use std::io::Write;
            let _ = writeln!(file, "apply_editor_command: {:?}", command);
        }

        match command {
            EditorCommand::Copy => {
                if let Some(text) = self.document.selected_text() {
                    match Clipboard::set_contents(text) {
                        Ok(_) => tracing::info!("Copied to clipboard"),
                        Err(e) => tracing::warn!("Failed to copy: {:?}", e),
                    }
                }
            }
            EditorCommand::Cut => {
                if let Some(text) = self.document.selected_text() {
                    match Clipboard::set_contents(text) {
                        Ok(_) => tracing::info!("Cut to clipboard"),
                        Err(e) => tracing::warn!("Failed to copy for cut: {:?}", e),
                    }
                }
                // Now perform the cut
                match self.document.apply(command) {
                    Ok(CommandOutcome::Changed | CommandOutcome::Unchanged) => self.refresh_snapshot(),
                    Ok(CommandOutcome::SaveRequested) => self.save_current(),
                    Ok(CommandOutcome::SaveAsRequested(path)) => self.save_as(path),
                    Err(err) => tracing::warn!(?err, "editor command failed"),
                }
            }
            EditorCommand::Paste(_) => {
                // Get text from clipboard
                match Clipboard::get_contents() {
                    Ok(text) => {
                        let paste_cmd = EditorCommand::Paste(text);
                        match self.document.apply(paste_cmd) {
                            Ok(CommandOutcome::Changed | CommandOutcome::Unchanged) => self.refresh_snapshot(),
                            Ok(CommandOutcome::SaveRequested) => self.save_current(),
                            Ok(CommandOutcome::SaveAsRequested(path)) => self.save_as(path),
                            Err(err) => tracing::warn!(?err, "editor command failed"),
                        }
                    }
                    Err(e) => tracing::warn!("Failed to get clipboard contents: {:?}", e),
                }
            }
            _ => {
                match self.document.apply(command) {
                    Ok(CommandOutcome::Changed | CommandOutcome::Unchanged) => self.refresh_snapshot(),
                    Ok(CommandOutcome::SaveRequested) => self.save_current(),
                    Ok(CommandOutcome::SaveAsRequested(path)) => self.save_as(path),
                    Err(err) => tracing::warn!(?err, "editor command failed"),
                }
            }
        }
    }

    #[allow(dead_code)]
    fn move_cursor_to_local_point(&mut self, point: Point, wrap_columns: usize) {
        let position = text_position_from_local_point(&self.document, point, wrap_columns);
        self.apply_editor_command(EditorCommand::MoveCursor(CursorMove::ToPosition(position)));
    }

    #[allow(dead_code)]
    fn open_via_dialog(&mut self) {
        let Some(path) = choose_markdown_open_path() else {
            return;
        };
        match document_from_file(&path) {
            Ok(document) => *self = Self::from_document(document),
            Err(err) => tracing::warn!(?path, ?err, "failed to open markdown file"),
        }
    }

    fn save_current(&mut self) {
        let path = match self
            .document
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
        match write_markdown_file(&path, &self.document.text()) {
            Ok(()) => self.document.mark_saved_as(path),
            Err(err) => tracing::warn!(?err, "failed to save markdown file"),
        }
        self.refresh_snapshot();
    }

    fn source_display_text(&self, wrap_columns: usize) -> String {
        let mut text = self.document.text();
        if let Some(preedit) = &self.ime_preedit {
            let cursor = self.document.selection.cursor();
            let byte_index = char_to_byte_index(&text, cursor);
            text.insert_str(byte_index, preedit);
        }
        soft_wrap_text(&text, wrap_columns)
    }

    fn refresh_snapshot(&mut self) {
        let mut parser = MarkdownParser::new().expect("tree-sitter markdown parser is available");
        self.snapshot = parser.parse(
            self.document.id,
            self.document.version,
            &self.document.text(),
        );
        self.preview_text = preview_text(&self.snapshot);
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
        .init();
    unsafe { std::env::set_var("RUST_LOG", "debug"); }
    floem::launch(app_view);
}

fn app_view() -> impl IntoView {
    let model = RwSignal::new(AppModel::open_initial());
    let source_origin = RwSignal::new(Point::ZERO);
    let source_wrap_columns = RwSignal::new(DEFAULT_SOURCE_WRAP_COLUMNS);
    let preview_wrap_columns = RwSignal::new(DEFAULT_PREVIEW_WRAP_COLUMNS);

    let source_text = move || {
        let wrap_columns = source_wrap_columns.get();
        model.with(|model| model.source_display_text(wrap_columns))
    };
    let preview = move || {
        let wrap_columns = preview_wrap_columns.get();
        model.with(|model| soft_wrap_text(&model.preview_text, wrap_columns))
    };
    let status = move || {
        model.with(|model| {
            let path = model
                .document
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Untitled.md".to_string());
            format!(
                "{} | {} chars | {} blocks | version {}{}",
                path,
                model.document.char_count(),
                model.snapshot.blocks.len(),
                model.document.version.0,
                if model.document.dirty { " | dirty" } else { "" }
            )
        })
    };
    let source_label = label(source_text)
        .style(|style| {
            style
                .font_family("Menlo".to_string())
                .font_size(SOURCE_FONT_SIZE)
                .line_height((SOURCE_LINE_HEIGHT / SOURCE_FONT_SIZE) as f32)
                .cursor(CursorStyle::Text)
                .width_full()
                .max_width_full()
                .min_height_full()
        })
        .pointer_events(|| false);
    let caret = empty()
        .style(move |style| {
            let wrap_columns = source_wrap_columns.get();
            let (row, col) =
                model.with(|model| cursor_visual_position(&model.document, wrap_columns));
            style
                .absolute()
                .inset_left(col as f64 * SOURCE_CHAR_WIDTH)
                .inset_top(row as f64 * SOURCE_LINE_HEIGHT)
                .width(1.5)
                .height(SOURCE_LINE_HEIGHT)
                .background(Color::BLACK)
        })
        .pointer_events(|| false);
    let source_editor =
        stack((source_label, caret)).style(|style| style.width_full().height_full());
    let source_pane = container(scroll(source_editor))
        .style(|style| style.cursor(CursorStyle::Text))
        .keyboard_navigable();
    let source_id = source_pane.id();
    let source_pane = source_pane
        .on_move(move |origin| {
            source_origin.set(origin);
        })
        .on_resize(move |rect| {
            source_wrap_columns.set(wrap_columns_for_width(
                rect.width(),
                SOURCE_CHAR_WIDTH,
                MAX_SOURCE_WRAP_COLUMNS,
            ));
        })
        .on_event_cont(EventListener::FocusGained, move |_| {
            set_ime_allowed(true);
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            model.update(|model| {
                model.ime_preedit = None;
            });
            set_ime_allowed(false);
        })
        .on_event(EventListener::PointerDown, move |event| {
            let Event::PointerDown(pointer_event) = event else {
                return EventPropagation::Continue;
            };
            if pointer_event.button != PointerButton::Primary {
                return EventPropagation::Continue;
            }

            source_id.request_active();
            source_id.request_focus();
            set_ime_allowed(true);
            let origin = source_origin.get_untracked();
            set_ime_cursor_area(
                Point::new(
                    origin.x + pointer_event.pos.x,
                    origin.y + pointer_event.pos.y,
                ),
                Size::new(2.0, SOURCE_LINE_HEIGHT),
            );
            model.update(|model| {
                let position = text_position_from_local_point(
                    &model.document,
                    point_inside_source_text(pointer_event.pos),
                    source_wrap_columns.get_untracked(),
                );
                let char_idx = model.document.position_to_char(position);
                // Set both anchor and head to clicked position
                model.document.selection.anchor = char_idx;
                model.document.selection.head = char_idx;
            });
            EventPropagation::Stop
        })
        .on_event(EventListener::PointerMove, move |event| {
            let Event::PointerMove(_pointer_event) = event else {
                return EventPropagation::Continue;
            };
            // For now, we only update selection during pointer move if explicitly needed
            // Drag selection is typically tracked via pointer capture
            EventPropagation::Stop
        })
        .on_event(EventListener::PointerUp, move |_| EventPropagation::Stop)
        .on_event(EventListener::ImePreedit, move |event| {
            if let Event::ImePreedit { text, .. } = event {
                model.update(|model| {
                    model.ime_preedit = (!text.is_empty()).then(|| text.clone());
                });
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .on_event(EventListener::ImeCommit, move |event| {
            if let Event::ImeCommit(text) = event {
                model.update(|model| {
                    model.ime_preedit = None;
                    if !text.is_empty() {
                        model.apply_editor_command(EditorCommand::InsertText(text.clone()));
                    }
                });
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .on_event(EventListener::KeyDown, move |event| {
            if model.with(|model| model.ime_preedit.is_some()) {
                return EventPropagation::Continue;
            }
            let Some(command) = key_event_to_command(event) else {
                return EventPropagation::Continue;
            };
            model.update(|model| match command {
                UiCommand::Edit(command) => model.apply_editor_command(command),
                UiCommand::Save => model.save_current(),
            });
            EventPropagation::Stop
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

    v_stack((
        h_stack((
            button(text("Open")).on_click_stop(move |_| {
                model.update(AppModel::open_via_dialog);
            }),
            button(text("Save")).on_click_stop(move |_| {
                model.update(AppModel::save_current);
            }),
            button(text("Insert paragraph")).on_click_stop(move |_| {
                model.update(|model| model.insert_sample_line());
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

fn preview_text(snapshot: &MarkdownSnapshot) -> String {
    let engine = BlockLayoutEngine::new(LayoutConfig::default());
    let blocks = engine.render_blocks(snapshot);
    plain_preview_text(&blocks)
}

enum UiCommand {
    Edit(EditorCommand),
    Save,
}

fn key_event_to_command(event: &Event) -> Option<UiCommand> {
    let Event::KeyDown(key_event) = event else {
        return None;
    };
    let modifiers = key_event.modifiers;
    let primary = primary_modifier(modifiers);
    let shift = modifiers.shift();

    // Debug logging to file
    let debug_log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/Users/hujinxian/hya/code/typora-rust2/logs/editor.log")
        .ok();
    if let Some(mut file) = debug_log {
        use std::io::Write;
        let _ = writeln!(file, "key_event: {:?}, primary: {}, shift: {}", key_event.key.logical_key, primary, shift);
    }

    if primary {
        return match &key_event.key.logical_key {
            Key::Character(ch) if ch.eq_ignore_ascii_case("c") => {
                tracing::debug!("Copy command");
                Some(UiCommand::Edit(EditorCommand::Copy))
            }
            Key::Character(ch) if ch.eq_ignore_ascii_case("x") => {
                tracing::debug!("Cut command");
                Some(UiCommand::Edit(EditorCommand::Cut))
            }
            Key::Character(ch) if ch.eq_ignore_ascii_case("v") => {
                tracing::debug!("Paste command");
                Some(UiCommand::Edit(EditorCommand::Paste(String::new())))
            }
            Key::Character(ch) if ch.eq_ignore_ascii_case("a") => {
                tracing::debug!("SelectAll command");
                Some(UiCommand::Edit(EditorCommand::SelectAll))
            }
            Key::Character(ch) if ch.eq_ignore_ascii_case("s") => Some(UiCommand::Save),
            Key::Character(ch) if ch.eq_ignore_ascii_case("z") && modifiers.shift() => {
                Some(UiCommand::Edit(EditorCommand::Redo))
            }
            Key::Character(ch) if ch.eq_ignore_ascii_case("z") => {
                Some(UiCommand::Edit(EditorCommand::Undo))
            }
            Key::Character(ch) if ch.eq_ignore_ascii_case("y") => {
                Some(UiCommand::Edit(EditorCommand::Redo))
            }
            _ => None,
        };
    }

    // Shift + Arrow for selection extension
    if shift {
        tracing::debug!("Shift pressed, checking arrow keys");
        match &key_event.key.logical_key {
            Key::Named(NamedKey::ArrowLeft) => {
                tracing::debug!("ExtendSelection Left");
                Some(UiCommand::Edit(
                    EditorCommand::ExtendSelection(CursorMove::Left),
                ))
            }
            Key::Named(NamedKey::ArrowRight) => {
                tracing::debug!("ExtendSelection Right");
                Some(UiCommand::Edit(
                    EditorCommand::ExtendSelection(CursorMove::Right),
                ))
            }
            Key::Named(NamedKey::ArrowUp) => {
                tracing::debug!("ExtendSelection Up");
                Some(UiCommand::Edit(EditorCommand::ExtendSelection(
                    CursorMove::Up,
                )))
            }
            Key::Named(NamedKey::ArrowDown) => {
                tracing::debug!("ExtendSelection Down");
                Some(UiCommand::Edit(
                    EditorCommand::ExtendSelection(CursorMove::Down),
                ))
            }
            _ => None,
        }
    } else {
        match &key_event.key.logical_key {
            Key::Named(NamedKey::Backspace) => Some(UiCommand::Edit(EditorCommand::DeleteBackward)),
            Key::Named(NamedKey::Enter) => {
                Some(UiCommand::Edit(EditorCommand::InsertText("\n".to_string())))
            }
            Key::Named(NamedKey::ArrowLeft) => {
                Some(UiCommand::Edit(EditorCommand::MoveCursor(CursorMove::Left)))
            }
            Key::Named(NamedKey::ArrowRight) => Some(UiCommand::Edit(EditorCommand::MoveCursor(
                CursorMove::Right,
            ))),
            Key::Named(NamedKey::ArrowUp) => {
                Some(UiCommand::Edit(EditorCommand::MoveCursor(CursorMove::Up)))
            }
            Key::Named(NamedKey::ArrowDown) => {
                Some(UiCommand::Edit(EditorCommand::MoveCursor(CursorMove::Down)))
            }
            Key::Named(NamedKey::Home) => Some(UiCommand::Edit(EditorCommand::MoveCursor(
                CursorMove::LineStart,
            ))),
            Key::Named(NamedKey::End) => Some(UiCommand::Edit(EditorCommand::MoveCursor(
                CursorMove::LineEnd,
            ))),
            Key::Named(NamedKey::Space) => {
                Some(UiCommand::Edit(EditorCommand::InsertText(" ".to_string())))
            }
            Key::Character(ch) if !modifiers.control() && !modifiers.alt() && !modifiers.meta() => {
                Some(UiCommand::Edit(EditorCommand::InsertText(ch.to_string())))
            }
            _ => None,
        }
    }
}

fn primary_modifier(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.meta()
    } else {
        modifiers.control()
    }
}

fn char_to_byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

fn point_inside_source_text(point: Point) -> Point {
    Point::new(
        (point.x - PANE_PADDING).max(0.0),
        (point.y - PANE_PADDING).max(0.0),
    )
}

fn text_position_from_local_point(
    document: &DocumentState,
    point: Point,
    wrap_columns: usize,
) -> TextPosition {
    let visual_row = (point.y.max(0.0) / SOURCE_LINE_HEIGHT).floor() as usize;
    let visual_col = (point.x.max(0.0) / SOURCE_CHAR_WIDTH).round() as usize;
    visual_position_to_text_position(document, visual_row, visual_col, wrap_columns)
}

fn cursor_visual_position(document: &DocumentState, wrap_columns: usize) -> (usize, usize) {
    let position = document.char_to_position(document.selection.cursor());
    text_position_to_visual_position(document, position, wrap_columns)
}

fn text_position_to_visual_position(
    document: &DocumentState,
    position: TextPosition,
    wrap_columns: usize,
) -> (usize, usize) {
    let wrap_columns = wrap_columns.max(1);
    let line = position.line.min(document.line_count().saturating_sub(1));
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
        let display = model.source_display_text(DEFAULT_SOURCE_WRAP_COLUMNS);

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
        let display = model.source_display_text(DEFAULT_SOURCE_WRAP_COLUMNS);

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
            .apply(EditorCommand::MoveCursor(CursorMove::ToPosition(
                TextPosition {
                    line: 1,
                    character: 2,
                },
            )))
            .unwrap();
        let model = AppModel::from_document(document.clone());

        assert_eq!(
            cursor_visual_position(&document, DEFAULT_SOURCE_WRAP_COLUMNS),
            (1, 2)
        );
        assert_eq!(
            model.source_display_text(DEFAULT_SOURCE_WRAP_COLUMNS),
            "abcd\nefgh"
        );
    }

    #[test]
    fn backspace_deletes_character_left_of_visual_caret() {
        let mut document = DocumentState::from_text("abcd");
        document
            .apply(EditorCommand::MoveCursor(CursorMove::ToPosition(
                TextPosition {
                    line: 0,
                    character: 2,
                },
            )))
            .unwrap();

        assert_eq!(
            cursor_visual_position(&document, DEFAULT_SOURCE_WRAP_COLUMNS),
            (0, 2)
        );

        document.apply(EditorCommand::DeleteBackward).unwrap();

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
