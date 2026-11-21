# 0562: A shard whose setup failed tested nothing: it exits 77, the report reads no verdict, and a real failing test still turns the main check red

- **Status**: Accepted
- **Date**: 2026-10-23
- **Amends**: [0464](0464-main-check-on-github.md)

## Context

The main check of 2026-10-22 went red with "Failed: browser chromium, browser
mobile". Both suites reported 0 failed tests. Two shards had died in the apt
warm-up, which stalled on a package mirror, before Playwright started. The
shard exited 1, the report read that as a failed suite, and the red opened a
fix thread with nothing to fix. Archiving that thread does not close a red, so
no check ran after it, and 313 commits landed unchecked.

## Decision

**A warm-up failure is a distinct exit code, 77, and the report maps it to no
verdict.** `e2e_warm_wait` returns `E2E_WARM_FAILED_EXIT` for a task that
failed, never started or timed out. `e2e_github_shard_rc` turns that code into
the no-verdict exit (76), with an `ERROR:` line naming the shard and the
warm-up task. The engine already turns no verdict into `MainCheckAbandoned`:
no red, no fix thread.

A failing test in any other shard still outranks no verdict
(`e2e_github_worse_rc`). A run with one setup failure and one real failure
stays red and names that failure.

The warm-up itself stops depending on one mirror. The Linux shards restore the
browser chain's `.deb` files from a cache. `seed-cache` fills it on main. Apt
attempts 2 and 3 use `archive.ubuntu.com` instead of the Azure mirror.

## Rationale

A shard that ran no test says nothing about main. This is the precedent
`HOST_MEMORY_BUILD_REFUSED_EXIT` set for a refused build. The shard script
owns the distinction as an exit code, so no reader matches log text to decide a
verdict.

## Consequences

- A flaky runner no longer turns main red. The next landing starts the next
  check, as for every no verdict.
- A broken warm-up that fails on every run shows as repeated abandoned checks,
  not as red. The abandoned summary names the shard and the task.
- Archiving a red fix thread still leaves the red open (ADR 0464). Setup
  failures no longer create the empty fix thread that invited it.

## Alternatives considered

- **Match "warm-up task ... failed" in the shard log, in `main_check.sh`.**
  Text matching decides a verdict by prose, and any rewording breaks it.
- **Retry the whole shard on a setup failure.** It hides a broken warm-up and
  doubles the cost of a run that was always going to be no verdict.
