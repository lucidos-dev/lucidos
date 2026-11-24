---
name: Lucidos Glossary, Advanced (coding agents) (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Advanced (coding agents) (continued)

### Active coding thread
A *coding-agent thread* in this workspace that is ongoing or blocked, including Claude Code and Codex. It counts while running a turn, paused for resume after **Switch**, waiting on its own background task, or awaiting a question or permission answer. Keep-alive-only turns do not count. Idle, finished, failed, canceled and archived threads do not count unless they still have a live event wait.
See also: *build deferral*.

### Landing nudge
The call a gateway makes to each *remote gateway* after it pushes a new `main`, a queue entry, or a landing result to `origin`. Each one then *follows main* at once, rather than at its next interval. A lost nudge costs one interval, and an older gateway that lacks the call is skipped quietly.
See also: *follow main*, *gateway pairing*.

### Lane batch
The changes a *landing lane* pass could not merge cleanly onto `main`, resolving together in one *lane thread*. At most one resolves at a time. The batch lands whole, one merge commit per change, or fails whole. A failed batch leaves every member pending and *apply-ready*, and nothing queues it again until Apply is pressed. Announced by `LaneBatchStarted`, and ended by `LaneBatchLanded` or `LaneBatchFailed`. Cancel one with `POST /api/v1/changes/lane/cancel`.

An *Apply All* press's conflicts form one batch, since the lane waits until the press has handed over every change.

### Arrival hold
While a *lane batch* hardens again after `main` moves, the *landing lane* holds new clean changes. The hold covers changes handed off to the *lander*. They wait, in the order they arrived, and land as soon as the batch lands or fails. A cap of 15 minutes ends the hold early, so a stuck batch never blocks everyone. The lane snapshot names the holding batch as `arrival_hold`. Announced by `LaneArrivalsHeld`, and ended by `LaneArrivalsReleased`.

### Lane thread
The *coding-agent thread* the engine opens for a *lane batch*, titled for what it lands ("Landing 3 changes"). It merges each member as its own merge commit, resolves the conflicts, runs `/harden` once, and asks you when a resolution needs your judgment. You can follow its steps and write to it like any thread.

When its turn ends cleanly, the batch lands. A message you send while it works gets its own turn first. If `main` moved meanwhile and conflicts with its work, it gets one more turn to merge `main`.

**Stop** ends its turn and keeps everything, as on any thread: the batch stays open with its worktree, and your next message or **Continue** carries it on. The lane's **Cancel** fails the batch. So do a failed turn, a turn that cannot start, and discarding, archiving or deleting the thread, even while it sits idle. A crash leaves it a Continue, and no second batch starts meanwhile.

### Main check
A GitHub run on a stripped copy of `main` after changes landed (ADR 0464). It runs the engine, crates and script suites on Linux and macOS, and every e2e suite, and gates nothing: `main` never waits for it. One runs at a time. When it ends, the next covers everything that landed meanwhile. A red one schedules one *main repair*.

A *landing lane* landing starts one, and so does a `main` that *follow main* fast-forwarded to another gateway's landing (`MainFollowed`). It starts only under the *main-check setting* `here`, in a checkout with the release libraries and the `lucidos` remote. An engine restart picks up the GitHub run it left going, once that run is on record. A run that ends without a verdict (GitHub down, a refused push, a restart before the run started) starts nothing by itself: the next landing does.

A newer `main` cancels a check whose tests have not started, and the next check starts on it (`MainCheckSuperseded`). A check whose tests started runs to its verdict. Its events are `MainCheckStarted`, `MainCheckRunLinked`, `MainCheckPassed`, `MainCheckFailed`, `MainCheckAbandoned`, `MainCheckSuperseded`, `MainCheckRedClosed` and `MainCheckRepairStarted`.

The *landing lane*'s line in the Lucidos menu says where it stands: "Main check: passing", "Main check: running" linking the run, or "Main red: <what failed>" linking the fix thread, with **Close**. While it runs or main is red, the line shows even with the lane empty. `lucidos main-check status` reads the same.

### Main-check setting
Which workspace runs the *main check* (ADR 0694), in Settings → Coding Agents. `here`: this workspace checks every new `main` and repairs a red one. `another-host`: another workspace checks `main`, so nothing starts here. `off`: no main check covers this workspace.

Unset, it is `here` in a checkout that can run the main check, else `off`. Under `here` and `another-host`, `/harden` leaves the slow suites to the main check. Under `off`, it runs every suite before a change lands. Two workspaces set to `here` both check and both repair: set one of them to `another-host`. `lucidos main-check setting` prints it.

### Main repair
The single repair that owns open main-check failures and flaky tests (ADR 0682). Its fix-thread identity stays stable across continued failures.

An automatic repair starts only under the *main-check setting* `here`. It waits for two minutes without running coding-agent work, an Apply, queued work, or machine builds. Keep-alive turns and parked threads do not count. **Fix now** starts the repair immediately as owner work, under any setting. Automatic repairs use background build slots that yield to chosen work. Running repairs are never stopped.

A working repair arms an *event wait* on its red. Each later failing check, each flaky finding and the closing pass then reach it while it works.

The Lucidos menu shows whether the repair waits, what blocks it, or its running thread. Each new red creates one in-app notification, without push. A restart recovers a missing notice under the same identity. A later pass drops a waiting red repair. Archived or deleted repair threads stay gone; a newer failure or **Fix now** can replace them.

### Fix thread
The one *coding-agent thread* a *main repair* runs in, titled "Fix main: <what failed>". Its first prompt carries the failure and its tail, the run, the range, and the changes that landed in it. It is an ordinary thread: its change goes through the *landing lane* when you press Apply.

Checks continue while main is red. A newer failure updates the red under the same repair owner; a newer pass closes it. A fix landing also closes red. A check that predates that fix cannot reopen red.

You can close red with **Close** (`POST /api/v1/main-check/close`). Close never stops a running repair. An agent cannot close red.

Its first prompt lists each failed test with the head of its error. It runs on Claude Code, Sonnet 5.5, medium effort, whatever the workspace's own default is.

### Flaky fix thread
The *coding-agent thread* a *main check* opens when it passes but some tests failed once and passed a retry, titled "Fix flaky: <first test>". The engine suite reruns its failed tests once, and Playwright retries each browser test once. A test that fails twice still turns main red.

A pass with flaky tests holds no check. Its findings attach to the existing main repair, or schedule one if none exists. A later flaky pass retains that owner. It closes when a change from it lands, or when you archive or delete it. It runs on the same implementer as the *fix thread*.

### Apply estimate
How long a hardening and a merge conflict resolution usually take in this workspace, from its own past applies. Recent applies count most, since the process gets faster. With fewer than five past runs there is no estimate, and the Lucidos menu shows only the time so far. It is a typical time, never a countdown: the slowest runs take two to three times as long.

Each pending change also carries a conflict prediction: whether merging it into `main` now would conflict. If so, its Changes panel row says "Likely merge conflict", and the batch estimate counts a resolution.

The engine emits `ApplyAllBatchStarted` with the full change-id list and the actor. It advances the batch as each member's `ChangeApplied` / `ChangeApplyFailed` lands, and parks a member on its `MergeConflictDetected`. Once all members resolve, it emits `ApplyAllBatchCompleted` with `applied: Vec<Uuid>` and `failed: Vec<ApplyFailure>`. Member status is first-write-wins, so one failure does not abandon the batch. Each member follows the same *hardening* and restart-derivation rules as a single *Apply*. Persisted under aggregate `apply_all_batch`, `aggregate_id` = `batch_id` (UUID).

While the batch runs, its Lucidos menu line offers **Cancel** (`POST /api/v1/changes/apply-all/cancel`). The engine stops advancing and interrupts the in-flight *hardening* or merge session and every parked resolution. It marks the remaining members `failed` with "Apply All canceled", so `ApplyAllBatchCompleted` still fires. Applied members stay applied; the rest return to pending (best-effort for a merge that already landed). A single *Apply* that woke a *hardening* or merge session can be canceled from its *coding-agent thread*'s Cancel button.

Cancel reaches the *landing lane* too. A member waiting there leaves it, and a *lane batch* holding a member fails whole. A lane member's own thread is never interrupted: the lane holds its change, not the thread's agent.

**The sweep is the prompt's form.** Ask Lucidos to apply everything as it settles, and it applies what is ready now. It also adds a *standing apply* to every *settling* thread whose change Lucidos applies, even one with no change yet. An *external-repo coding-agent thread* is passed over, having no change to apply. The batch's **Cancel** in the Lucidos menu takes back the whole sweep. The Changes panel's **Apply all on settle** arms only the changes it lists (*standing apply*).

### Standing apply
The owner's instruction to *Apply* a *change* once its thread finishes: pressed while the thread settles, carried out by the engine later (ADR 0168 clause 5). It replaces the Apply button on an unsettled thread, so nothing on either surface renders disabled.

Two forms. **Apply on settle** arms one change, from the thread's prompt row or its Changes panel row. **Apply all on settle**, atop the Changes panel's **Not finished** section, arms each *settling* change listed there and touches nothing in **Ready**. Both follow the same rule and are one-shot. The *sweep* is the prompt's form (*Apply All*).

It goes wherever *Apply* goes, and nowhere else. An *external-repo coding-agent thread* never gets one: Lucidos does not merge into that repo, and the thread proposes no *change*. So its prompt row draws no flag, the sweep passes it over, and the engine refuses any other arm.

Nor does a change whose *Apply* hit merge conflicts. Its thread works only to resolve that merge, and the resolver lands the change itself. Its Changes panel row reads **Applying...** with "Resolving merge conflicts", and its prompt row draws no flag.

It always ends. The change applies once the thread finishes and the agent has saved its last edits. A thread on an *event wait* keeps the instruction, since the wait ends by itself. A thread parked on a question, with a failed turn, or whose session stopped mid-settle never settles by itself. There the instruction is dropped and reported. It acts only on the change it was armed for, never a later one.

Cancel it from the control that armed it; every control is a toggle showing one shared state. A **Changes panel** row reads **✓ Applying on settle** once armed. The bulk control atop Not finished reads **✓ Applying all on settle** once all its changes are armed, and a press cancels them. On the **thread's own prompt row** it is a flag icon, filled once armed, with a matching tooltip.

Cancelling stops what has not started. A change already merging or hardening finishes, and nothing applied is undone. A running *Apply All* has its own Cancel, on its Lucidos menu line.

### Settling thread
A *coding-agent thread* that has not finished but will by itself: running, paused, or on an *event wait*. A *standing apply* waits through these, and through the moment after a turn while the agent saves its last edits. A thread parked on a question is not settling, since only you can end that. While it runs or watches an event, its *Apply* is withheld and the standing apply takes its place.

### Cancel (Stop)
The user-clicked **Stop** action on a working *coding-agent thread*. Like **Esc** in the *Claude Code* CLI, it *interrupts* the current turn but keeps the session resumable. The same `cc_session_id` and branch are kept, so the next message continues the *same* conversation (a `--resume`) with full context. It is NOT a kill and NOT a fresh start.

It emits `ResponseCanceled` (the visible "Canceled" chip) and `CodingAgentIdled` (the resume anchor). *Apply*, Discard and Archive are distinct: each ends the turn with its own lifecycle event. A stopped turn proposes nothing. Its work stays on the branch as *unproposed work* with reason `turn_incomplete`, and the *Not ready* strip offers **Continue** (ADR 0400). Stopped work does not block Archive: archiving sets it aside.

It routes through `interrupt_agent` (`POST /api/v1/claude-code/stop`, default `StopReason::UserStop`). A bounded fallback hard-stops only if the agent fails to honor the interrupt. Source: `crates/lucidos-agents-api/src/engine/claude_code/control.rs`, `agent_session/lifecycle.rs` (`idle_change_write`, `SessionEndAction`).

### Change
A *coding-agent*-proposed set of file edits shown as a pending branch in the UI. It is resolved by *Apply* or Discard, or kept for later by putting its thread on *hold*. Apply is a non-disruptive merge into main; an engine-affecting change then surfaces *New version available / Switch to new version*. Lifecycle events: `ChangeProposed`, `ChangeApplied`, `ChangeDiscarded`, `ChangeSetAside`, `ChangeBroughtBack`, `ChangeWithdrawn`. Stored as a row in the `changes` table. Internal (Lucidos-repo) coding-agent threads produce changes; *external-repo coding-agent threads* skip this flow.

The Changes panel lists pending changes in three sections. **Ready** comes first: the *apply-ready* changes, with **Discard All** and **Apply All**. **Applying** comes second: changes the landing lane holds or an apply has claimed, which Apply All and Discard All leave alone. **Not finished** comes third: changes whose thread is still unsettled (*settling* or parked on a question), with **Apply all on settle**, and any older change that still needs hardening. Discard All never reaches Not finished.

Below them sits **Recently applied**. A row links to its thread, and *Hold* lives on the thread, never on the row.

A change's `status` is one of six values: `pending` (awaiting Apply or Discard), `set_aside` (a *set-aside change*), `applied`, `discarded`, `reverted` (applied, then undone), or `withdrawn` (a *withdrawn change*). No other value exists.

A user **Stop** proposes nothing: its work stays *unproposed work* on the branch (see *Change state*).

A change's file list tracks git. When later commits cancel the diff out (a commit plus its revert), the engine re-syncs the row to **zero files**. The card then reads "No file changes" rather than claiming edits its Diff can't show. Such a change stays pending, since the engine never resolves a change for the user. But *Apply* is refused, having nothing to merge; **Discard** resolves it. The re-sync runs when the coding agent next idles, when its session ends, and in an engine-startup sweep for stale rows.

### Set-aside change
A *change* kept for later, out of the way. A change is set aside only with its thread: **Hold** sets each pending change of the held thread and its sub-threads aside. A set-aside change leaves the Review list, the attention badge and *Apply All*. The branch stays. A *thread on hold* lists under **On hold**, and **Resume** returns exactly the changes its hold set aside to pending. A set-aside change also returns by itself when its thread's agent proposes new work on the same branch.

Lucidos keeps work this way for you in a few cases, such as a thread archived with *unproposed work* on its branch. That includes uncommitted files in its *worktree*: about an hour after the archive, Lucidos commits them on the branch and removes the worktree. Archived work sits on no list, and its row reads " · Incomplete" when nobody saw its turn finish. **Unarchive** offers it again: a new change if the last turn finished, otherwise unproposed work with its reason. When Lucidos keeps work on a live thread, it puts that thread on hold.

### Change state
What a *coding-agent thread*'s branch holds, as one of three values (ADR 0400):

| State | Means |
|---|---|
| `none` | No work on the branch. |
| `unproposed` | *Unproposed work*: commits on the branch that no pending *change* carries. |
| `proposed` | A pending *change*, which *Apply* takes. It also says whether applying it needs a restart. |

*Apply* acts only on `proposed`. Filter threads by it with `lucidos threads list --change-state`.

### Unproposed work
Work on a *coding-agent thread*'s branch that no pending *change* carries. Its reason says which turn end withheld it:

| Reason | Why |
|---|---|
| `plan_missing` | The branch has no implementation plan. |
| `plan_changed` | The plan changed after approval. Restoring the approved text reuses the approval. |
| `plan_awaiting_approval` | The plan is not approved yet. |
| `outside_bound` | A security fix reaches past the files it named. |
| `hardening_missing` | The work never ran `/harden`, so *Apply* would have to harden it first. A branch that carries only implementation plans is never held for this. |
| `turn_incomplete` | The turn did not finish: a Stop, a failure or an engine restart. |
| `awaiting_landing` | The thread's change waits in the *landing lane*. The newer work is proposed once that change lands or its batch fails. |
| `integrated_into_parent` | An *integrated sub-thread*'s hardened work. It is never proposed: its parent's agent merges the branch. If the parent's change lands without it, the work is set aside. |
| `carried_by_successor` | The thread continued in a legacy *successor thread*, whose branch starts at this thread's last commit. It is never proposed here: the successor's change carries it. A *successor session* never sets it, since it stays in the same thread. |

No reason means no turn end withheld it. The thread is still running, waits on an event, works in an external repo, or its work was set aside. A turn end that withholds work emits `ProposalWithheld`. Unfinished work is never a proposal: Continue resumes the turn, and a turn that finishes proposes it.

### Not ready
The amber strip above the prompt box on a resting *coding-agent thread* that holds *unproposed work* with a reason. It shows where *Apply* would be, and says why there is none: "Not ready: needs a plan", "Not ready: plan awaits approval", "Not ready: outside its fix bound", "Not ready: not hardened" or "Not ready: the turn did not finish". A short hint follows, and a tap shows the full reason. It carries **Continue** for an unfinished turn, and **Harden** for work that is not hardened. Harden starts the hardening run, and when that run finishes, Lucidos proposes the change, or an *integrated sub-thread* hands it to its parent. It stays hidden while the agent still holds a queued message it has not read, since that message starts the next turn by itself.

On an *integrated sub-thread* whose work is hardened, the strip reads "Integrated into" and the parent's title, with nothing to press: the parent's agent merges the branch, and the parent's change carries it to *Apply*.

A thread that continued in a *legacy successor thread* shows no strip,
since that legacy successor's change carries the work. Its closing notice links
the legacy successor thread.

