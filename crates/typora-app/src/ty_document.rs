use std::cell::RefCell;
use std::rc::Rc;

use floem::keyboard::Modifiers;
use floem::prelude::{RwSignal, SignalUpdate, SignalWith};
use floem::reactive::Scope;
use floem::views::editor::{
    command::{Command, CommandExecuted},
    core::selection::Selection,
    id::EditorId,
    phantom_text::PhantomTextLine,
    text::{Document, DocumentPhantom, PreeditData},
    Editor, EditorStyle,
};
use floem_editor_core::buffer::rope_text::RopeTextVal;
use floem_editor_core::command::EditCommand;
use floem_editor_core::editor::EditType;
use floem_editor_core::movement::Movement;
use lapce_xi_rope::Rope;
use typora_core::{CursorMove, DocumentState, EditorCommand, Selection as TySelection};

fn byte_to_char_idx(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    text[..byte].chars().count()
}

fn char_to_byte_offset(text: &str, char_idx: usize) -> usize {
    text.chars()
        .take(char_idx)
        .map(|c| c.len_utf8())
        .sum()
}

pub struct TyDocument {
    doc: Rc<RefCell<DocumentState>>,
    cache_rev: RwSignal<u64>,
    preedit: PreeditData,
}

impl TyDocument {
    pub fn new(doc: Rc<RefCell<DocumentState>>, change_count: RwSignal<u64>) -> Self {
        Self {
            doc,
            cache_rev: change_count,
            preedit: PreeditData::new(Scope::current()),
        }
    }

    fn sync_from_doc(&self) {
        self.cache_rev.update(|c| *c += 1);
    }

    /// Sync DocumentState's char-based selection → editor's byte-offset cursor.
    fn sync_cursor_to_editor(&self, ed: &Editor) {
        let doc_ref = self.doc.borrow();
        let text = doc_ref.text();
        let selection = doc_ref.selection;
        let head_byte = char_to_byte_offset(&text, selection.head);
        let anchor_byte = char_to_byte_offset(&text, selection.anchor);
        drop(doc_ref);

        ed.cursor.update(|cursor| {
            if selection.anchor == selection.head {
                cursor.set_offset(head_byte, false, false);
            } else {
                cursor.set_offset(anchor_byte, false, false);
                cursor.set_offset(head_byte, true, false);
            }
        });
    }

    /// Sync editor's byte-offset cursor → DocumentState's char-based selection.
    fn sync_cursor_from_editor(&self, ed: &Editor) {
        let regions: Vec<_> = ed.cursor.with_untracked(|c| c.regions_iter().collect());

        let doc_ref = self.doc.borrow();
        let text = doc_ref.text();
        let current_anchor = doc_ref.selection.anchor;
        drop(doc_ref);

        let mut doc = self.doc.borrow_mut();
        if let Some(&(byte_start, byte_end)) = regions.first() {
            let char_start = byte_to_char_idx(&text, byte_start);
            let char_end = byte_to_char_idx(&text, byte_end);
            if char_start == char_end {
                doc.selection = TySelection::caret(char_start);
            } else if current_anchor == char_start {
                doc.selection = TySelection { anchor: char_start, head: char_end };
            } else if current_anchor == char_end {
                doc.selection = TySelection { anchor: char_end, head: char_start };
            } else {
                doc.selection = TySelection { anchor: char_start, head: char_end };
            }
        }
    }

    fn movement_to_cursor_move(&self, movement: &Movement) -> Option<CursorMove> {
        match movement {
            Movement::Left => Some(CursorMove::Left),
            Movement::Right => Some(CursorMove::Right),
            Movement::Up => Some(CursorMove::Up),
            Movement::Down => Some(CursorMove::Down),
            Movement::DocumentStart => Some(CursorMove::DocumentStart),
            Movement::DocumentEnd => Some(CursorMove::DocumentEnd),
            Movement::StartOfLine => Some(CursorMove::LineStart),
            Movement::EndOfLine => Some(CursorMove::LineEnd),
            _ => None,
        }
    }

    fn handle_edit_command(&self, ed: &Editor, cmd: &EditCommand) -> CommandExecuted {
        match cmd {
            EditCommand::ClipboardCopy => {
                self.sync_cursor_from_editor(ed);
                if let Some(text) = self.doc.borrow().selected_text() {
                    let _ = floem::Clipboard::set_contents(text);
                }
                CommandExecuted::Yes
            }
            EditCommand::ClipboardCut => {
                self.sync_cursor_from_editor(ed);
                if let Some(text) = self.doc.borrow().selected_text() {
                    let _ = floem::Clipboard::set_contents(text);
                }
                let mut doc = self.doc.borrow_mut();
                let _ = doc.apply(EditorCommand::Cut);
                drop(doc);
                self.sync_from_doc();
                self.sync_cursor_to_editor(ed);
                CommandExecuted::Yes
            }
            EditCommand::ClipboardPaste => {
                if let Ok(text) = floem::Clipboard::get_contents() {
                    self.sync_cursor_from_editor(ed);
                    let mut doc = self.doc.borrow_mut();
                    let _ = doc.apply(EditorCommand::Paste(text));
                    drop(doc);
                    self.sync_from_doc();
                    self.sync_cursor_to_editor(ed);
                }
                CommandExecuted::Yes
            }
            EditCommand::Undo => {
                let mut doc = self.doc.borrow_mut();
                let _ = doc.apply(EditorCommand::Undo);
                drop(doc);
                self.sync_from_doc();
                self.sync_cursor_to_editor(ed);
                CommandExecuted::Yes
            }
            EditCommand::Redo => {
                let mut doc = self.doc.borrow_mut();
                let _ = doc.apply(EditorCommand::Redo);
                drop(doc);
                self.sync_from_doc();
                self.sync_cursor_to_editor(ed);
                CommandExecuted::Yes
            }
            EditCommand::DeleteBackward => {
                self.sync_cursor_from_editor(ed);
                let mut doc = self.doc.borrow_mut();
                let _ = doc.apply(EditorCommand::DeleteBackward);
                drop(doc);
                self.sync_from_doc();
                self.sync_cursor_to_editor(ed);
                CommandExecuted::Yes
            }
            EditCommand::DeleteForward => {
                self.sync_cursor_from_editor(ed);
                let mut doc = self.doc.borrow_mut();
                let _ = doc.apply(EditorCommand::MoveCursor(CursorMove::Right));
                let _ = doc.apply(EditorCommand::DeleteBackward);
                drop(doc);
                self.sync_from_doc();
                self.sync_cursor_to_editor(ed);
                CommandExecuted::Yes
            }
            EditCommand::InsertNewLine => {
                self.sync_cursor_from_editor(ed);
                let mut doc = self.doc.borrow_mut();
                let _ = doc.apply(EditorCommand::InsertText("\n".to_string()));
                drop(doc);
                self.sync_from_doc();
                self.sync_cursor_to_editor(ed);
                CommandExecuted::Yes
            }
            EditCommand::InsertTab => {
                self.sync_cursor_from_editor(ed);
                let mut doc = self.doc.borrow_mut();
                let _ = doc.apply(EditorCommand::InsertText("\t".to_string()));
                drop(doc);
                self.sync_from_doc();
                self.sync_cursor_to_editor(ed);
                CommandExecuted::Yes
            }
            _ => CommandExecuted::No,
        }
    }
}

impl Document for TyDocument {
    fn text(&self) -> Rope {
        Rope::from(self.doc.borrow().text())
    }

    fn rope_text(&self) -> RopeTextVal {
        RopeTextVal::new(self.text())
    }

    fn cache_rev(&self) -> RwSignal<u64> {
        self.cache_rev
    }

    fn preedit(&self) -> PreeditData {
        self.preedit.clone()
    }

    fn run_command(
        &self,
        ed: &Editor,
        cmd: &Command,
        count: Option<usize>,
        modifiers: Modifiers,
    ) -> CommandExecuted {
        match cmd {
            Command::Move(move_cmd) => {
                let movement = move_cmd.to_movement(count);
                if let Some(cursor_move) = self.movement_to_cursor_move(&movement) {
                    self.sync_cursor_from_editor(ed);
                    let mut doc = self.doc.borrow_mut();
                    if modifiers.shift() {
                        doc.apply(EditorCommand::ExtendSelection(cursor_move)).ok();
                    } else {
                        doc.apply(EditorCommand::MoveCursor(cursor_move)).ok();
                    }
                    drop(doc);
                    self.sync_from_doc();
                    self.sync_cursor_to_editor(ed);
                    return CommandExecuted::Yes;
                }
            }
            Command::Edit(edit_cmd) => {
                return self.handle_edit_command(ed, edit_cmd);
            }
            Command::MultiSelection(multi_cmd) => {
                use floem_editor_core::command::MultiSelectionCommand;
                if matches!(multi_cmd, MultiSelectionCommand::SelectAll) {
                    let len = self.doc.borrow().char_count();
                    self.doc.borrow_mut().selection = TySelection {
                        anchor: 0,
                        head: len,
                    };
                    self.sync_from_doc();
                    self.sync_cursor_to_editor(ed);
                    return CommandExecuted::Yes;
                }
            }
            _ => {}
        }
        CommandExecuted::No
    }

    fn receive_char(&self, ed: &Editor, c: &str) {
        self.sync_cursor_from_editor(ed);
        let mut doc = self.doc.borrow_mut();
        let _ = doc.apply(EditorCommand::InsertText(c.to_string()));
        drop(doc);
        self.sync_from_doc();
        self.sync_cursor_to_editor(ed);
    }

    fn edit(&self, iter: &mut dyn Iterator<Item = (Selection, &str)>, _edit_type: EditType) {
        for (selection, content) in iter {
            if let Some(region) = selection.regions().first() {
                let byte_start = region.start.min(region.end);
                let byte_end = region.start.max(region.end);

                let doc_ref = self.doc.borrow();
                let text = doc_ref.text();
                let char_start = byte_to_char_idx(&text, byte_start);
                let char_end = byte_to_char_idx(&text, byte_end);
                drop(doc_ref);

                let mut doc = self.doc.borrow_mut();

                if char_start != char_end {
                    doc.apply(EditorCommand::SetSelection(TySelection {
                        anchor: char_start,
                        head: char_end,
                    }))
                    .ok();
                    doc.apply(EditorCommand::DeleteSelection).ok();
                }

                if !content.is_empty() {
                    doc.apply(EditorCommand::SetSelection(TySelection::caret(char_start)))
                        .ok();
                    doc.apply(EditorCommand::InsertText(content.to_string()))
                        .ok();
                }
            }
        }
        self.sync_from_doc();
    }
}

impl DocumentPhantom for TyDocument {
    fn phantom_text(
        &self,
        _edid: EditorId,
        _styling: &EditorStyle,
        _line: usize,
    ) -> PhantomTextLine {
        PhantomTextLine::default()
    }
}
