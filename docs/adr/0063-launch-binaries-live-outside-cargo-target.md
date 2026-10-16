# 0063: Published launch binaries live outside cargo's target dir, so a cargo clean cannot disable the workspace

- **Status**: Accepted
- **Date**: 2026-08-13
- **Amends**: [ADR 0022](0022-launch-binaries-are-published-per-build-variant.md) (the
  location only; its per-variant decision stands unchanged)

## Context

ADR 0022 established that a completed build **publishes** `lucidos-engine`,
`lucidos-gateway` and the `lucidos` CLI into a directory written only by builds of the same
profile and feature variant, and that every launch path uses the published copy. It put that
directory at `target/<profile>/launch/<variant>/`.

That location makes the whole running system a child of `cargo clean`.

The launch dir is not just where a workspace finds its engine. It is where the **`lucidos`
CLI** lives, and the CLI is load-bearing at runtime rather than only at launch:
`find_lucidos_cli_dir` (`crates/lucidos-engine/src/runtime/lucidos_cli.rs`) walks up from the
engine's own exe looking for a `lucidos` sibling, and the engine prepends that directory to
`PATH` for **every spawned coding-agent session and trigger subprocess**. It is also what
`run_coding_agent` requires in order to start a Claude Code session at all, because the
permission-prompt MCP server is served by the CLI.

So `cargo clean` did not merely delete build output. It silently removed the interpreter
every automation in the workspace depends on, from under a still-running engine.

**This is not hypothetical: it took the dev workspace down for eight hours on 2026-08-13.**
The Nightly Build → Harden → E2E orchestrator ran `cargo clean` inline in the trigger thread
at 00:13 instead of inside a Step 1 child session. The consequences chained:

- Its own `run_coding_agent` calls at 00:15 and 00:16 failed with *"the bundled `lucidos` CLI
  ... was not found next to the engine binary nor on PATH"*. **The clean destroyed the tool
  the orchestrator needed to delegate the rebuild**, so it could not recover by spawning the
  child that would have rebuilt.
- The pre-flight memory gate then correctly refused the run at 00:55 (available RAM 5.19 GB,
  below the 8 GB floor) and stopped, per its own knowhow. Nothing rebuilt.
- For the next eight hours every trigger shelling out to the CLI died with
  `FileNotFoundError: [Errno 2] No such file or directory: 'lucidos'`: 41 failure
  notifications across the notarization refresh, the hourly stats snapshot, both notify
  triggers, and the memory-leak watchdog.

The same `cargo clean` also deletes `<repo_root>/target/.lucidos-engine-build.lock`, the
checkout-shared advisory lock that serializes engine rebuilds across co-located workspaces.

## Decision

**The published launch directory moves out of `target/`, to the checkout-local
`.launch/<profile>/<variant>/`.** The engine-build lock moves with it, to
`.launch/.lucidos-engine-build.lock`.

ADR 0022's actual requirement of this path is that it stay **inside the checkout**, not that
it stay inside `target/`. `target/` was the incidental way of satisfying that. A checkout-local
dot-dir satisfies it too, and no cargo subcommand touches it.

Everything else in ADR 0022 is unchanged: one directory per (profile, variant), atomic
publish via temp file plus `mv -f`, a failed publish leaving the previous binary untouched,
the no-build fallback to cargo's uplift path with a warning, and build-id verification
against HEAD.

## Rationale

**The three blockers ADR 0022 raised are properties of a WORKSPACE-local directory, not of
being outside `target/`.** Re-read against the code, each survives the move:

| ADR 0022 blocker | Holds for `<repo>/.launch/`? | Why |
|---|---|---|
| The engine resolves its checkout by walking `current_exe()`'s ancestors for `scripts/web-dev.sh` | **Preserved** | `paths::repo_root_above` is a pure ancestor walk, already documented as depth-independent. `<repo>` is still an ancestor, so `paths::script("web-dev.sh")`, `run_engine_build`, `engine_build_lock_path` and `engine_source_matches_head` all keep their repo. |
| ADR 0021's worktree refusal is a pure path test on `LUCIDOS_ENGINE_BIN` | **Preserved** | `path_is_in_cc_worktree` matches `*/.lucidos/worktrees/*`. A worktree's own `.launch/` sits at `<worktree>/.launch/...`, which still contains that segment. Nothing is laundered. |
| `<workspace>/.lucidos/bin` is already taken by `ensure_workspace_bin_symlink` | **Not applicable** | That is the workspace directory. This is the checkout. |

Both preserved properties are now asserted rather than assumed:
`build_lock_lives_outside_cargos_target_dir` and `test_launch_dir_is_outside_cargo_target`
pin the negative constraint, `repo_root_above_is_independent_of_binary_depth` and
`test_worktree_refusal_still_sees_a_worktree_launch_dir` pin the two positive ones.

**Why the build lock has to move too, and is not merely tidy.** `flock` binds to an inode,
not to a path. Deleting the lock file while a build holds it releases nothing: the next
builder creates a fresh file, gets a fresh inode, and takes an uncontended lock. Two cargo
builds then run against the shared `target/` simultaneously, which is the exact collision the
lock exists to prevent, and it would happen **precisely during a clean build**, when the
compile is heaviest and the host least able to absorb a second one.

**Why not simply forbid `cargo clean`.** A rule that must be obeyed by every agent, script
and human in every session, forever, to avoid disabling the machine is not a safeguard. The
2026-08-13 run had knowhow telling it to spawn Step 1 as a child, and ran the clean anyway.
The knowhow guardrail is worth having and was added in the same change, but as the second
line of defence, not the first. A `cargo clean` is a legitimate, routine command; it should
be survivable.

**Why this reads as a fix rather than a workaround.** The launch dir was never a cargo
output. Cargo does not write it, cargo does not know about it, and nothing about it is
derived from cargo's layout. Its presence under `target/` was an accident of where the source
binaries happened to sit, and that accident is what coupled the running system's lifetime to
a build-cache command.

## Consequences

- **Landing this needs one manual `./scripts/web-dev.sh -w <ws> -b` per checkout, and the
  in-app Switch cannot deliver it.** This is the one real cost, and it does not repeat.
  A running engine reads the on-disk build id from `current_exe()`, which for every engine
  started before this change is `target/<profile>/launch/<variant>/lucidos-engine`. After
  this change **nothing writes that path again**: cargo uplifts to `target/<profile>/`, and
  publishing goes to `.launch/`. So the running engine's `disk_build_id` is frozen,
  `update_available` stays false, and *Switch to new version* never appears. Self-heal makes
  it worse rather than better: `source_behind_head` is true, so it triggers a rebuild, the
  rebuild succeeds without advancing the binary it is watching, and `self_heal_is_wedged`
  gives up for that HEAD, parking the workspace on a "New engine version pending, Rebuild"
  toast whose button cannot resolve it. That is precisely the dead end
  `docs/plans/2026-07-03-engine-version-switch-selfheal.md` exists to prevent, so it is worth
  being explicit that this transition reintroduces it exactly once.
  ADR 0022's transition did NOT have this problem, and the difference is instructive: the
  path it orphaned was cargo's own uplift path, which cargo keeps rewriting, so the pinned
  binary stayed live. This one orphans a path nothing will ever write again.
  **The window is now legible rather than merely predicted.** The engine reports the
  give-up as `rebuild_wedged` on `version-status` instead of only logging it, so the toast
  drops the *Rebuild* button that cannot resolve it and names the relaunch, the brand mark
  carries a dot for the whole window, and the toast can be dismissed and re-opened from
  that dot. See *pending engine version* / *wedged rebuild* in `docs/glossary.md`, and
  `docs/plans/2026-08-13-pending-engine-version-is-a-first-class-surface.md`.
- **During that window co-located workspaces do not share a build lock.** The lock path is
  resolved by the *engine binary*, so a pre-change engine flocks
  `target/.lucidos-engine-build.lock` while a post-change peer flocks
  `.launch/.lucidos-engine-build.lock`. Different inodes, so neither sees the other and both
  can run `cargo build` against the shared `target/` at once, the OOM collision the lock
  exists to prevent. Bounded and one-shot, but it does not close on its own for the reason
  above: it closes when every co-located workspace has been through a manual `-b`.
- **`cargo clean` no longer reclaims the launch binaries** (roughly 250 MB for a debug engine
  per variant). That is the entire point. To reclaim them deliberately: `rm -rf .launch`, or
  `make clean-all`, which now includes it. `make clean` deliberately does NOT, and says so.
  The next `-b` republishes.
- **The old `target/<profile>/launch/` tree is left behind as dead weight** until the next
  `cargo clean` collects it. Nothing reads it once a `-b` has published to `.launch/`.
- **`.launch/` is gitignored** and never tracked.
- **Stale-after-clean is correct, not a bug.** `published_build_state` classifies against
  HEAD, not against `target/`, so a clean followed by no rebuild leaves the launch binaries
  reported as `current` while cargo's cache is empty. That is the desired behavior: the CLI
  keeps working, and a genuine source advance still marks them stale.
- **A machine with the old path baked into a shell profile keeps a dangling `PATH` entry**
  until the operator updates it. Nothing in Lucidos depends on that entry: the engine derives
  the CLI directory from its own exe path and prepends it for spawned subprocesses.
- **`<workspace>/.lucidos/bin/lucidos` retargets on its own.** `ensure_workspace_bin_symlink`
  replaces a stale symlink on each engine boot.
- **Packaging is untouched.** `build-headless.sh` and `build-dmg.sh` read
  `target/release/lucidos-engine` (cargo's uplift path) directly and never consult the launch
  dir.

## Alternatives considered

**Leave the launch dir under `target/` and forbid inline `cargo clean` in knowhow only.**
Rejected as the sole fix: it makes the workspace's availability depend on every future agent
and human remembering a rule, and the run that caused this outage already had knowhow it did
not follow. Adopted as the *second* half of the change, not as a substitute for the first.

**Have `cargo clean` be wrapped by a script that re-publishes afterwards.** Rejected. It only
covers the wrapper, and `cargo clean` is typed directly far more often than through a
wrapper. It also inverts the dependency: the launch dir would be repaired after damage rather
than never damaged.

**Stage the binaries per workspace at `<workspace>/.lucidos/bin/`.** Still rejected, for
exactly the three reasons ADR 0022 gave. This ADR does not reopen that: the move is to a
CHECKOUT-local directory, which is the category the ADR's reasoning permits.

**Give each variant its own `CARGO_TARGET_DIR`.** Rejected in ADR 0022 (no shared dependency
cache, several GB and a full cold compile per variant) and rejected again here for an
additional reason: a per-variant target dir is still a target dir, so `cargo clean` in it
would take the binaries with it just the same.

**Keep the build lock under `target/` and accept the inode hazard.** Rejected. The window is
narrow but it opens exactly when two concurrent cargo builds are least survivable, and the
fix is a one-line path change beside a move that was happening anyway.

## References

- [ADR 0022](0022-launch-binaries-are-published-per-build-variant.md) (amended: location only)
- [ADR 0021](0021-long-lived-stack-never-runs-from-a-worktree.md) (the worktree path test this preserves)
- `docs/plans/2026-08-13-launch-binaries-survive-cargo-clean.md` (the implementation plan)
- `.claude/rules/dev-runtime.md` (the day-to-day rule)
