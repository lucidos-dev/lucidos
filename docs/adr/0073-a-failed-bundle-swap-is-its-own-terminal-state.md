# 0073: A failed bundle swap is its own terminal state, not a retryable failure: the check fails closed and the launchd job is left running the deleted inode

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

`tauri-plugin-updater` moves the current `.app` into a `TempDir` and has no
restore branch if the final rename fails. The `TempDir` then deletes the backup
on the way out, so a failed swap can leave nothing at the bundle path. This is
written up for upstream in
[`docs/upstream-issues/tauri-plugin-updater-macos-no-rollback.md`](../upstream-issues/tauri-plugin-updater-macos-no-rollback.md).

Our blast radius is larger than a typical Tauri app's. The launchd job
`gui/<uid>/com.lucidos.engine` has `KeepAlive=true` and its `ProgramArguments`
point into the bundle. Kickstarting it onto a missing binary is a crash loop on
a 10-second `ThrottleInterval`. It takes the gateway, every workspace engine and
the embedded Postgres down with it.

The failure also arrives in two shapes. The destructive case reports `Err`,
because the rename is what failed. A rarer one reports `Ok` over a partial
unpack that left no runnable app.

## Decision

After every install attempt, look at the bundle on disk before touching the
launchd job. When no runnable app is there, raise the dedicated
`bundle-swap-failed` phase, and leave the launchd job exactly as it is.

## Rationale

**Its own terminal phase, not a longer `failed` string.** The two need different
handling, not different wording. `failed` is retryable and this is not, the
recovery is a reinstall from the `.dmg`, and the page must not re-offer the
update. Telling a user whose app is gone to try again is the one thing that
cannot work.

**Both install outcomes ask the same question.** The destructive case reaches us
as an `Err`, so checking only the `Ok` path would miss it entirely. The opening
of the message still differs: saying "the update reported success" to someone
whose installer reported failure would be a lie.

**Fail closed on a path we cannot resolve.** Not knowing where the bundle is is
exactly as informative as finding nothing there, and the recovery advice is the
same either way. The case is close to unreachable, so the strict direction costs
near zero and the lenient one costs the crash loop.

**Leave the launchd job alone.** The service running right now still holds the
deleted inode, so it keeps serving the user's workspaces. Reinstalling from the
`.dmg` restores the exact path the job already points at, which is what makes
the recovery a drag-and-drop.

## Consequences

- The bundle path is resolved the way the plugin resolves it, through its own
  `extract_path_from_executable` over `current_exe()`. A hardcoded
  `/Applications/Lucidos.app` would report a false failure for anyone running
  from `~/Applications`.
- The executable name inside the bundle comes from `CARGO_PKG_NAME` rather than
  a literal. A drifted literal would not fail loudly: the check would look for
  a path that cannot exist and block every update over a rename.
- Any execute bit at all is the floor. The question is whether the swap landed
  a real executable, not whether the permissions are ideal.
- The TypeScript side owes `bundle-swap-failed` its own arm. It is a
  discriminated union on `phase`, so a missing arm is a `tsc` error.

## Alternatives considered

- **Report it as an ordinary `failed`.** Reuses a phase the page already
  handles, and tells the user to retry an update that has already destroyed
  their app.
- **Boot the launchd job out on detection.** `stop_service` would kill the
  running service and remove the agent. With no app on disk, nothing could
  bring either back, so the user loses their workspaces on top of their app.
- **Kickstart the job anyway and let launchd report it.** That is the crash
  loop, discovered at the next boot instead of immediately, with the whole
  stack down in between.
- **Roll back ourselves by keeping a copy of the old bundle.** Doubles the disk
  cost of every update and duplicates work that belongs upstream. The
  fail-closed check gets the same recovery for the price of one `stat`.
- **Check only that the bundle directory exists.** A partial unpack leaves a
  directory tree that is not an app, and this would call that a success.
