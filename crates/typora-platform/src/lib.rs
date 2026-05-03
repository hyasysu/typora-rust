use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use typora_core::{DocumentState, NewlineStyle};

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("file io failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("file watcher failed: {0}")]
    Notify(#[from] notify::Error),
    #[error("front matter parse failed: {0}")]
    FrontMatter(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileDocument {
    pub path: PathBuf,
    pub text: String,
    pub newline: NewlineStyle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FrontMatterKind {
    Yaml,
    Toml,
    Json,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrontMatter {
    pub kind: FrontMatterKind,
    pub value: Value,
    pub body_start_byte: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileChangeKind {
    Modified,
    Removed,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileChange {
    pub path: PathBuf,
    pub kind: FileChangeKind,
}

pub fn read_markdown_file(path: impl AsRef<Path>) -> Result<FileDocument, PlatformError> {
    let path = path.as_ref();
    let text = fs::read_to_string(path).map_err(|source| PlatformError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(FileDocument {
        path: path.to_path_buf(),
        newline: NewlineStyle::detect(&text),
        text,
    })
}

pub fn document_from_file(path: impl AsRef<Path>) -> Result<DocumentState, PlatformError> {
    let file = read_markdown_file(path)?;
    let mut document = DocumentState::from_path_and_text(file.path, &file.text);
    document.newline = file.newline;
    Ok(document)
}

pub fn write_markdown_file(path: impl AsRef<Path>, text: &str) -> Result<(), PlatformError> {
    let path = path.as_ref();
    fs::write(path, text).map_err(|source| PlatformError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[must_use]
pub fn choose_markdown_open_path() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Markdown", &["md", "markdown", "mdown"])
        .pick_file()
}

#[must_use]
pub fn choose_markdown_save_path() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Markdown", &["md", "markdown", "mdown"])
        .set_file_name("Untitled.md")
        .save_file()
}

pub fn parse_front_matter(text: &str) -> Result<Option<FrontMatter>, PlatformError> {
    let Some(first_line_end) = text.find('\n') else {
        return Ok(None);
    };
    let first_line = text[..first_line_end].trim_end_matches('\r');
    let Some((kind, delimiter)) = front_matter_delimiter(first_line) else {
        return Ok(None);
    };

    let mut search_from = first_line_end + 1;
    while search_from < text.len() {
        let Some(line_end_offset) = text[search_from..].find('\n') else {
            break;
        };
        let line_end = search_from + line_end_offset;
        let line = text[search_from..line_end].trim_end_matches('\r');
        if line == delimiter {
            let raw = &text[first_line_end + 1..search_from];
            let body_start_byte = line_end + 1;
            let value = match kind {
                FrontMatterKind::Yaml => serde_yaml_ng::from_str::<Value>(raw)
                    .map_err(|err| PlatformError::FrontMatter(err.to_string()))?,
                FrontMatterKind::Toml => {
                    let value = toml::from_str::<toml::Value>(raw)
                        .map_err(|err| PlatformError::FrontMatter(err.to_string()))?;
                    serde_json::to_value(value)
                        .map_err(|err| PlatformError::FrontMatter(err.to_string()))?
                }
                FrontMatterKind::Json => serde_json::from_str::<Value>(raw)
                    .map_err(|err| PlatformError::FrontMatter(err.to_string()))?,
            };
            return Ok(Some(FrontMatter {
                kind,
                value,
                body_start_byte,
            }));
        }
        search_from = line_end + 1;
    }

    Ok(None)
}

fn front_matter_delimiter(line: &str) -> Option<(FrontMatterKind, &'static str)> {
    match line {
        "---" => Some((FrontMatterKind::Yaml, "---")),
        "+++" => Some((FrontMatterKind::Toml, "+++")),
        ";;;" => Some((FrontMatterKind::Json, ";;;")),
        _ => None,
    }
}

pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    receiver: Receiver<notify::Result<Event>>,
}

impl FileWatcher {
    pub fn watch(path: impl AsRef<Path>) -> Result<Self, PlatformError> {
        let (sender, receiver) = channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })?;
        watcher.watch(path.as_ref(), RecursiveMode::NonRecursive)?;
        Ok(Self {
            _watcher: watcher,
            receiver,
        })
    }

    #[must_use]
    pub fn poll(&self) -> Vec<FileChange> {
        let mut changes = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            if let Ok(event) = event {
                let kind = if event.kind.is_modify() {
                    FileChangeKind::Modified
                } else if event.kind.is_remove() {
                    FileChangeKind::Removed
                } else {
                    FileChangeKind::Other
                };
                changes.extend(event.paths.into_iter().map(|path| FileChange {
                    path,
                    kind: kind.clone(),
                }));
            }
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn parses_yaml_front_matter() {
        let fm = parse_front_matter("---\ntitle: Hello\ntags:\n  - rust\n---\n# Body")
            .unwrap()
            .unwrap();
        assert_eq!(fm.kind, FrontMatterKind::Yaml);
        assert_eq!(fm.value["title"], "Hello");
        assert!(fm.body_start_byte > 0);
    }

    #[test]
    fn parses_toml_front_matter() {
        let fm = parse_front_matter("+++\ntitle = \"Hello\"\n+++\nBody")
            .unwrap()
            .unwrap();
        assert_eq!(fm.value["title"], "Hello");
    }

    #[test]
    fn file_round_trip_preserves_text() {
        let dir = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = dir.join(format!("typora-rust-{stamp}.md"));

        write_markdown_file(&path, "# Title\n\nBody").unwrap();
        let file = read_markdown_file(&path).unwrap();
        assert_eq!(file.text, "# Title\n\nBody");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn document_open_edit_save_round_trip() {
        let dir = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = dir.join(format!("typora-rust-edit-{stamp}.md"));

        write_markdown_file(&path, "# GUI Test\n\nOriginal line.\n").unwrap();
        let mut document = document_from_file(&path).unwrap();
        document
            .apply(typora_core::EditorCommand::MoveCursor(
                typora_core::CursorMove::DocumentEnd,
            ))
            .unwrap();
        document
            .apply(typora_core::EditorCommand::InsertText(
                "\nEdited line from test.\n".to_string(),
            ))
            .unwrap();
        write_markdown_file(&path, &document.text()).unwrap();

        let saved = read_markdown_file(&path).unwrap();
        assert!(saved.text.contains("Original line."));
        assert!(saved.text.contains("Edited line from test."));
        fs::remove_file(path).unwrap();
    }
}
