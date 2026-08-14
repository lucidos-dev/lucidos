# 0079: A cargo build script resolves CARGO_MANIFEST_DIR at run time, never with compile-time env!

- **Status**: Accepted
- **Date**: 2026-08-14

## Context

A dev workspace wedged on a recurring "New engine version failed to build"
toast whose Retry button never helped. Every attempt failed identically:

```
thread 'main' panicked at crates/lucidos-engine/build.rs:52:45:
Failed to create default VERSION: Os { code: 2, kind: NotFound }
```

The cached build-script binary in the checkout's own `target/` had
`/private/tmp/sweep-dryrun/crates/lucidos-engine` baked into it, from a
throwaway checkout that had since been deleted. All three engine build scripts
read their crate directory with `env!("CARGO_MANIFEST_DIR")`, which expands
when the script is COMPILED rather than when cargo runs it.

Two checkouts of one package name and version get the same `-C metadata` hash.
So with a shared `CARGO_TARGET_DIR`, cargo considers the first checkout's
artifact fresh for the second and hands it over. That was reproduced directly:
building in directory `a` and then `b` against one target directory made both
runs print `a`'s path.

Nothing then expires the poison. Cargo believes the artifact is fresh, so every
self-heal attempt and every manual rebuild replays the same broken binary.

## Decision

A cargo build script resolves `CARGO_MANIFEST_DIR` through
`std::env::var` at run time, and never through compile-time `env!`. The same
holds for `CARGO_MANIFEST_PATH`. `./scripts/check-build-script-paths.sh`
enforces it for every diff in `/harden` Phase 4.5.

Sharing a `CARGO_TARGET_DIR` across checkouts stays allowed.

## Rationale

Cargo sets `CARGO_MANIFEST_DIR` when it RUNS a build script. Reading it there
means a binary reused across checkouts asks where it is, instead of
remembering where it was built. The bug becomes unrepresentable rather than
merely discouraged, which is the house preference.

The gate is deterministic rather than a review habit because two of the three
failures were **silent**. Only the engine's build script panics. The gateway's
`git` call fails in a missing directory, so `GATEWAY_BUILD_ID` collapses to one
constant and the picker's new-gateway badge stops firing. The app's does the
same and stamps `0000.00.00.0`. Neither would fail a test, and neither had been
noticed.

The gate is scoped to a `build.rs` sitting beside a `Cargo.toml`, which is
cargo's own rule for what a build script is. Compile-time `env!` stays correct
in ordinary crate code, where no cargo variable is set at run time at all. That
scoping is load-bearing: `repo_root_or_compile_time_fallback` in
`crates/lucidos-engine/src/paths.rs` uses the banned form legitimately, and the
repo also has an unrelated `build.rs` under `src/` that cargo never runs.

## Consequences

- A build script is path-independent, so a shared target directory, a copied
  tree, or a `/tmp` scratch checkout can no longer poison a sibling.
- The build id and the CalVer version are computed against the checkout being
  built, so both stop lying after a cross-checkout artifact reuse.
- A build script that runs outside cargo now fails with a named cause instead
  of a bare ENOENT.
- One more whole-tree gate in `/harden` Phase 4.5, costing milliseconds.
- A crate that renames its script through `package.build` is not discovered.
  Parsing TOML to find it is deferred until a crate does that.

## Alternatives considered

**Forbid a shared `CARGO_TARGET_DIR`.** Rejected. It is a legitimate and
documented optimization, and the ban would have to reach every ad-hoc script
and every agent that ever copies the tree. A convention that must hold in
places we do not control is not a fix. Making the artifact correct wherever it
lands is.

**Validate the baked path at run time and fall back.** Rejected. It keeps two
sources of truth for one answer and only papers over the wrong one. The
run-time read is strictly simpler and has no failure mode to detect.

**Clear the cache when it breaks.** Rejected as the *fix*, though it is the
remedy for an already-poisoned tree. It treats the symptom, and the symptom
recurs whenever a scratch checkout meets a shared target directory again.

**Leave it to code review.** Rejected. The two silent failures are exactly what
review misses, and the correct and incorrect forms differ by one token.

**Make sccache the culprit.** Investigated and ruled out, recorded here so it
is not re-investigated. The repo sets `rustc-wrapper = "sccache"`, which makes
it the obvious suspect. It is not: `-C metadata` differs per package path, so
two crates in different directories each bake their own correct path. Only a
shared target directory collides.
