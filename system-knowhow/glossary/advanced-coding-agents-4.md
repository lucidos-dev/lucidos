---
name: Lucidos Glossary, Advanced (coding agents) (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Advanced (coding agents) (continued)

### App coding-agent thread
A *coding-agent thread* whose isolated *worktree* sparse-checks out the workspace git on one `data/apps/<id>/` folder. Same machinery as a Lucidos-internal one (worktree, branch, *change*, *Apply* ff-merge), but on the workspace git, not the Lucidos source repo. *Apply* restarts no engine, and Lucidos's `/harden` does not run (apps own their hardening). If *Apply* changed an iframe-bundled file, the engine emits a transient `AppUiRefreshRequested { app_id }` so open iframes reload. The *WIP app preview* shows the in-flight app from the worktree while the thread is open.

Branch name shape: `lucidos-<coding-agent>-app-<app_id>-<slug>-<id>`, e.g. `lucidos-claude-code-app-habit-tracker-add-streaks-401a2d19` (ADR 0041, ADR 0076).
See also: `docs/plans/2026-05-27-app-coding-agent-threads-design.md`.

### WIP app preview
The in-progress rendering of an *app* served from an open *app coding-agent thread*'s *worktree* instead of the workspace's main copy. Add `?thread_id=<id>` to the app UI URL. The panel-overlay slot then swaps from the live app (`<workspace>/data/apps/<id>/`) to the WIP (`<worktree>/data/apps/<id>/`). The WIP iframe loads HTML/CSS/JS from the worktree, but its SDK calls (`lucidos.data.*`, `lucidos.events.*`) still hit the live workspace. So data-coupled UI edits show their full effect only after *Apply*. It reverts to live when the user leaves the thread or *Apply* removes the worktree.

### Hardening
The quality gate every *coding-agent thread* must run via `/harden` before handing back to the user. It reviews the diff against project rules, runs the relevant test suites (Rust + TS + e2e, skipping irrelevant layers), and checks system-knowhow drift. A turn end whose work never ran it proposes nothing: the work waits as *unproposed work* with reason `hardening_missing`, and the *Not ready* strip offers **Harden**. A branch that carries only implementation plans skips hardening, since plans have no tests: it runs on the branch that implements the plan. If the marker is still missing when the user clicks *Apply* on an older change, Apply runs `/harden` synchronously and the user waits.

An *integrated sub-thread* runs the **fast tier**: the review, the lint and the frontend checks, with no engine suite, since its integrating parent runs every suite once over the merged work. A fast-tier marker counts only for an integrated sub-thread. Any other branch still needs the full run before Lucidos proposes it or Apply takes it.
See also: `.claude/commands/harden.md`, the playbook the agent runs. The engine system prompt owns the requirement to run it and states it to every session.

### Hold
Putting a thread and every unarchived *sub-thread* below it **on hold**: Lucidos starts no turn there by itself until you *resume* it. It covers chat and *coding-agent threads*.

Nothing wakes a held thread: no sub-thread report, event-wait delivery, `/harden` or plan nudge, context handoff or automatic re-entry. Hold stops every running turn in the tree, and sets each pending change aside until Resume. It raises no card.

Your own message still runs a turn, and the thread stays on hold. You can hold a thread from its menu. An agent may hold its own thread or one below it, on your words and with a reason. Home may hold any thread in its workspace.

Run `lucidos threads hold --reason "<why>"` when told to checkpoint and pause. Distinct from a *Stop*, which ends one turn and lifts with your next message. Distinct from a *thread pause* or an *agent pause*. See ADR 0805.

### Thread on hold
A thread a *hold* reached. Its strip says who put it on hold, when and why, and lists what waits. The thread the hold was placed on is its **root**, and only the root offers **Resume**. A sub-thread the hold reached links to its root instead. Its liveness reads `held`, with `hold: { by: "hold", reason, root_thread_id }`.

### On hold section
The *thread drawer* section between Pinned and Current that lists every *thread on hold*. A held root lists there with the sub-threads its hold reached, even while one still runs. A pinned root stays in Pinned, since a pin beats a hold. The section count counts held roots. A thread on hold is in no *Ongoing grouping* tile: not *Blocked*, *Review* or *In flight*.

### Resume
Ending a *hold*. **Resume** on the root's strip lifts the hold on the root and the sub-threads it reached. It brings back the changes it set aside. A sub-thread already stopped or held before stays so. What came due while held is worked out again, never replayed.

Only you resume. The thread's own agent may run `lucidos threads resume` only inside a turn your own message started there. A parent, Home and a trigger cannot. Archiving the thread yourself also lifts its hold.

### Repository
A user-registered external git repository (a row in the `repositories` table) that an *external-repo coding-agent thread* can run against. Distinct from `data/imported/` *imported* repos, flattened to plain files as *artifacts*.

### Plan kind

The purpose of a plan, recorded explicitly as `design` or `implementation`.
Approval of a design plan agrees its decisions and never permits implementation.
Approval of an implementation plan can authorize implementation or just landing the plan for later.
An implementation approval covers one version of a plan within its repository and thread tree.
The version ignores the status flip that records approval.
A filename never determines the kind.

### Design plan

A plan that records agreed design decisions, without implementation phases.
Its approval always covers the plan alone. An implementation plan and its own approval precede building it.

## Core terms (continued)

### Workspace snapshot

An engine-owned local copy of one workspace's database and included files.
It stays on the same machine, outside the workspace, and contains no secret
keys. Restore needs the existing key store. See [workspace snapshots](../backups/workspace-snapshots.md).

### Snapshot handle

The opaque identifier of a workspace snapshot. It resolves only within its
workspace; it is not a filesystem path.

### Recovery snapshot

A verified workspace snapshot captured before a snapshot restore changes
anything. It preserves the state being replaced, including the discarded timeline.

### Snapshot restore

Replacement of the same workspace's database and included files from a workspace
snapshot, coordinated by the gateway while its engine is stopped. It does not
undo external side effects or resume archived work automatically.

### Restore journal

The durable record of a snapshot restore's progress, stored outside the
workspace database and files. It lets the gateway finish recovery before
starting the engine after an interruption.

### Poster

A still first frame shown while a video is idle.
The built-in video player uses a poster so it holds no loaded video before playback intent.
`lucidos.media.poster` captures one through the host and releases the decoder afterwards.
