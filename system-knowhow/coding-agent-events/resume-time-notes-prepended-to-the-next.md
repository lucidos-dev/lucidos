---
name: Coding Agent Lifecycle Events, Resume-time notes prepended to the next prompt
description: Part of Coding Agent Lifecycle Events. Its index lists the sections it covers.
---

One part of `system-knowhow/coding-agent-events`, which lists every part.

## Resume-time notes prepended to the next prompt

`--resume` replays the prior conversation, but NOT what changed on disk or in `main` since. So on resume the engine prepends up to three short `[Note from engine: …]` blocks to the user's next message. They are NOT persisted events. They ride only on the in-memory prompt text, built in `build_resume_prompt_text` for both Claude Code and Codex. On a resume they appear only when the user's message is non-empty:

- **Branch adoption**: the worktree was switched to a new branch holding the agent's prior work; the engine adopts it and says so.
- **Side branch**, in place of adoption: a Lucidos-source thread's worktree is on a branch the agent cut from its own. The engine keeps tracking the thread's branch, which is what Apply lands, and tells the agent to switch back before it finishes.
- **Turn gap**: what the user or the engine did to the agent's work since its previous prompt. See below.
- **External edits**: the worktree changed between turns, detected by diffing against the `worktree_head_sha` on the last `CodingAgentIdled`. The wording names no cause. The detector sees only a SHA and a `git status`, so it cannot tell a hand edit from an engine reset. When the turn-gap note already states why HEAD moved, one line is suppressed: the `HEAD moved (no log available)` fallback, the signature of a backwards reset. A real commit log and any uncommitted change are still reported.

**A kept-alive session's follow-ups carry the turn-gap note too.** A session the engine keeps alive for a queued follow-up takes it without a resume. So the run loop prepends the turn-gap note to every prompt it forwards (`build_followup_prompt_text`), image-only ones included. The other two notes stay resume-only.

**A slash command goes verbatim.** Claude Code runs a command only when the input starts with `/`, so anything in front turns it into a model prompt. When the user's text starts with `/`, the engine forwards it exactly as typed, on a resume and on a live follow-up: no note and no device block. Its read carries `verbatim: true`, so the next prompt carries the whole turn gap, from before the command and after it. The branch-adoption, side-branch and external-edit notes are not carried over: a command sent as the first message of a respawn skips them.

**A non-empty block closes by disclaiming the topic.** The notes quote commit subjects, branch names and file paths. The next message often leans on a pronoun, and those nouns can capture it. So the block's last line calls those lines engine status, not the subject. A turn with no note gains nothing.

### The turn-gap note

The **turn gap** is the window between the agent's previous turn boundary and the current one. The resumed agent cannot see events in it, because `--resume` replays only its own conversation. Left unsaid, the agent might offer to Apply a discarded change or vanished commits, or treat reverted work as still in `main`.

| Event in the gap | What the note tells the agent |
|---|---|
| `ChangeApplied` | Merged into `main` and the worktree reset to match. Lists the merged commit subjects with the short post-merge `main` SHA. Not pending anymore. |
| `ChangeDiscarded` | Discard reset the change's branch to `main` and cleaned the worktree, so those commits are gone. Names the branch, and says whether it is the session's own branch or a stale change on a different one (the reconcile path discards siblings on other branches, and that must not read as "your work is gone"). Not pending: do not offer to Apply it. |
| `ChangeReverted` | The change had applied and has since been undone in `main` by revert commits, so `main` no longer contains that work. The branch and worktree were not touched, because a revert runs in the main repo. |
| `ChangeApplyFailed` | An Apply attempt did not land and the change is still pending. The engine's error is quoted and attributed as the message shown to the user, never rendered as an instruction to the agent. |
| `WorktreeCleaned` | Tier 1 stripped build artifacts, so the next build starts cold. Tiers 0 and 2 removed the whole worktree, including ignored files. Archived cleanup first commits uncommitted work on the thread branch. The branch stays available for resumption. Older events can report that cleanup deleted it. |
| `ChildReportFiled` | One of the agent's children finished cleanly, and its report was filed without waking the agent: its *wake mode* said so, or, for a child with no wake mode, its *wake decision* said no. The note carries the whole `[CHILD THREAD REPORT FILED]` block, once. |
| `ChildThreadDetached` | The user moved a child to top level. The parent stops waiting for its report; the child continues independently. |
| `ChildThreadStopped` | A user Stop paused one of the agent's child threads. The child is alive and waits for the user; a completion card follows when it next finishes, or a canceled one if the user archives or discards it. It says not to roll back the child's work, respawn it, or follow it up meanwhile. |
| `BackgroundBashCompleted` | A background task the agent spawned ended and no wake reached it. With `abandoned: true` the engine stopped mid-run, so there is no verdict: it says to drain the task rather than report it as passed or failed. Otherwise the task finished normally and the line carries its status. |
| `ResponseAborted` with cause `engine_shutdown` or `recovery_after_restart` | An engine restart stopped the agent's last turn mid-turn. The agent's transcript may show a tool call as rejected ("The user doesn't want to proceed … STOP … wait for the user") or a `[Request interrupted by user …]` line. The line says the shutdown wrote that and the user refused nothing. It says a running tool call did not complete and is safe to run again, after checking for partial side effects. |

**The restart row reads only the newest coding-agent terminal in the gap.** Claude Code writes the rejection text itself at teardown, and the engine cannot change it. So the resume names the real cause, or the agent reads a restart as the user saying no. The row stays out once a later coding-agent terminal follows the abort, since a resumed turn already ran past the restart. Other abort causes, user cancels and chat-channel aborts never produce it. It rides every resume path: the automatic resume after a switch, a Continue click, and the user's next message after a crash.

**The `BackgroundBashCompleted` row skips what an event wait delivered.** A coding agent's `lucidos background-task run` arms an *event wait* on the completion. A *polled background task* gets one at a clean turn end, if it still runs or finished unread. The re-opened turn carries the delivery as its prompt. So the note drops a completion an `EventWaitDelivered` names. Only an undelivered one reaches the note, such as a task whose wait a subscription cap refused.

Three more are left out. A task the agent stopped itself: the stop stood its watch down, so the killed completion is not news (ADR 0369). A completion that landed before the previous turn's real idle: the agent was still in its turn and could read it there. A polled one it left unread is not lost: the clean turn end armed a wait that delivers it. And a *cache keep-alive* turn, which is no boundary for the note and whose idle ends no turn. See § `BackgroundBashCompleted` in `system-knowhow/thread-events.md`.

Deliberately NOT in the note, because another mechanism already delivers them:

- `ChildThreadCompleted`. It is itself a coding-agent turn origin, and wakes the parent carrying the child's summary.
- `CodingAgentPermissionResolved`. Delivered in-band to the waiting call.
- `UserQuestionAnswered`. Delivered in-band while the subprocess is alive. When it is not, it arrives as the `answered_after_idle` resume message's own BODY, see "Answering a question the subprocess is no longer waiting for" above. That body carries the same `[Note from engine: …]` marker but is not one of the three reconciliation notes here, and `agent_recovery::continue_input_for_reason` assembles it rather than `build_resume_prompt_text`.

Deliberately NOT in the note, because one would add nothing:

- `MergeConflictDetected` and the merge-resolution events. The conflict session spawns with a purpose-built system prompt naming the conflicted files.
- `ChangeHardened`. The agent's own `/harden` caused it.
- `ChangeSummarized`. It names the agent's own commits back to it.
- `ChangeSetAside` / `ChangeBroughtBack`. The change and its branch are untouched, and the agent's next proposal brings a set-aside change back by itself.
- `ChangeWithdrawn` / `ProposalWithheld`. Neither touches the branch, and the agent's next turn end decides the work again.
- `CodingAgentSettingsChanged`. The resumed process already runs with the new model.
- The cosmetic thread events.
- `ThreadArchived` / `ThreadDiscarded`. Terminal, and archive's pending-change discard already arrives as `ChangeDiscarded`.

The note is **stateless and self-clearing**. It surfaces every covered event whose `sequence` falls between the *previous* turn boundary and the *current* turn's triggering event. The engine persists the current turn's origin before resuming the agent, and its `sequence` bounds the window. Anything after the origin (a second message racing the same spawn, a click while the agent starts) belongs to the next turn's gap.

A follow-up to a live session ends its window at its own `CodingAgentPromptSent` instead, which the run loop writes just before it forwards the prompt. A boundary counts only once the agent read the input it carried: a `CodingAgentInputRead` names its id or its `input_event_id`. A `verbatim` read does not count, since a slash command carried no note. So a prompt the agent never read moves no bound, whether the send failed, the agent died first or the user withdrew it. The next prompt or the respawn carries its gap. A boundary older than the workspace's first read predates read reports and counts as before.

The boundary set is every event type that can originate a coding-agent turn (`MessageReceived`, `CodingAgentUserMessageSent`, `TriggerStarted`, `ChildThreadCompleted`), plus `CodingAgentPromptSent`. Each prompt's origin or delivery becomes the next prompt's boundary once the agent read it, so an event surfaces once. No new event, projection column or migration backs it: the `events` table is the cursor.

## Recipe-shaped guidance

**Every recipe below is a *trigger*, the right shape only when the reaction should outlive the conversation and reach the user as a notification.** To tell the user **in the thread they are typing in** ("let me know here when a coding agent edits code"), use the `await_event` tool. A trigger runs in its own thread and cannot report into an existing conversation. The `on` entries below transplant verbatim into an `await_event` call. Pick the mechanism by where the answer must land, then by how long it must last. See `system-knowhow/triggers/when-a-trigger-is-the-right-answer.md` § "When a trigger is the right answer".

For the trigger config field reference (cron format, the `on` subscription list, per-entry `condition` operators), see `system-knowhow/triggers.md`. A condition key is a field path: a dot reads one level down. Operators: `$eq` `$ne` `$lt` `$lte` `$gt` `$gte` `$in` `$nin` `$regex` (a bare value is `$eq`), plus `$or` in key position taking a list of conditions. See `system-knowhow/triggers/writing-cron-expressions.md` § "What a condition can say" for the full language.

### Notify when a coding agent is waiting on the user

```yaml
on:
  - event_type: UserQuestionAsked
run:
  intent: "Notify me when the coding agent has a question waiting for me. The push should deep-link straight to the question: tapping it takes me to the originating thread and pulses the question card on land."
```

The `tap` + `event_id` pair makes the push deep-link: the tap opens the originating thread *and* pulses the matching event card. Without them the push opens the inbox modal. See `triggers.md` for the full pattern.

To scope to a specific coding-agent session, add a per-entry `condition`:

```yaml
on:
  - event_type: UserQuestionAsked
    condition:
      cc_session_id: "abc123-…"
```

Conditions are pure payload filters: the event's own payload fields, plus `thread_id`, which the engine supplies for every thread event. `cc_session_id` is on `UserQuestionAsked`'s payload, so this works. Nothing else about the thread is reachable (no title, no app id, no status). A **domain event** has no `thread_id`, since it belongs to no thread.

### Notify when a coding-agent session finishes / produced changes

```yaml
on:
  - event_type: ChangeProposed
    condition:
      set_aside: { $ne: true }
run:
  intent: "Tell me the coding agent finished and left a change to review."
```

Only a finished turn proposes, and an external-repo session never does. Add `requires_restart: true` for changes that need a new engine version. For "the turn ended with work on the branch, proposed or not", use `CodingAgentIdled` with `has_changes: true`, and add `is_external_repo: { $ne: true }` to scope to the engine repo. The same condition language works on every payload field.

### Wait for one named coding-agent session to finish

```yaml
on:
  - event_type: CodingAgentIdled
    condition:
      thread_id: "<uuid>"
```

The same entry works as an `await_event` subscription. That is how a chat thread waits for a coding-agent session it did not spawn: list the running ones (`threads` list, `status: ["running"]`), then subscribe per session. Two caveats:

- `CodingAgentIdled` is a **turn boundary**, not a session terminator, so a session that gets a follow-up emits another one later.
- The first match spends a subscription. To wait on several sessions, re-list on each wake and subscribe again while any are still running.

### Notify when a coding agent errors

```yaml
on:
  - event_type: ResponseFailed
run:
  intent: "Send me a push notification with the failure error."
```

`ResponseFailed` fires for both chat and coding-agent failures. See "The error gap" above for what "error" means here, and for finer-grained alternatives.

## A human Stop holds automatic wakes

A device Stop makes a *stopped thread*, on chat and coding-agent threads alike.
It takes no engine nudge until a device message, Continue, card answer, Harden
or Apply resumes it. The hold folds from events and survives restart. An
unreadable state holds rather than starting work.

Event waits keep their subscriptions and unread delivery anchors. Child reports
are filed, and a human resume reads them with the held waits. Queued deliveries
retain their anchors, so the agent's read settles each once. Turn-end nudges
are recomputed at the next idle. A message from Home or a parent may still
start a turn; it does not lift the hold on engine wakes. See ADR 0696 and
*stopped thread* in `system-knowhow/glossary.md`.

A successor handoff made while the thread was stopped stays held after the
resume. A resume that runs no turn, such as an Apply, does not start it. The
owner's next message or Continue starts the successor session. A handoff that
carries a request is never held.
