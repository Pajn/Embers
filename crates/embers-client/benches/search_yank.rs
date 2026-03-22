use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use embers_client::{
    SelectionKind, SelectionPoint, SelectionState, configured_client::benchmark_search_matches,
    configured_client::benchmark_serialize_selection,
};

fn synthetic_lines(count: usize, width: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            format!(
                "{index:05} {} needle {index:05} {}",
                "abcd".repeat(width / 8),
                "wxyz".repeat(width / 8)
            )
        })
        .collect()
}

fn bench_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("search");
    for line_count in [1_000_usize, 10_000, 50_000] {
        let lines = synthetic_lines(line_count, 96);
        group.bench_with_input(BenchmarkId::from_parameter(line_count), &lines, |b, lines| {
            b.iter(|| benchmark_search_matches(lines, "needle"));
        });
    }
    group.finish();
}

fn bench_yank(c: &mut Criterion) {
    let mut group = c.benchmark_group("yank");
    for line_count in [1_000_usize, 10_000, 50_000] {
        let lines = synthetic_lines(line_count, 96);
        let selection = SelectionState {
            kind: SelectionKind::Character,
            anchor: SelectionPoint { line: 10, column: 0 },
            cursor: SelectionPoint {
                line: (line_count.saturating_sub(10)) as u64,
                column: 48,
            },
        };
        group.bench_with_input(BenchmarkId::from_parameter(line_count), &lines, |b, lines| {
            b.iter(|| benchmark_serialize_selection(lines, &selection));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_search, bench_yank);
criterion_main!(benches);
