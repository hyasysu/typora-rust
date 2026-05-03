use criterion::{Criterion, criterion_group, criterion_main};
use typora_core::{DocVersion, DocumentId};
use typora_md::MarkdownParser;
use typora_render::{BlockLayoutEngine, LayoutConfig};

fn render_large_snapshot(c: &mut Criterion) {
    let markdown =
        "# Heading\n\nParagraph with some text and `code`.\n\n- item\n- item\n\n".repeat(20_000);
    c.bench_function("render 10mb-class snapshot layout", |b| {
        b.iter_batched(
            || {
                let mut parser = MarkdownParser::new().unwrap();
                parser.parse(DocumentId(1), DocVersion(1), &markdown)
            },
            |snapshot| {
                let engine = BlockLayoutEngine::new(LayoutConfig::default());
                let blocks = engine.render_blocks(&snapshot);
                engine.layout(&blocks, None)
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, render_large_snapshot);
criterion_main!(benches);
