#!/usr/bin/env bash
#
# e2e_projects.sh: which browser project runs which spec. Sourced by
# scripts/lib/e2e.sh and scripts/lib/harden_suites.sh. Defines functions only,
# so a caller can source it without the e2e harness.

# ── project_runs_spec ────────────────────────────────────────────────
# Succeed when a browser project runs the given spec, fail when it ignores it.
#
#   project_runs_spec mobile header.spec.ts   # → fails
#
# Mirrors each project's `testIgnore` and `testMatch` in
# crates/lucidos-app/playwright.config.ts: chromium skips `-mobile`, and the
# phone projects run only `-mobile` and `-all`. crates/lucidos-app/e2e/project-rules.test.ts
# compares the two. A targeted `-f` run asks every project for the spec. A
# project that ignores it finds no tests and exits 1, which fails a run that
# passed.
project_runs_spec() {
    case "$1:$2" in
        chromium:*-mobile.spec.ts) return 1 ;;
        chromium:*) return 0 ;;
        *:*-mobile.spec.ts | *:*-all.spec.ts) return 0 ;;
    esac
    return 1
}
