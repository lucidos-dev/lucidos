#!/usr/bin/env bash
#
# e2e_projects.sh: which browser project runs which spec. Sourced by
# scripts/lib/e2e.sh and scripts/lib/harden_suites.sh. Defines functions only,
# so a caller can source it without the e2e harness.

# The specs desktop-webkit runs, one file name per line. playwright.config.ts
# reads the same file through e2e/projectRules.ts.
E2E_DESKTOP_WEBKIT_SPECS="${E2E_DESKTOP_WEBKIT_SPECS:-$(dirname "${BASH_SOURCE[0]}")/../../crates/lucidos-app/e2e/desktop-webkit-specs.txt}"

# ── project_runs_spec ────────────────────────────────────────────────
# Succeed when a browser project runs the given spec, fail when it ignores it.
#
#   project_runs_spec mobile header.spec.ts   # → fails
#
# Mirrors each project's `testIgnore` and `testMatch` in
# crates/lucidos-app/playwright.config.ts: chromium skips `-mobile`, the
# phone projects run only `-mobile` and `-all`, and desktop-webkit runs the
# specs $E2E_DESKTOP_WEBKIT_SPECS names. crates/lucidos-app/e2e/project-rules.test.ts
# compares the two. A targeted `-f` run asks every project for the spec. A
# project that ignores it finds no tests and exits 1, which fails a run that
# passed.
project_runs_spec() {
    case "$1:$2" in
        chromium:*-mobile.spec.ts) return 1 ;;
        chromium:*) return 0 ;;
        desktop-webkit:*)
            grep -qxF -- "${2##*/}" "$E2E_DESKTOP_WEBKIT_SPECS"
            return
            ;;
        *:*-mobile.spec.ts | *:*-all.spec.ts) return 0 ;;
    esac
    return 1
}

# ── e2e_webkit_projects / is_webkit_project ──────────────────────────
# The projects that drive WebKit, one per line, in run order. `--webkit` runs
# these and `--no-webkit` drops them. GitHub runs them on macOS.
e2e_webkit_projects() {
    printf '%s\n' desktop-webkit mobile-webkit
}

is_webkit_project() {
    e2e_webkit_projects | grep -qxF -- "$1"
}
