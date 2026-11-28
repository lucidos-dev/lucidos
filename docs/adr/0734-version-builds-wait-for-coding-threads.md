# 0734: Version builds wait for active coding threads

- **Status**: Accepted
- **Date**: 2026-11-18
- **Amends**: [0304](0304-apply-build-has-build-slot-priority.md), [0412](0412-apply-joins-the-running-engine-build.md), [0589](0589-github-first-landing.md)

## Context

A paired source gateway on a 16 GB host has one build slot.
A new-version engine build there holds it for 12 to 20 minutes, whether Apply, follow main or self-heal started it.
Coding threads on the same host then queue their own builds behind it, or thrash swap beside it.
Those threads run in the same workspace as the engine that starts the build.

## Decision

1. A workspace preference, `defer_version_builds`, takes `never` (default) or `while-coding`.
2. With `while-coding`, every automatic new-version build waits while the workspace has an active coding thread.
   Automatic means Apply, follow main, self-heal, boot catch-up and a joined follow-up build.
3. An **active coding thread** is a coding-agent thread that is ongoing or blocked:
   - a running turn that is not a keep-alive turn, or a turn the user's Switch paused for resume;
   - a live event wait on its own background task;
   - `waiting_for_user_answer`, which covers question and permission cards.
4. Requests made while a build waits coalesce into one, which builds HEAD when it starts. Nothing is dropped.
5. Explicit requests never wait: Build now, Build new version, Retry build, `POST /api/v1/engine/rebuild`.
6. A build that has started is never stopped, paused or restarted.
7. A waiting build reads `deferred` on the version surface, names up to three threads holding it, and offers Build now.
   The Apply result carries `version_build: "deferred"`. There is no time cap.
8. The existing 10-second maintenance tick starts it once no active coding thread remains.
9. Slot classes, capacity, the memory gate and the slot pool are unchanged.

## Amendments

- **ADR 0304** said the engine's Apply rebuild goes straight to the slot pool as a priority waiter.
  With the hold on, it may first wait for coding threads. Once it asks for a slot, it is still a priority waiter.
- **ADR 0412** said an Apply joins a running build, which builds once more if HEAD moved.
  With the hold on, that follow-up build waits like any other automatic start. A finished `Ready` build still offers Switch.
- **ADR 0589** said a followed `main` builds in the background. With the hold on, that build waits too.

## Rationale

**Opt-in, per workspace.** Only the owner knows whether a host should favour coding threads over fresh versions.
A one-slot heuristic would also catch every 16 GB laptop, and an env var is invisible and needs a restart.
The motivating host serves one workspace, so a workspace setting is a host setting there.

**Blocked threads count.** A thread parked on a card resumes and builds the moment someone answers.
Starting a 15-minute build in that gap is the collision this decision exists to prevent.

**No time cap, and no starvation in the dark.** Every blocked thread already lights the Blocked badge and notifies the owner.
The waiting build names those threads, so one tap opens them. Build now overrides at once.

**The gate decides only whether to spawn.** The memory gate (ADR 0351) and slot pool (ADR 0070) keep their jobs.
No queue file, ticket or lease is added. The held build lives in the in-memory build state and is derived again after a restart.

## Consequences

- On a busy host with the hold on, the Switch can wait for hours. The version surface says why.
- In `follow_main_build = on-switch` mode a restart turns a held Apply build into the pending toast with Build new version.
- Threads in another workspace on the same host are not counted.
- An unreadable preference or thread count admits the build, as before.

## Alternatives considered

- **Hold only the follow build.** An earlier draft. Rejected by the owner: an Apply build collides just the same.
- **Tie the hold to slot capacity on every host.** Rejected: it changes laptops and dev Macs that never asked for it.
- **A machine-wide flock presence pool.** Rejected: the contending threads are in the same workspace, so a query suffices.
- **Count only running turns.** Rejected by the owner: a blocked thread resumes into the build it would collide with.
- **A time cap after N hours.** Rejected by the owner: it starts a long build at an arbitrary moment, unseen.
- **Pause or kill a running build.** Rejected for the reasons ADRs 0304, 0682 and 0683 give: it throws away paid work.
- **`follow_main_build = on-switch`.** Already exists, but it stops follow builds always, and never holds Apply builds.
