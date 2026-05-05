use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ropey::Rope;
use serde::{Deserialize, Serialize};
use thiserror::Error;

static NEXT_DOCUMENT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct DocumentId(pub u64);

impl DocumentId {
    #[must_use]
    pub fn fresh() -> Self {
        Self(NEXT_DOCUMENT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
)]
pub struct DocVersion(pub u64);

impl DocVersion {
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NewlineStyle {
    Lf,
    Crlf,
}

impl NewlineStyle {
    #[must_use]
    pub fn detect(text: &str) -> Self {
        if text.contains("\r\n") {
            Self::Crlf
        } else {
            Self::Lf
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TextPosition {
    pub line: usize,
    pub character: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    #[must_use]
    pub const fn caret(position: usize) -> Self {
        Self {
            anchor: position,
            head: position,
        }
    }

    #[must_use]
    pub const fn is_caret(self) -> bool {
        self.anchor == self.head
    }

    #[must_use]
    pub fn normalized(self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    #[must_use]
    pub const fn cursor(self) -> usize {
        self.head
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CursorMove {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
    ToPosition(TextPosition),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorCommand {
    InsertText(String),
    DeleteBackward,
    DeleteSelection,
    MoveCursor(CursorMove),
    ExtendSelection(CursorMove),
    SetSelection(Selection),
    SelectAll,
    Undo,
    Redo,
    Save,
    SaveAs(PathBuf),
    Copy,
    Cut,
    Paste(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandOutcome {
    Changed,
    Unchanged,
    SaveRequested,
    SaveAsRequested(PathBuf),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum DocumentError {
    #[error("selection index {index} is outside document length {len}")]
    SelectionOutOfBounds { index: usize, len: usize },
}

#[derive(Clone)]
struct HistoryFrame {
    rope: Rope,
    selection: Selection,
    dirty: bool,
}

#[derive(Clone)]
pub struct DocumentState {
    pub id: DocumentId,
    pub path: Option<PathBuf>,
    rope: Rope,
    pub version: DocVersion,
    pub dirty: bool,
    pub newline: NewlineStyle,
    pub selection: Selection,
    undo_stack: Vec<HistoryFrame>,
    redo_stack: Vec<HistoryFrame>,
}

impl DocumentState {
    #[must_use]
    pub fn empty() -> Self {
        Self::from_text("")
    }

    #[must_use]
    pub fn from_text(text: &str) -> Self {
        let newline = NewlineStyle::detect(text);
        Self {
            id: DocumentId::fresh(),
            path: None,
            rope: Rope::from_str(text),
            version: DocVersion::default(),
            dirty: false,
            newline,
            selection: Selection::caret(0),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    #[must_use]
    pub fn from_path_and_text(path: PathBuf, text: &str) -> Self {
        let mut document = Self::from_text(text);
        document.path = Some(path);
        document
    }

    #[must_use]
    pub fn char_count(&self) -> usize {
        self.rope.len_chars()
    }

    #[must_use]
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    #[must_use]
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    #[must_use]
    pub fn line_text(&self, line: usize) -> Option<String> {
        (line < self.rope.len_lines()).then(|| self.rope.line(line).to_string())
    }

    #[must_use]
    pub fn line_visible_char_count(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let line = self.rope.line(line).to_string();
        line.trim_end_matches(['\r', '\n']).chars().count()
    }

    /// Returns the text in the given byte-index range.
    #[must_use]
    pub fn text_range(&self, range: Range<usize>) -> String {
        let len = self.char_count();
        let start = range.start.min(len);
        let end = range.end.min(len);
        if start >= end {
            return String::new();
        }
        self.rope.slice(start..end).to_string()
    }

    /// Returns the currently selected text, or None if the selection is empty (caret only).
    #[must_use]
    pub fn selected_text(&self) -> Option<String> {
        let range = self.selection.normalized();
        if range.is_empty() {
            return None;
        }
        Some(self.text_range(range))
    }

    pub fn set_path(&mut self, path: PathBuf) {
        self.path = Some(path);
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
        self.undo_stack
            .iter_mut()
            .for_each(|frame| frame.dirty = true);
    }

    pub fn mark_saved_as(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.mark_saved();
    }

    pub fn apply(&mut self, command: EditorCommand) -> Result<CommandOutcome, DocumentError> {
        match command {
            EditorCommand::InsertText(text) => self.insert_text(&text),
            EditorCommand::DeleteBackward => self.delete_backward(),
            EditorCommand::DeleteSelection => self.delete_selection(),
            EditorCommand::MoveCursor(movement) => {
                self.move_cursor(movement);
                Ok(CommandOutcome::Unchanged)
            }
            EditorCommand::ExtendSelection(movement) => {
                self.extend_selection(movement);
                Ok(CommandOutcome::Unchanged)
            }
            EditorCommand::SetSelection(selection) => {
                self.set_selection(selection)?;
                Ok(CommandOutcome::Unchanged)
            }
            EditorCommand::SelectAll => {
                self.selection = Selection {
                    anchor: 0,
                    head: self.char_count(),
                };
                Ok(CommandOutcome::Unchanged)
            }
            EditorCommand::Undo => Ok(self.undo()),
            EditorCommand::Redo => Ok(self.redo()),
            EditorCommand::Save => Ok(CommandOutcome::SaveRequested),
            EditorCommand::SaveAs(path) => Ok(CommandOutcome::SaveAsRequested(path)),
            EditorCommand::Copy => {
                // Clipboard copy is handled by the app layer
                Ok(CommandOutcome::Unchanged)
            }
            EditorCommand::Cut => {
                let range = self.selection.normalized();
                if !range.is_empty() {
                    self.push_undo();
                    self.rope.remove(range.clone());
                    self.selection = Selection::caret(range.start);
                    self.mark_changed();
                    Ok(CommandOutcome::Changed)
                } else {
                    Ok(CommandOutcome::Unchanged)
                }
            }
            EditorCommand::Paste(text) => self.insert_text(&text),
        }
    }

    pub fn set_selection(&mut self, selection: Selection) -> Result<(), DocumentError> {
        let len = self.char_count();
        for index in [selection.anchor, selection.head] {
            if index > len {
                return Err(DocumentError::SelectionOutOfBounds { index, len });
            }
        }
        self.selection = selection;
        Ok(())
    }

    #[must_use]
    pub fn char_to_position(&self, char_idx: usize) -> TextPosition {
        let bounded = char_idx.min(self.char_count());
        let line = self.rope.char_to_line(bounded);
        let line_start = self.rope.line_to_char(line);
        TextPosition {
            line,
            character: bounded.saturating_sub(line_start),
        }
    }

    #[must_use]
    pub fn position_to_char(&self, position: TextPosition) -> usize {
        let line = position.line.min(self.rope.len_lines().saturating_sub(1));
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_visible_char_count(line);
        line_start + position.character.min(line_len)
    }

    fn insert_text(&mut self, text: &str) -> Result<CommandOutcome, DocumentError> {
        if text.is_empty() {
            return Ok(CommandOutcome::Unchanged);
        }

        self.validate_selection()?;
        self.push_undo();

        let insert_at = self.delete_selection_inner();
        self.rope.insert(insert_at, text);
        let cursor = insert_at + text.chars().count();
        self.selection = Selection::caret(cursor);
        self.mark_changed();
        Ok(CommandOutcome::Changed)
    }

    fn delete_backward(&mut self) -> Result<CommandOutcome, DocumentError> {
        self.validate_selection()?;
        if !self.selection.is_caret() {
            return self.delete_selection();
        }

        let cursor = self.selection.cursor();
        if cursor == 0 {
            return Ok(CommandOutcome::Unchanged);
        }

        self.push_undo();
        self.rope.remove(cursor - 1..cursor);
        self.selection = Selection::caret(cursor - 1);
        self.mark_changed();
        Ok(CommandOutcome::Changed)
    }

    fn delete_selection(&mut self) -> Result<CommandOutcome, DocumentError> {
        self.validate_selection()?;
        if self.selection.is_caret() {
            return Ok(CommandOutcome::Unchanged);
        }

        self.push_undo();
        let cursor = self.delete_selection_inner();
        self.selection = Selection::caret(cursor);
        self.mark_changed();
        Ok(CommandOutcome::Changed)
    }

    fn delete_selection_inner(&mut self) -> usize {
        let range = self.selection.normalized();
        if !range.is_empty() {
            self.rope.remove(range.clone());
        }
        range.start
    }

    fn move_cursor(&mut self, movement: CursorMove) {
        let cursor = self.selection.cursor();
        let next = match movement {
            CursorMove::Left => cursor.saturating_sub(1),
            CursorMove::Right => (cursor + 1).min(self.char_count()),
            CursorMove::Up => {
                let pos = self.char_to_position(cursor);
                if pos.line == 0 {
                    0
                } else {
                    self.position_to_char(TextPosition {
                        line: pos.line - 1,
                        character: pos.character,
                    })
                }
            }
            CursorMove::Down => {
                let pos = self.char_to_position(cursor);
                if pos.line + 1 >= self.line_count() {
                    self.char_count()
                } else {
                    self.position_to_char(TextPosition {
                        line: pos.line + 1,
                        character: pos.character,
                    })
                }
            }
            CursorMove::LineStart => {
                let pos = self.char_to_position(cursor);
                self.position_to_char(TextPosition {
                    line: pos.line,
                    character: 0,
                })
            }
            CursorMove::LineEnd => {
                let pos = self.char_to_position(cursor);
                self.position_to_char(TextPosition {
                    line: pos.line,
                    character: self.line_visible_char_count(pos.line),
                })
            }
            CursorMove::DocumentStart => 0,
            CursorMove::DocumentEnd => self.char_count(),
            CursorMove::ToPosition(position) => self.position_to_char(position),
        };
        self.selection = Selection::caret(next);
    }

    fn extend_selection(&mut self, movement: CursorMove) {
        let head = self.selection.head;
        // Compute next head position based on movement
        let next = match movement {
            CursorMove::Left => head.saturating_sub(1),
            CursorMove::Right => (head + 1).min(self.char_count()),
            CursorMove::Up => {
                let pos = self.char_to_position(head);
                if pos.line == 0 {
                    0
                } else {
                    self.position_to_char(TextPosition {
                        line: pos.line - 1,
                        character: pos.character,
                    })
                }
            }
            CursorMove::Down => {
                let pos = self.char_to_position(head);
                if pos.line + 1 >= self.line_count() {
                    self.char_count()
                } else {
                    self.position_to_char(TextPosition {
                        line: pos.line + 1,
                        character: pos.character,
                    })
                }
            }
            CursorMove::LineStart => {
                let pos = self.char_to_position(head);
                self.position_to_char(TextPosition {
                    line: pos.line,
                    character: 0,
                })
            }
            CursorMove::LineEnd => {
                let pos = self.char_to_position(head);
                self.position_to_char(TextPosition {
                    line: pos.line,
                    character: self.line_visible_char_count(pos.line),
                })
            }
            CursorMove::DocumentStart => 0,
            CursorMove::DocumentEnd => self.char_count(),
            CursorMove::ToPosition(position) => self.position_to_char(position),
        };
        self.selection.head = next;
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(HistoryFrame {
            rope: self.rope.clone(),
            selection: self.selection,
            dirty: self.dirty,
        });
        self.redo_stack.clear();
    }

    fn undo(&mut self) -> CommandOutcome {
        let Some(frame) = self.undo_stack.pop() else {
            return CommandOutcome::Unchanged;
        };
        self.redo_stack.push(HistoryFrame {
            rope: self.rope.clone(),
            selection: self.selection,
            dirty: self.dirty,
        });
        self.rope = frame.rope;
        self.selection = frame.selection;
        self.dirty = frame.dirty;
        self.version = self.version.next();
        CommandOutcome::Changed
    }

    fn redo(&mut self) -> CommandOutcome {
        let Some(frame) = self.redo_stack.pop() else {
            return CommandOutcome::Unchanged;
        };
        self.undo_stack.push(HistoryFrame {
            rope: self.rope.clone(),
            selection: self.selection,
            dirty: self.dirty,
        });
        self.rope = frame.rope;
        self.selection = frame.selection;
        self.dirty = frame.dirty;
        self.version = self.version.next();
        CommandOutcome::Changed
    }

    fn mark_changed(&mut self) {
        self.dirty = true;
        self.version = self.version.next();
    }

    fn validate_selection(&self) -> Result<(), DocumentError> {
        let len = self.char_count();
        for index in [self.selection.anchor, self.selection.head] {
            if index > len {
                return Err(DocumentError::SelectionOutOfBounds { index, len });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_and_deletes_text_with_rope_indices() {
        let mut doc = DocumentState::from_text("hello");

        doc.apply(EditorCommand::MoveCursor(CursorMove::DocumentEnd))
            .unwrap();
        doc.apply(EditorCommand::InsertText(" 世界".to_string()))
            .unwrap();
        assert_eq!(doc.text(), "hello 世界");
        assert!(doc.dirty);

        doc.apply(EditorCommand::DeleteBackward).unwrap();
        assert_eq!(doc.text(), "hello 世");
    }

    #[test]
    fn selection_replacement_is_single_undo_frame() {
        let mut doc = DocumentState::from_text("abcdef");
        doc.apply(EditorCommand::SetSelection(Selection {
            anchor: 1,
            head: 5,
        }))
        .unwrap();
        doc.apply(EditorCommand::InsertText("X".into())).unwrap();

        assert_eq!(doc.text(), "aXf");
        doc.apply(EditorCommand::Undo).unwrap();
        assert_eq!(doc.text(), "abcdef");
        assert_eq!(doc.selection, Selection { anchor: 1, head: 5 });
    }

    #[test]
    fn char_position_round_trip_handles_unicode() {
        let doc = DocumentState::from_text("a\n中文🙂\nlast");
        let idx = doc.position_to_char(TextPosition {
            line: 1,
            character: 3,
        });
        assert_eq!(doc.char_to_position(idx).line, 1);
        assert_eq!(doc.char_to_position(idx).character, 3);
    }

    #[test]
    fn vertical_cursor_movement_clamps_to_visible_line_width() {
        let mut doc = DocumentState::from_text("abcd\n中🙂\nxyz");
        doc.apply(EditorCommand::MoveCursor(CursorMove::ToPosition(
            TextPosition {
                line: 0,
                character: 4,
            },
        )))
        .unwrap();

        doc.apply(EditorCommand::MoveCursor(CursorMove::Down))
            .unwrap();
        assert_eq!(
            doc.char_to_position(doc.selection.cursor()),
            TextPosition {
                line: 1,
                character: 2
            }
        );

        doc.apply(EditorCommand::MoveCursor(CursorMove::Down))
            .unwrap();
        assert_eq!(
            doc.char_to_position(doc.selection.cursor()),
            TextPosition {
                line: 2,
                character: 2
            }
        );

        doc.apply(EditorCommand::MoveCursor(CursorMove::Up))
            .unwrap();
        assert_eq!(
            doc.char_to_position(doc.selection.cursor()),
            TextPosition {
                line: 1,
                character: 2
            }
        );
    }

    #[test]
    fn save_command_is_request_not_file_io() {
        let mut doc = DocumentState::from_text("draft");
        assert_eq!(
            doc.apply(EditorCommand::Save).unwrap(),
            CommandOutcome::SaveRequested
        );
    }

    #[test]
    fn selected_text_returns_none_for_empty_selection() {
        let doc = DocumentState::from_text("hello");
        assert!(doc.selected_text().is_none());
    }

    #[test]
    fn selected_text_returns_selected_content() {
        let mut doc = DocumentState::from_text("hello world");
        doc.apply(EditorCommand::SetSelection(Selection {
            anchor: 0,
            head: 5,
        }))
        .unwrap();
        assert_eq!(doc.selected_text(), Some("hello".to_string()));
    }

    #[test]
    fn text_range_returns_correct_substring() {
        let doc = DocumentState::from_text("hello world");
        assert_eq!(doc.text_range(0..5), "hello");
        assert_eq!(doc.text_range(6..11), "world");
        assert_eq!(doc.text_range(0..0), "");
    }

    #[test]
    fn extend_selection_moves_head_keeping_anchor() {
        let mut doc = DocumentState::from_text("hello world");
        doc.apply(EditorCommand::SetSelection(Selection {
            anchor: 0,
            head: 5,
        }))
        .unwrap();

        doc.apply(EditorCommand::ExtendSelection(CursorMove::Right))
            .unwrap();
        assert_eq!(doc.selection.anchor, 0);
        assert_eq!(doc.selection.head, 6);

        doc.apply(EditorCommand::ExtendSelection(CursorMove::Left))
            .unwrap();
        assert_eq!(doc.selection.anchor, 0);
        assert_eq!(doc.selection.head, 5);
    }

    #[test]
    fn extend_selection_with_unicode() {
        let mut doc = DocumentState::from_text("a中文b");
        doc.apply(EditorCommand::SetSelection(Selection {
            anchor: 1,
            head: 1,
        }))
        .unwrap();

        doc.apply(EditorCommand::ExtendSelection(CursorMove::Right))
            .unwrap();
        assert_eq!(doc.selection.head, 2);

        doc.apply(EditorCommand::ExtendSelection(CursorMove::Right))
            .unwrap();
        assert_eq!(doc.selection.head, 3);
    }

    #[test]
    fn delete_after_cut_removes_selected_text() {
        let mut doc = DocumentState::from_text("hello world");
        doc.apply(EditorCommand::SetSelection(Selection {
            anchor: 0,
            head: 5,
        }))
        .unwrap();

        let cut_text = doc.selected_text().unwrap();
        assert_eq!(cut_text, "hello");

        doc.apply(EditorCommand::DeleteSelection).unwrap();
        assert_eq!(doc.text(), " world");
    }
}
