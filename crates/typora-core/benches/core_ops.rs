use criterion::{Criterion, criterion_group, criterion_main};
use typora_core::{CursorMove, DocumentState, EditorCommand};

fn insert_single_char(c: &mut Criterion) {
    let text = "# Title\n\nParagraph text.\n".repeat(50_000);
    c.bench_function("insert single char into large rope", |b| {
        b.iter_batched(
            || {
                let mut doc = DocumentState::from_text(&text);
                doc.apply(EditorCommand::MoveCursor(CursorMove::DocumentEnd))
                    .unwrap();
                doc
            },
            |mut doc| {
                doc.apply(EditorCommand::InsertText("x".to_string()))
                    .unwrap();
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, insert_single_char);
criterion_main!(benches);
