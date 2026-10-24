# 0294: An engine build never signals another process to free cargo's lock: cargo waits on its own flock

- **Status**: Accepted
- **Date**: 2026-09-26

## Context

`make lint` failed with `make[1]: *** [lint-rust-clippy] Error 255` and no
compiler diagnostic. It failed at random points in the build, and only some of
the time. Every `/harden` run on the host could report the rust suite as FAIL on
a clean tree.

The cause was `build_or_find_engine` in `scripts/lib/workspace.sh`. Before each
engine build it sent SIGTERM to every `cargo` process on the host whose command
line held `cargo check`. It then deleted the `.cargo-lock` files under
`target/`. The intent was to clear an IDE or rust-analyzer check holding the
checkout's `target/` lock.

`cargo clippy` runs a child `cargo check`, so that child matched. `cargo-clippy`
reports a child killed by a signal as exit 255 and prints nothing. The selector
was host-wide, but the lock it meant to free belongs to one checkout. So every
Apply rebuild, self-heal build, `web-dev.sh -b` and e2e engine build killed
lints and checks in every other worktree.

## Decision

An engine build signals no other process and deletes no cargo lock file. When
another cargo holds the `target/` lock, the build waits for it.

## Rationale

**Cargo already serialises its own builds.** It takes an `flock` on
`.cargo-lock` and prints "Blocking waiting for file lock" while it waits. An IDE
check costs the engine build seconds of waiting. Killing it costs someone else
their build.

**Deleting the lock file never helped and could hurt.** An `flock` belongs to
an open file, and the kernel drops it when its holder dies. A lock file left
behind after sleep or wake therefore blocks nothing. Deleting it while a peer
holds it lets a new cargo lock a fresh inode, so two builds then share one
`target/`.

**Coordination has owners already.** The build slot caps heavy builds on the
host (ADR 0070). The engine build lock serialises engine-triggered builds in one
checkout. Neither needs a kill to work.

## Consequences

- A lint, test or agent `cargo check` in any worktree survives an engine build
  elsewhere.
- An engine build in a checkout where the IDE is mid-check starts a little
  later.
- `select_cargo_lock_holders` is gone. `engine_build_spares_other_builds_test.sh`
  pins that the build sends no signal and keeps the lock files.

## Alternatives considered

- **Scope the kill to processes whose cwd is inside this checkout.** It fixes
  the worktree case but still kills a human `make lint` in the main checkout
  during an Apply rebuild. It keeps the hazard and only narrows it.
- **Stop `make lint` from matching**, for example by renaming or hiding the
  inner cargo. That treats the victim. Every other `cargo check` on the host
  would stay a target.
