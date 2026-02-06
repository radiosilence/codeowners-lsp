mod testdata;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use codeowners_lsp::pattern::{pattern_matches, pattern_subsumes, CompiledPattern};

fn bench_pattern_matching(c: &mut Criterion) {
    let data = testdata::generate(&testdata::TestDataConfig::default());
    let mut group = c.benchmark_group("pattern_matching");

    // Single pattern vs single path
    group.bench_function("pattern_matches_single", |b| {
        b.iter(|| pattern_matches("src/**/*.rs", "src/packages/auth/lib/validate.rs"));
    });

    // CompiledPattern single
    group.bench_function("compiled_pattern_single", |b| {
        let compiled = CompiledPattern::new("src/**/*.rs");
        b.iter(|| compiled.matches("src/packages/auth/lib/validate.rs"));
    });

    // Pattern against all files (parameterized by pattern type)
    let representative_patterns = [
        ("extension", "*.rs"),
        ("directory", "src/"),
        ("deep_wildcard", "src/**/*.rs"),
        ("catch_all", "*"),
        ("anchored_deep", "docs/**/*.md"),
    ];

    for (label, pattern) in &representative_patterns {
        group.throughput(Throughput::Elements(data.file_list.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("pattern_vs_50k_files", label),
            pattern,
            |b, pattern| {
                b.iter(|| {
                    data.file_list
                        .iter()
                        .filter(|f| pattern_matches(pattern, f))
                        .count()
                });
            },
        );
    }

    // CompiledPattern batch against all files
    group.throughput(Throughput::Elements(data.file_list.len() as u64));
    group.bench_function("compiled_batch_50k", |b| {
        let compiled = CompiledPattern::new("src/**/*.rs");
        b.iter(|| {
            data.file_list
                .iter()
                .filter(|f| compiled.matches(f))
                .count()
        });
    });

    group.finish();
}

fn bench_subsumption(c: &mut Criterion) {
    let data = testdata::generate(&testdata::TestDataConfig::default());
    let mut group = c.benchmark_group("subsumption");

    // Single check
    group.bench_function("single", |b| {
        b.iter(|| pattern_subsumes("src/lib/", "src/"));
    });

    // All-pairs on 200 patterns (simulates the O(n^2) diagnostics loop)
    let patterns: Vec<&str> = data.patterns.iter().take(200).map(|s| s.as_str()).collect();
    let n = patterns.len();
    group.throughput(Throughput::Elements((n * n) as u64));
    group.bench_function("all_pairs_200", |b| {
        b.iter(|| {
            let mut count = 0usize;
            for a in &patterns {
                for bb in &patterns {
                    if pattern_subsumes(a, bb) {
                        count += 1;
                    }
                }
            }
            count
        });
    });

    group.finish();
}

criterion_group!(benches, bench_pattern_matching, bench_subsumption);
criterion_main!(benches);
