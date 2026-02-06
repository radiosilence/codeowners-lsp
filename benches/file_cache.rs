mod testdata;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

use codeowners_lsp::file_cache::FileCache;
use codeowners_lsp::parser::parse_codeowners_file_with_positions;

fn bench_file_cache(c: &mut Criterion) {
    let data = testdata::generate(&testdata::TestDataConfig::default());
    let mut group = c.benchmark_group("file_cache");

    // Construction from file list
    group.bench_function("from_files_50k", |b| {
        let files = data.file_list.clone();
        b.iter(|| FileCache::from_files(files.clone()));
    });

    // count_matches - cold cache (first call per pattern)
    group.throughput(Throughput::Elements(data.file_list.len() as u64));
    group.bench_function("count_matches_cold", |b| {
        b.iter_batched(
            || FileCache::from_files(data.file_list.clone()),
            |cache| cache.count_matches("src/**/*.rs"),
            criterion::BatchSize::LargeInput,
        );
    });

    // count_matches - warm cache (subsequent calls)
    group.bench_function("count_matches_warm", |b| {
        let cache = FileCache::from_files(data.file_list.clone());
        cache.count_matches("src/**/*.rs"); // prime cache
        b.iter(|| cache.count_matches("src/**/*.rs"));
    });

    // find_patterns_with_matches - the parallel batch operation (1000 patterns, cold)
    let patterns: Vec<&str> = data.patterns.iter().map(|s| s.as_str()).collect();
    group.throughput(Throughput::Elements(data.patterns.len() as u64));
    group.bench_function("find_patterns_with_matches_cold_1000", |b| {
        b.iter_batched(
            || FileCache::from_files(data.file_list.clone()),
            |cache| cache.find_patterns_with_matches(&patterns),
            criterion::BatchSize::LargeInput,
        );
    });

    // get_unowned_files
    let rules = parse_codeowners_file_with_positions(&data.codeowners_content);
    group.throughput(Throughput::Elements(data.file_list.len() as u64));
    group.bench_function("get_unowned_files_50k", |b| {
        let cache = FileCache::from_files(data.file_list.clone());
        b.iter(|| cache.get_unowned_files(&rules));
    });

    group.finish();
}

criterion_group!(benches, bench_file_cache);
criterion_main!(benches);
