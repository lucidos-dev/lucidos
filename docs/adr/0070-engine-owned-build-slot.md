# 0070: A machine-wide build slot caps concurrent heavy builds; the build process holds the flock, the engine owns the policy

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

Every coding-agent thread gets its own *worktree*, and a worktree has its own
`target/`. So N agent sessions running `make lint` or `make test` are N full
`lucidos-engine` compiles resident at once. CLAUDE.md already forbids two
`cargo test` runs against the same worktree, because they OOM the host. The
same physics applies across worktrees, where nothing enforced a limit.

A multi-partition comment sweep hit this. Several Rust partitions ran in
parallel behind an ad-hoc `mkdir` lock pasted into their prompts. It was
undiscoverable, untested, and wedged on stale state, so it was discarded rather
than committed.

Two locks already exist and neither covers this. The **engine build lock**
(`engine_version.rs`) serialises engine-triggered builds inside ONE checkout,
protecting a shared `target/`. The **e2e lock** (`scripts/lib/e2e_lock.sh`)
hard-fails a second run, because two suites are never both wanted. A second
build IS wanted, just not concurrently.

## Decision

A **build slot** is one of N permits to run a heavy build on the host.

- **The build process holds it**, as an `fs2` flock in a machine-wide pool
  under `$HOME/.lucidos/build-slots/`.
- **The pool is machine-wide**, not per checkout and not per workspace.
- **N** resolves as `LUCIDOS_MAX_CONCURRENT_BUILDS`, then a capacity file next
  to the pool, then `max(1, total_GB / 16)` clamped to 8.
- **Over the limit a build waits**, with no built-in deadline, announcing
  contention as domain events so a session can subscribe rather than block.
- **It is not a queue.** Arrival order is not preserved.
- **Everything fails open.** No broker, no pool, or no engine means the build
  runs unrestricted.

Taken by `make lint`, `make test`, and `run_engine_cargo_build`. That last one
is the single cargo call reached by the e2e harness, a human `web-dev.sh -b`,
and the engine's own background rebuild.

## Rationale

**The engine cannot hold the count.** `cargo` runs as a detached grandchild of
an agent session's Bash tool, and Apply restarts engines routinely. An engine
that merely counted slots would leak every one on restart, while the compiles
carried on. It would need leases, heartbeats and an expiry sweep, which is the
stale-state problem rebuilt one layer up. A flock is released by the kernel on
process death, which is exactly what the `mkdir` lock could not do.

**The engine still owns the policy and the visibility**, which is the part that
belongs to it. It sets no count in code, but the count, the pool and the
reporting are Lucidos surfaces rather than a snippet in a prompt.

**Machine-wide matches the resource.** Host RAM is machine-wide, several
workspaces run at once by design, and an external-repo agent session builds a
project outside every Lucidos checkout. A per-workspace pool would not limit
anything real. It also settles configuration: a machine-wide pool cannot be
configured from a per-workspace store, so the count lives in a file beside the
pool it governs.

**Announcement goes in the doors people already use.** A Claude Code PreToolUse
hook reaches neither a human nor Codex, which has no hooks. Wiring the slot
into `make` and the build scripts covers everyone with nothing to remember.

**`macOS ships no flock binary` no longer rules shell-side locking out.** That
reasoning, recorded in `.claude/rules/dev-runtime.md`, was about the *shell*
being the broker. Here the broker is the `lucidos` binary taking an `fs2`
flock, and the shell only resolves where that binary is.

**Waiting rather than refusing** is the difference from the e2e lock. A second
build is wanted, so the loser queues. The wait is unbounded by default, because
a human at a terminal would happily wait. `--max-wait` exits 75, the
`EX_TEMPFAIL` code `host_load_guard.sh` already uses for backpressure.

## Consequences

- A killed build cannot wedge the pool, and there is no reclaim path to get
  wrong. This is the single property the design is built around.
- A plain `git clone` has no `lucidos` binary, so `make lint` runs
  unrestricted. Bootstrapping works for the same reason: the build that
  produces the binary cannot wait on it.
- A build that bypasses the wrapper takes no slot. A bare `cargo build` typed
  directly is unlimited, and no hook enforces otherwise.
- **A build orphaned from its wrapper keeps its slot freed.** SIGKILL the
  wrapper alone and the flock drops while `cargo` compiles on, so the pool can
  briefly exceed N. Every ordinary kill signals the whole process group, which
  the child stays in on purpose, so this needs a deliberate single-pid kill.
  The obvious fix is worse. Clear close-on-exec and the child inherits the
  lock, but then a leaked grandchild pins that slot until someone kills it.
  That is the stale-holder problem this design exists to avoid, traded for a
  narrow one that ends when the build does.
- **`BuildSlotReleased` is emitted on every release, unconditionally.** One
  event row per build, and the alternative was tried and rejected: gating it on
  a live waiting flag went silent for the one subscriber it exists to wake. A
  build that hit `--max-wait` has already exited, so its flag is gone, and it
  is precisely that build which was told to subscribe. `BuildSlotWaiting` and
  `BuildSlotAcquired` stay contention-only, since nobody sleeps on them.
- An emit lands in the emitting subprocess's own workspace, so a holder in one
  workspace cannot wake a waiter in another. This is the gap `e2e_lock.sh`
  documents. Here it degrades only the optional subscribe path, because the
  blocking poll reads filesystem state and is workspace-agnostic.
- The engine's rebuild now takes both locks, in order: it wins the checkout
  build lock, then waits for a slot inside the child it spawns. A loser returns
  `SkippedLocked` before spawning anything, so it never blocks on a slot it
  will not use.
- Frontend commands are deliberately outside. `tsc`, Vitest and `vite build`
  are tens of seconds and a fraction of a rustc tree's RSS.

## Alternatives considered

**The engine holds the count over HTTP.** Truly engine-owned, and the Thread
Queue panel could show it natively. Rejected because a slot outlives its
engine: an Apply restart, a crash or a killed session leaks it. Recovering that
needs the lease machinery this design exists to avoid.

**A build is Thread Queue work the engine runs itself.** Would unify capacity
under the existing *capacity policy*. Rejected on three counts. The agent needs
the build's output inline in its own turn. A human in a plain checkout has no
engine at all. And the engine would be running builds in worktrees it does not
own.

That option has a fourth problem worth naming. The capacity policy is
event-sourced per workspace, so it cannot express a machine-wide number at all.

**Per-checkout scope, keyed on `git rev-parse --git-common-dir`.** Follows the
engine build lock's existing scoping and resolves correctly from a worktree.
Rejected: N per checkout means two checkouts run 2N builds, and an
external-repo agent session is outside every Lucidos checkout.

**A PreToolUse hook refusing an unwrapped heavy build.** Would be airtight for
Claude Code in every workspace. Rejected for now, on three costs. It needs a
classifier that recognises a heavy build in languages it does not know. False
positives block real work. And it covers neither Codex nor a human.

**Real FIFO via a ticket file.** Would make "queue" an honest word. Rejected
because a killed waiter leaves a ticket behind. That needs liveness checking
and reclaim, which is precisely the stale state that made the `mkdir` lock
unusable. The concept is named a *slot* rather than a queue, so the word
promises nothing we do not deliver.

**The slot partitions cargo jobs too**, exporting `CARGO_BUILD_JOBS = ncpu / N`.
Attacks peak RSS directly and is what the e2e release path already does by
hand. Deferred: the slot gates a build's start, and shaping the build's
environment is a larger promise to keep.

**Two hand-synced copies of the pool logic**, the way ADR 0014 forced on the
gateway's `build_id.rs`. Rejected in favour of the `lucidos-build-slot` crate,
since nothing here forbids sharing one.
