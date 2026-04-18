mod testdata;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use codeowners_lsp::diagnostics::{compute_diagnostics_sync, DiagnosticConfig};
use codeowners_lsp::file_cache::FileCache;

fn bench_diagnostics(c: &mut Criterion) {
    let data = testdata::generate(&testdata::TestDataConfig::default());
    let config = DiagnosticConfig::default();
    let mut group = c.benchmark_group("diagnostics");

    // On-keystroke: no file cache (fast path)
    // Parameterized by rule count to show O(n^2) scaling
    for num_rules in [50, 200, 500, 1000] {
        let content = testdata::content_with_n_rules(&data, num_rules);
        group.throughput(Throughput::Elements(num_rules as u64));
        group.bench_with_input(
            BenchmarkId::new("sync_no_cache", num_rules),
            &content,
            |b, content| {
                b.iter(|| compute_diagnostics_sync(content, None, &config));
            },
        );
    }

    // On-save: with file cache (includes pattern-no-match checks)
    let file_cache = FileCache::from_files(data.file_list.clone());
    group.throughput(Throughput::Elements(1000));
    group.bench_function("sync_with_cache_1000", |b| {
        b.iter(|| compute_diagnostics_sync(&data.codeowners_content, Some(&file_cache), &config));
    });

    group.finish();
}

criterion_group!(benches, bench_diagnostics);
criterion_main!(benches);
