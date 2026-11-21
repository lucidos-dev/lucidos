#!/usr/bin/env bash
# Insert before the first top-level test module to preserve production spans
# where tests follow production code, and satisfy clippy's
# items_after_test_module. With no test module, append. The shift count
# includes test code and excludes blank lines and line comments.
measure_probe() ( # <file> <sequence>
    local file="$1" sequence="$2" scratch
    scratch="$(mktemp "$file.probe.XXXXXX")" || return
    trap 'rm -f "$scratch"' EXIT
    local report
    report="$(awk -v n="$sequence" -v output="$scratch" '
        function emit() {
            printf "#[allow(dead_code)]\nfn measure_engine_build_probe_%s() -> u32 {\n    %s\n}\n\n", n, n > output
        }
        {
            lines[NR] = $0
            if (anchor) next
            ends_attribute = /\][[:space:]]*(\/\/.*)?$/
            if (/^#\[/ || /^\/\/\//) {
                if (!attributes) attributes = NR
                if (/^#\[cfg\(test\)\][[:space:]]*(\/\/.*)?$/) test = 1
                multiline = /^#\[/ && !ends_attribute
                next
            }
            if (multiline) {
                if (ends_attribute) multiline = 0
                next
            }
            if (test && /^(pub(\([^)]*\))?[[:space:]]+)?mod[[:space:]]+/)
                anchor = attributes
            if (!/^[[:space:]]*($|\/\/)/) attributes = test = 0
        }
        END {
            for (i = 1; i <= NR; i++) {
                if (i == anchor) emit()
                print lines[i] > output
                if (anchor && i >= anchor && lines[i] !~ /^[[:space:]]*($|\/\/)/)
                    shifted++
            }
            if (!anchor) {
                print "" > output
                anchor = NR + 2
                emit()
            }
            if (close(output) != 0) exit 1
            printf "line %d, %d code lines shifted", anchor, shifted
        }
    ' "$file")" || return
    mv "$scratch" "$file" || return
    printf 'probe %s: %s\n' "$file" "$report"
)
