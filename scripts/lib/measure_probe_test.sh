#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/measure_probe.sh"

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
file="$scratch/mod.rs"

check_probe() {
    local before="$1" after="$2" line="$3" shifted="$4"
    printf '%s%s' "$before" "$after" > "$file"
    measure_probe "$file" 7 > "$scratch/log"
    {
        printf '%s' "$before"
        printf '#[allow(dead_code)]\nfn measure_engine_build_probe_7() -> u32 {\n    7\n}\n\n'
        printf '%s' "$after"
    } > "$scratch/expected"
    diff -u "$scratch/expected" "$file"
    diff -u <(printf 'probe %s: line %s, %s code lines shifted\n' "$file" "$line" "$shifted") "$scratch/log"
}

check_probe $'#[cfg(test)]\nuse crate::fixture;\n\nasync fn production() {}\n' \
    $'#[cfg(test)]\nmod tests {\n    // fixture\n}\n' 5 3
check_probe $'fn production() {}\n' \
    $'#[allow(unused)]\n#[cfg(test)]\n#[path = "tests.rs"]\nmod tests;\n' 2 4
check_probe $'fn production() {}\n' \
    $'#[cfg(test)]\n#[cfg_attr(\n    unix,\n    path = "tests.rs"\n)]\nmod tests;\n' 2 6
check_probe $'#[allow(dead_code)] // fixture\nasync fn production() {}\n' \
    $'#[cfg(test)]\nmod tests;\n' 3 2
check_probe $'fn production() {}\n' \
    $'#[cfg(test)] // fixture\n#[cfg_attr(\n    unix,\n    path = "tests.rs"\n)] // fixture\nmod tests;\n' 2 6

printf '#[cfg(test)]\nuse crate::fixture;\nfn production() {}\n' > "$file"
measure_probe "$file" 7 > "$scratch/log"
printf '#[cfg(test)]\nuse crate::fixture;\nfn production() {}\n\n#[allow(dead_code)]\nfn measure_engine_build_probe_7() -> u32 {\n    7\n}\n\n' > "$scratch/expected"
diff -u "$scratch/expected" "$file"
diff -u <(printf 'probe %s: line 5, 0 code lines shifted\n' "$file") "$scratch/log"

echo "measure_probe tests passed"
