mod testdata;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use codeowners_lsp::parser::parse_codeowners_file_with_positions;
use codeowners_lsp::validation::{validate_owner, validate_pattern};

fn bench_parse_with_positions(c: &mut Criterion) {
    let data = testdata::generate(&testdata::TestDataConfig::default());
    let mut group = c.benchmark_group("parsing");

    for size in [100, 500, 1000] {
        let content = testdata::content_with_n_rules(&data, size);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::new("parse_with_positions", size),
            &content,
            |b, content| {
                b.iter(|| parse_codeowners_file_with_positions(content));
            },
        );
    }

    group.finish();
}

fn bench_validation(c: &mut Criterion) {
    let data = testdata::generate(&testdata::TestDataConfig::default());
    let mut group = c.benchmark_group("validation");

    group.throughput(Throughput::Elements(data.patterns.len() as u64));
    group.bench_function("validate_pattern_1000", |b| {
        b.iter(|| {
            for p in &data.patterns {
                let _ = validate_pattern(p);
            }
        });
    });

    group.throughput(Throughput::Elements(data.owners.len() as u64));
    group.bench_function("validate_owner_250", |b| {
        b.iter(|| {
            for o in &data.owners {
                let _ = validate_owner(o);
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_parse_with_positions, bench_validation);
criterion_main!(benches);
