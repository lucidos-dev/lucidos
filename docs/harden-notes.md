# Hardening notes

Why `/harden` runs what it runs. The procedure is `.claude/commands/harden.md`.
The suite rows live in code (`scripts/lib/harden_suites.sh`), and this page
explains the ones a reader would not guess.

## Suite rows

- **The piped installer has a row.** macOS `/bin/sh` is bash 3.2, so
  `install.sh`'s re-exec guard never fires there. Everything a `curl … | sh`
  reaches must stay in the bash-3.2 POSIX subset. ShellCheck reads those files as
  bash, so it passes the process substitution that broke the one-liner.
  `install_test.sh` holds the line, and nothing else in the local gate runs it.
- **The release path has a row.** `make lint` parses those scripts and says
  nothing about what they do. Two offline suites drive stubbed `gh` state that no
  other gate models: the DMG-gate dispatch with its failure classes, and the draft
  wait that decides whether Phase B adopts the rc tarballs or rebuilds all four.
  A regression there stays invisible until someone is mid-release.
- **A compile input selects the Rust suite, whatever its extension.** `start`
  lists cargo's dep-info plus every `include_str!` and `include_bytes!` literal.
  An edit to only `CHANGELOG.md`, `VERSION` or `sdk_iframe.css` therefore still
  runs `make lint && make test`. The guard often sits across the diff:
  `voice/language.rs` includes `LocaleSection.tsx` to check its dropdown, and
  `tsc` never compiles that guard. Hand-kept rows missed files this way.
- **A file the CLI source includes selects the CLI row, even an engine path.**
  `make test` covers the engine crate alone, and `make lint` compiles CLI tests
  without running them. CLI tests include engine files (the two menu JSONs, and
  `api/data_api.rs` with `dataPathPrefixes.ts`). `start` records those includes
  in `cli-inputs`.
- **A `system-knowhow/**` edit is not a docs-only skip.** Its `name` and
  `description` ride in the chat agent's routing list on every request. Two Rust
  tests cap them: `system_knowhow_descriptions_stay_routing_sized` and
  `always_loaded_context_stays_under_budget`. A docs-only skip once let the
  ceiling drift 794 characters over.
- **CSS is not a skip.** Nothing else parses CSS, so a syntax error once reached
  `main` and killed the shared build-watch's `vite build`. The watch kept
  serving the old `dist/`. The next frontend Apply then failed with "applied but
  not served yet", which names the build-watch, not the CSS file.
  `npx vite build` is the exact command the watch runs.
- **The two CSS surfaces need different gates.** `sdk_iframe.css` is
  `include_str!`d by `api/sdk.rs`, outside the Vite graph. Its guard is
  `styles/__tests__/engine-served-css-parses.test.ts`. `vite build` resolves
  `@import` where a per-file parse cannot, and the guard reaches a file
  `vite build` cannot see. Neither subsumes the other.
- **A first-paint input runs the growth check (ADR 0454).** The row selects a
  coarse path superset, and `first-paint-growth.mjs` decides. It builds the merge
  base and this tree, and fails growth over 2 kB raw with no
  `First-paint-growth:` trailer. On FAIL, move what the first frame does not draw
  behind a dynamic import, or add the trailer with the reason.
- **A frontend change runs the browser specs it can break (ADR 0502).** The
  `e2e-touched` row selects three kinds of spec. They are changed specs, specs
  that name a string literal the diff removed, and specs the main check's coverage
  map ties to a changed file.
  A backend-only diff selects none, and the fast tier drops the row.

## Why `make lint` and not `cargo check`

Lucidos is not PR-based, so `/harden` is the only stage between a change and
`main`. `cargo check` runs no clippy lints. Until 2026-07-29 the table said
`cargo check`, and the nightly then found nineteen lints accumulated over three
weeks. `make lint` is the same compile plus clippy with `CLIPPY_FLAGS`, ShellCheck
over every tracked `*.sh`, and a rustfmt-clean tree. Never restate its flags.

## Timeouts under load are contention, not findings

- **The Codex driver tests run alone.** Both modules spawn a `/bin/sh` stub with
  30 s and 60 s timeouts. A run that overlapped them with the engine suite took
  539 s against the usual 82 s, and twelve tests failed that passed alone.
  The early run skips both modules, and `wait` runs them by themselves.
- **A jsdom test that times out at exactly 5000 ms is contention.** The engine
  suite saturates every core, and an async Vitest case starves. The tell is a
  round 5000 ms in a file the diff never touched. Re-run it alone first.

## Never pipe a test command

A pipeline reports the last command's exit code. `cargo test … | tail` exits 0
when a Rust test failed, and Phase 4.5 then reports a false PASS. That has shipped
a failing nightly. Redirect to a log, capture `$?` first, then read the log. A
"tests pass" claim needs the real exit code and the `test result: ok.` line.

## A silent other-model review is configuration

A `NO VERDICT` line is never a pass. The reviewer started and returned nothing,
and the status line quotes its cause. Read the cause before calling it flaky.

- **Codex.** The companion folds the real cause into one generic sentence, and
  the status line quotes it back from the reviewer's stderr. The CLI once moved
  its default model to one the local login could not reach, and every review
  then returned a 403. `codex login status` names the login, and a reachable
  `model` in `~/.codex/config.toml` fixes it.
- **Claude.** The script quotes `result`, `subtype` and the served model from the
  JSON result. `claude auth status` names the login. A config dir with no login
  answers "Not signed in", including the default path set explicitly in
  `CLAUDE_CONFIG_DIR`. So the script never sets that variable, and inherits what
  the engine's own sessions inherit.

## Why a gate is unconditional

The gates under "Also always" run for every diff, docs-only included. They are
whole-tree or added-lines checks that cost milliseconds. A merge can break a
whole-tree gate with no edit on the branch. A docs-only diff is also the change
that grows prose, the context set and the knowhow.

- **`check-knowhow-refs.sh`**: the usual author of drift is the other side of the
  diff. A module becomes a directory, an event is renamed, and a knowhow file nobody
  edited starts lying.
- **`check-audit-scans.sh`**: a scan that misses a form reports zero hits, and the
  audit reads zero as clean. An argv list (`["psql",...]`) once never matched.
- **`hooks_registered_test.sh`**: a hook committed at mode 100644 never runs, and
  nothing says so. Several events ignore a hook's exit code by design.
