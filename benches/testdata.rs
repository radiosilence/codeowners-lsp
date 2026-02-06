use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

#[allow(dead_code)]
pub struct TestData {
    pub codeowners_content: String,
    pub file_list: Vec<String>,
    pub patterns: Vec<String>,
    pub owners: Vec<String>,
}

#[allow(dead_code)]
pub struct TestDataConfig {
    pub num_rules: usize,
    pub num_files: usize,
    pub num_owners: usize,
    pub num_teams: usize,
    pub seed: u64,
}

impl Default for TestDataConfig {
    fn default() -> Self {
        Self {
            num_rules: 1000,
            num_files: 50_000,
            num_owners: 200,
            num_teams: 50,
            seed: 42,
        }
    }
}

const TOP_DIRS: &[&str] = &[
    "src", "lib", "packages", "apps", "services", "docs", "scripts", "config", "test", "tools",
    "internal", "vendor", "proto", "deploy", "infra", "api", "web", "mobile", "shared", "core",
];

const MID_DIRS: &[&str] = &[
    "auth",
    "payments",
    "billing",
    "api",
    "core",
    "common",
    "shared",
    "utils",
    "helpers",
    "models",
    "views",
    "controllers",
    "middleware",
    "handlers",
    "routes",
    "schemas",
    "types",
    "config",
    "db",
    "cache",
    "queue",
    "events",
    "notifications",
    "analytics",
    "logging",
    "metrics",
    "health",
    "admin",
    "users",
    "teams",
    "orgs",
    "repos",
    "issues",
    "pulls",
];

const LEAF_DIRS: &[&str] = &[
    "internal",
    "tests",
    "fixtures",
    "mocks",
    "stubs",
    "helpers",
    "utils",
    "lib",
    "src",
    "generated",
    "migrations",
    "seeds",
    "templates",
    "assets",
    "static",
    "public",
];

const EXTENSIONS: &[&str] = &[
    "rs",
    "ts",
    "tsx",
    "js",
    "jsx",
    "json",
    "yaml",
    "yml",
    "md",
    "toml",
    "go",
    "py",
    "proto",
    "sql",
    "graphql",
    "css",
    "scss",
    "html",
    "sh",
    "dockerfile",
];

const FILE_NAMES: &[&str] = &[
    "main",
    "index",
    "lib",
    "mod",
    "utils",
    "helpers",
    "config",
    "types",
    "schema",
    "model",
    "controller",
    "handler",
    "middleware",
    "router",
    "service",
    "client",
    "server",
    "worker",
    "consumer",
    "producer",
    "validator",
    "serializer",
    "deserializer",
    "formatter",
    "parser",
    "lexer",
    "compiler",
    "resolver",
    "loader",
    "factory",
    "builder",
    "adapter",
    "decorator",
    "observer",
    "strategy",
    "command",
    "query",
    "event",
    "listener",
    "subscriber",
    "publisher",
    "README",
    "CHANGELOG",
    "LICENSE",
    "Makefile",
    "Dockerfile",
];

/// Generate synthetic test data deterministically.
#[allow(dead_code)]
pub fn generate(config: &TestDataConfig) -> TestData {
    let mut rng = StdRng::seed_from_u64(config.seed);

    let owners = generate_owners(&mut rng, config.num_owners, config.num_teams);
    let file_list = generate_files(&mut rng, config.num_files);
    let (patterns, codeowners_content) =
        generate_codeowners(&mut rng, config.num_rules, &owners, &file_list);

    TestData {
        codeowners_content,
        file_list,
        patterns,
        owners,
    }
}

fn generate_owners(rng: &mut StdRng, num_owners: usize, num_teams: usize) -> Vec<String> {
    let mut owners = Vec::with_capacity(num_owners + num_teams);

    for i in 0..num_owners {
        owners.push(format!("@user-{}", i));
    }
    for i in 0..num_teams {
        let org = if i % 3 == 0 {
            "platform"
        } else if i % 3 == 1 {
            "product"
        } else {
            "infra"
        };
        owners.push(format!("@{}/team-{}", org, i));
    }

    owners.shuffle(rng);
    owners
}

fn generate_files(rng: &mut StdRng, num_files: usize) -> Vec<String> {
    let mut files = Vec::with_capacity(num_files);

    while files.len() < num_files {
        let depth = rng.gen_range(1..=6);
        let mut path_parts = Vec::with_capacity(depth + 1);

        // Top-level dir
        path_parts.push(TOP_DIRS[rng.gen_range(0..TOP_DIRS.len())].to_string());

        // Middle dirs
        for _ in 1..depth {
            let dir = if rng.gen_bool(0.6) {
                MID_DIRS[rng.gen_range(0..MID_DIRS.len())]
            } else {
                LEAF_DIRS[rng.gen_range(0..LEAF_DIRS.len())]
            };
            path_parts.push(dir.to_string());
        }

        // Filename
        let name = FILE_NAMES[rng.gen_range(0..FILE_NAMES.len())];
        let ext = EXTENSIONS[rng.gen_range(0..EXTENSIONS.len())];

        // Some files don't have extensions (Makefile, Dockerfile, etc.)
        if name == "Makefile" || name == "Dockerfile" || name == "LICENSE" {
            path_parts.push(name.to_string());
        } else {
            path_parts.push(format!("{}.{}", name, ext));
        }

        files.push(path_parts.join("/"));
    }

    files.sort();
    files.dedup();
    files.truncate(num_files);
    files
}

fn generate_codeowners(
    rng: &mut StdRng,
    num_rules: usize,
    owners: &[String],
    files: &[String],
) -> (Vec<String>, String) {
    let mut lines = Vec::new();
    let mut patterns: Vec<String> = Vec::new();

    lines.push("# Auto-generated CODEOWNERS for benchmarking".to_string());
    lines.push(String::new());

    let mut rule_count = 0;
    while rule_count < num_rules {
        // Occasionally add section comments
        if rule_count > 0 && rule_count % 50 == 0 {
            lines.push(String::new());
            lines.push(format!("# Section {}", rule_count / 50));
        }

        let roll: f64 = rng.gen();
        let pattern = if roll < 0.15 {
            // Extension wildcard
            let ext = EXTENSIONS[rng.gen_range(0..EXTENSIONS.len())];
            format!("*.{}", ext)
        } else if roll < 0.35 {
            // Directory pattern from existing files
            let file = &files[rng.gen_range(0..files.len())];
            let parts: Vec<&str> = file.split('/').collect();
            let depth = rng.gen_range(1..=parts.len().min(3));
            format!("/{}/", parts[..depth].join("/"))
        } else if roll < 0.50 {
            // Deep wildcard
            let top = TOP_DIRS[rng.gen_range(0..TOP_DIRS.len())];
            let ext = EXTENSIONS[rng.gen_range(0..EXTENSIONS.len())];
            format!("{}/**/*.{}", top, ext)
        } else if roll < 0.65 {
            // Anchored directory
            let top = TOP_DIRS[rng.gen_range(0..TOP_DIRS.len())];
            let mid = MID_DIRS[rng.gen_range(0..MID_DIRS.len())];
            format!("/{}/{}/", top, mid)
        } else if roll < 0.75 {
            // Exact file from the file list
            let file = &files[rng.gen_range(0..files.len())];
            format!("/{}", file)
        } else if roll < 0.80 {
            // Catch-all variants
            if rng.gen_bool(0.5) {
                "*".to_string()
            } else {
                "**".to_string()
            }
        } else if roll < 0.90 {
            // Nested extension wildcard
            let top = TOP_DIRS[rng.gen_range(0..TOP_DIRS.len())];
            let ext = EXTENSIONS[rng.gen_range(0..EXTENSIONS.len())];
            format!(
                "{}/{}/**/*.{}",
                top,
                MID_DIRS[rng.gen_range(0..MID_DIRS.len())],
                ext
            )
        } else if roll < 0.95 {
            // Intentional shadowed duplicate (pick an earlier pattern)
            if !patterns.is_empty() {
                patterns[rng.gen_range(0..patterns.len())].clone()
            } else {
                "*.rs".to_string()
            }
        } else if roll < 0.98 {
            // Invalid pattern
            if rng.gen_bool(0.5) {
                "[broken".to_string()
            } else {
                "[invalid-class]".to_string()
            }
        } else {
            // No-match pattern
            format!("/nonexistent-{}/deep/path/", rng.gen_range(0..10000))
        };

        // Generate owners for this rule
        let num_owners = if rng.gen_bool(0.02) {
            0 // No owners (intentional)
        } else {
            rng.gen_range(1..=3)
        };

        let mut rule_owners: Vec<&str> = Vec::new();
        for _ in 0..num_owners {
            let owner = &owners[rng.gen_range(0..owners.len())];
            rule_owners.push(owner);
        }

        // Occasionally add duplicate owner
        if rng.gen_bool(0.03) && !rule_owners.is_empty() {
            let dup = rule_owners[0];
            rule_owners.push(dup);
        }

        // Occasionally add invalid owner
        if rng.gen_bool(0.02) {
            rule_owners.push("not-an-owner");
        }

        let line = if rule_owners.is_empty() {
            pattern.clone()
        } else {
            format!("{} {}", pattern, rule_owners.join(" "))
        };

        patterns.push(pattern);
        lines.push(line);
        rule_count += 1;
    }

    let content = lines.join("\n") + "\n";
    (patterns, content)
}

/// Generate a CODEOWNERS content string with exactly `n` rules (for parameterized benchmarks).
/// Uses a subset of the given data's patterns.
#[allow(dead_code)]
pub fn content_with_n_rules(data: &TestData, n: usize) -> String {
    let mut lines = Vec::new();
    lines.push("# Benchmark CODEOWNERS".to_string());

    for line in data.codeowners_content.lines().skip(2) {
        // skip header comment and blank line
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        lines.push(line.to_string());
        if lines.len() > n {
            break;
        }
    }

    lines.join("\n") + "\n"
}
