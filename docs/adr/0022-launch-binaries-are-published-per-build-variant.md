# 0022 — A workspace launches a *published* binary, one directory per build variant — never cargo's shared uplift path

- **Status**: Accepted. The published directory's LOCATION is amended by
  [ADR 0063](0063-launch-binaries-live-outside-cargo-target.md)
- **Date** — 2026-07-27

> **Location amended.** Everything below stands except the literal path. The
> published directory is now `.launch/<profile>/<variant>/`, not
> `target/<profile>/launch/<variant>/`: keeping it under `target/` meant a
> `cargo clean` deleted the `lucidos` CLI out from under a running engine, which
> disabled every trigger and blocked `run_coding_agent` for eight hours on
> 2026-08-13. The "Why not a per-workspace staged binary" section below is still
> correct, and its three blockers are exactly what constrain the new location to
> sit inside the CHECKOUT. See ADR 0063 for why a checkout-local dot-dir
> satisfies all three.

## Context

`target/<profile>/lucidos-engine` is a **single output path that every cargo
variant in the checkout uplifts to**, and the last writer wins. Until now
`build_or_find_engine` (`scripts/lib/workspace.sh`) launched straight from it
and exported it as `LUCIDOS_ENGINE_BIN`, so the binary a workspace ran — and the
binary its engine read back through `current_exe --build-id` to decide whether a
new version exists — was whatever most recently landed there.

Three distinct writers were verified on the live checkout:

1. **Feature unification through a dev-dependency.** `crates/lucidos-e2e`
   declared `lucidos-engine = { …, features = ["e2e-test-hooks"] }` as a
   **dev-dependency**. `cargo tree -e features -i lucidos-engine --workspace`
   confirmed the unification, and `crates/lucidos-engine/tests/` exists, so at
   workspace scope — a bare `cargo test`, `cargo build --all-targets`,
   rust-analyzer's `cargo check --workspace --all-targets` — cargo built the
   engine **with test hooks on** and uplifted that bin over the shared path. That
   binary replaces the real web-push transport with an in-process logger and
   exposes `/api/v1/_test/*`, so a dev workspace launching it silently stops
   delivering push notifications.
2. **The e2e harness's deliberate variant build** (`ENGINE_BUILD_FEATURES=e2e-test-hooks`),
   which uplifts over `target/release/lucidos-engine` — the same path `run.sh`
   and `web-dev.sh -r` launch from — or over the debug path under
   `LUCIDOS_E2E_DEBUG=1`.
3. **A build that outlives its own commit.** `build.rs` stamps `ENGINE_BUILD_ID`
   when the build script *runs*, so a build started at commit N that finishes
   after an Apply moved main to N+1 uplifts an N binary over an N+1 one.

The 2026-07-26 forensics found `target/debug/lucidos-engine` and the newest
`deps/lucidos_engine-*` artifact **both reporting a two-commits-old build id**
while the build script's output correctly held HEAD's — i.e. the build script
had re-run but the crate was never relinked, the signature of writer (3)
combined with `run_engine_build`'s `kill_on_drop` coalescing killing builds
mid-compile. (Those two files were also different inodes of different sizes,
which read at the time as two distinct builds. It is simpler than that: the
uplifted binary is re-signed by `sign_engine_binary` and the `deps/` artifact is
still ad-hoc linker-signed, and re-signing shifts the size by ~1.4 MB —
reproduced exactly while implementing this ADR. The size delta is the codesign
step, not evidence of a second build.) Downstream: a running engine reporting
one id and the path it was launched from reporting another, an endless "New
version available" toast whose *Switch* was a downgrade, and 152 logged rebuild
cycles. The engine-side half (never offer a provably older binary) landed
separately as `docs/plans/2026-07-26-downgrade-switch-toast-loop.md`; this ADR
is the cause.

Writer (1) is therefore an independently verified hazard rather than the proven
mechanism of that particular incident — but it is the one that silently swaps a
dev workspace onto an engine with no working push transport, so it is fixed
here too.

## Decision

**A completed build PUBLISHES its outputs, and every launch path uses the
published copy.**

`build_or_find_engine` copies `lucidos-engine`, `lucidos-gateway` and the
`lucidos` CLI out of cargo's uplift directory into

```
target/<profile>/launch/<variant>/
```

where `<profile>` is `debug` | `release` and `<variant>` is `plain` for a
default build or a slug of `ENGINE_BUILD_FEATURES` (`e2e-test-hooks`). That
directory is written **only by completed builds of the same profile and feature
variant**, so `ENGINE_BIN` / `GATEWAY_BIN` / `LUCIDOS_ENGINE_BIN` and the
engine's own `current_exe()` all resolve to a path no other configuration can
touch. Cargo keeps uplifting to `target/<profile>/lucidos-engine`; nothing
launches from it any more.

Four supporting rules:

| Rule | Why |
|---|---|
| Publish via temp file + `mv -f` (same-filesystem rename) | The path only ever holds a COMPLETE binary; a running engine keeps its own inode |
| A failed publish leaves the previously published binary untouched, and the no-build path falls back to cargo's uplift path with a warning | A build must never make the launch path missing — `No engine binary found. Run with -b` would strand every co-located workspace |
| After publishing, compare the binary's build-id commit against HEAD; rebuild **once** on a mismatch, then warn and succeed | Catches writer (3). Failing here would surface a false "New engine version failed to build" and abort the Apply-triggered rebuild for every peer |
| `crates/lucidos-e2e` no longer requests `e2e-test-hooks` | Kills writer (1) at the source; the hooks belong to the engine BINARY the harness builds, not to the test crate's linkage |

## Rationale

**Why not a separate `CARGO_TARGET_DIR` per variant.** It gives the same
single-writer guarantee, but a second target dir shares no dependency cache, so
the e2e harness would pay a full cold compile of the release dep graph
(wasmtime / aws-lc / ravif) plus several GB of disk on every fresh checkout. A
published copy buys the identical guarantee for ~250 MB and a sub-second `cp`
(a `clonefile` on APFS).

**Why not a per-workspace staged binary** (`<workspace>/.lucidos/bin/lucidos-engine`).
Three hard blockers, each load-bearing:

- The engine resolves its checkout by walking `current_exe()`'s ancestors for
  `scripts/web-dev.sh` (`crates/lucidos-engine/src/paths.rs`). A workspace-local
  copy has no such ancestor, so `paths::script("web-dev.sh")` fails and
  `run_engine_build` can no longer rebuild at all, `engine_build_lock_path()`
  returns `None` (co-located workspaces lose their shared build lock), and
  `engine_source_matches_head` — the INV-A veto behind every frontend-only
  Apply — loses its repo.
- ADR 0021's worktree refusal is a **pure path test** on `LUCIDOS_ENGINE_BIN`.
  Staging outside the checkout would launder a worktree-built binary straight
  past it.
- `<workspace>/.lucidos/bin` is already taken: it is where
  `ensure_workspace_bin_symlink` installs the `lucidos` CLI symlink.

Keeping the published directory under `target/` preserves all three for free.

**Why the CLI is published too.** `find_lucidos_cli_dir`
(`crates/lucidos-engine/src/runtime/lucidos_cli.rs`) walks up from the engine's
exe dir looking for a `lucidos` sibling, and the engine prepends that directory
to `PATH` for spawned coding-agent sessions. Without publishing it, the
lucidos-cli skill would silently stop being installed.

**Why verification warns instead of failing.** The compile genuinely succeeded;
what failed is only the claim "this is the source on disk now". The engine's
direction guard already decides whether to *offer* a binary, and
`self_heal_is_wedged` already gives up once with an actionable log. A non-zero
exit here would instead paint the failure as a compile error and abort the
background rebuild for every co-located workspace.

**Why an indeterminate build id is not a mismatch.** No git, an unreadable id,
or a no-git `src-…` id cannot be fixed by rebuilding — treating them as stale
would double every build forever. This is the same asymmetry the engine-side
direction guard uses.

## Consequences

- The first launch after this change has no published launch binary yet, so it falls
  back to cargo's uplift path **with a warning**; the next `-b` publishes. No
  workspace is stranded by the transition.
- `target/` grows by roughly one engine + gateway + CLI per profile × variant in
  active use (~250 MB for a debug set). `cargo clean` removes them along with
  everything else.
- A macOS TCC grant survives the path change: the dev Designated Requirement is
  `identifier + certificate leaf` with no CDHash and no path, and
  `sign_engine_binary` now signs the published copies — the binaries actually
  launched.
- **`cargo test` at workspace scope now runs the engine's unit tests against the
  REAL web-push transport**, matching what `./scripts/test-engine.sh`
  (`-p lucidos-engine`) always did. Anything that silently depended on the stub
  being compiled in at workspace scope will now see the real code path.
- No automated guard stops the dev-dependency feature from creeping back: a
  `cargo tree` assertion cannot run inside `cargo test` (nested cargo blocks on
  the target-dir lock), and a `cfg!(feature = …)` assertion would forbid a future
  legitimate hooks-on unit-test run. The structural fix is what makes a
  recurrence non-load-bearing — a re-added feature would once again poison
  `target/<profile>/lucidos-engine`, but nothing launches from there. The `why`
  is recorded as a comment in `crates/lucidos-e2e/Cargo.toml`.
- Release, installer and packaging paths are untouched:
  `scripts/build-headless.sh`, `scripts/build-dmg.sh`,
  `scripts/release-to-lucidos.sh`, `install.sh`, `scripts/lib/service.sh` and
  `crates/lucidos-app/src/desktop.rs` run their own cargo and stage from
  `target/release/…` directly.

## See also

- ADR 0014 — dev runtime topology (the gateway spawns engines by `LUCIDOS_ENGINE_BIN`)
- ADR 0021 — the long-lived stack never runs from a coding-agent worktree
- ADR 0063: the published directory moves out of `target/` so a `cargo clean`
  cannot delete the running system's `lucidos` CLI
- `docs/plans/2026-07-27-launch-binary-published-per-variant.md` — the implementation plan
- `docs/plans/2026-07-26-downgrade-switch-toast-loop.md` — the engine-side half (never offer a downgrade)
- `.claude/rules/dev-runtime.md` — the day-to-day rule for launching a workspace
