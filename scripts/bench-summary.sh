#!/usr/bin/env bash
# Print a summary table of all benchmark results from criterion JSON output.
# Usage: cargo bench && ./scripts/bench-summary.sh
set -euo pipefail

echo "=== codeowners-lsp Benchmark Summary ==="
echo ""
printf "%-55s %12s\n" "Benchmark" "Median"
printf "%-55s %12s\n" "---------" "------"

find target/criterion -path "*/new/estimates.json" -type f | sort | while read -r est; do
    # Extract benchmark name from path
    name=$(echo "$est" | sed 's|target/criterion/||;s|/new/estimates.json||')

    # Read median point estimate (in nanoseconds) and convert to human-readable
    result=$(python3 -c "
import json
with open('$est') as f:
    d = json.load(f)
v = d['median']['point_estimate']
if v >= 1_000_000_000:
    print(f'{v/1e9:.2f} s')
elif v >= 1_000_000:
    print(f'{v/1e6:.2f} ms')
elif v >= 1_000:
    print(f'{v/1e3:.2f} us')
else:
    print(f'{v:.0f} ns')
" 2>/dev/null) || continue

    [ -z "$result" ] && continue

    printf "%-55s %12s\n" "$name" "$result"
done
