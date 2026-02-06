mod testdata;

use criterion::{criterion_group, criterion_main, Criterion};

use codeowners_lsp::file_cache::FileCache;
use codeowners_lsp::handlers::lens::code_lenses;
use codeowners_lsp::handlers::navigation::find_references;
use codeowners_lsp::handlers::semantic::{folding_ranges, semantic_tokens};
use codeowners_lsp::handlers::symbols::document_symbols;
use codeowners_lsp::ownership::{check_file_ownership, check_file_ownership_parsed};
use codeowners_lsp::parser::parse_codeowners_file_with_positions;

use tower_lsp::lsp_types::{Position, Url};

fn bench_semantic(c: &mut Criterion) {
    let data = testdata::generate(&testdata::TestDataConfig::default());
    let mut group = c.benchmark_group("handlers_semantic");

    group.bench_function("semantic_tokens_1000", |b| {
        b.iter(|| semantic_tokens(&data.codeowners_content));
    });

    group.bench_function("folding_ranges_1000", |b| {
        b.iter(|| folding_ranges(&data.codeowners_content));
    });

    group.bench_function("document_symbols_1000", |b| {
        b.iter(|| document_symbols(&data.codeowners_content));
    });

    group.finish();
}

fn bench_code_lenses(c: &mut Criterion) {
    let data = testdata::generate(&testdata::TestDataConfig::default());
    let file_cache = FileCache::from_files(data.file_list.clone());

    // Warm the cache so we benchmark lens logic, not pattern matching
    for p in &data.patterns {
        file_cache.count_matches(p);
    }

    let mut group = c.benchmark_group("handlers_lenses");
    group.bench_function("code_lenses_warm_1000", |b| {
        b.iter(|| code_lenses(&data.codeowners_content, &file_cache));
    });
    group.finish();
}

fn bench_ownership(c: &mut Criterion) {
    let data = testdata::generate(&testdata::TestDataConfig::default());
    let mut group = c.benchmark_group("handlers_ownership");

    // Pick a file from the middle of the list
    let test_file = &data.file_list[data.file_list.len() / 2];

    group.bench_function("check_file_ownership", |b| {
        b.iter(|| check_file_ownership(&data.codeowners_content, test_file));
    });

    let lines = parse_codeowners_file_with_positions(&data.codeowners_content);
    group.bench_function("check_file_ownership_parsed", |b| {
        b.iter(|| check_file_ownership_parsed(&lines, test_file));
    });

    group.finish();
}

fn bench_references(c: &mut Criterion) {
    let data = testdata::generate(&testdata::TestDataConfig::default());
    let uri = Url::parse("file:///CODEOWNERS").unwrap();

    let mut group = c.benchmark_group("handlers_navigation");

    // Find an owner that appears on a known line
    let pos = Position {
        line: 5,
        character: 15,
    };
    group.bench_function("find_references_1000", |b| {
        b.iter(|| find_references(&data.codeowners_content, pos, &uri));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_semantic,
    bench_code_lenses,
    bench_ownership,
    bench_references
);
criterion_main!(benches);
