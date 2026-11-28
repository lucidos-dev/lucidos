---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos planned mark (--plan <path> | --simple "<reason>" | --security-fix "<reason>" --files <csv>)` / `lucidos planned approve [--plan-only]` / `lucidos planned state`

Record, approve, or query the *plan marker*. Before a *Lucidos-source* coding-agent branch is edited and applied, it proves the `implementation-plan` skill ran. It also proves the human approved its plan, or that a local fix was acknowledged. Without a **gate-satisfying** marker, Claude Code's first source edit is blocked (the `cc-plan-gate` PreToolUse hook). The engine's plan floor also holds the branch: turn end proposes no change, and Apply is refused. Apply consumes the marker, so work after an Apply needs a new one.

`mark --plan <path> --kind design|implementation` records the plan kind explicitly; omitted `--kind` means `implementation`.

On a calling thread, the marker and its lifecycle event commit atomically. The plan must be committed on that branch; `plan_sha` identifies its git blob. Re-marking reuses an implementation approval when the repository, thread tree, path and plan version match.
The plan version tolerates the status flip that `approve` writes.
The CLI reports whether approval carried forward or a new proposal awaits approval.
When the repository ships prose, em-dash or plan-section gates, the CLI runs them on the committed plan before recording a marker.
The plan-section gate refuses a plan without a `User Journey` and a `Visual Decisions` section.
If a gate fails, fix the reported issue, commit the plan and mark it again.

`approve` atomically records `PlanApproved` and its scope with the marker transition, refuses a changed proposed revision, and records one approval across concurrent calls or retries. Historical event replay never restores consumed marker permission.

A simple or bounded security fix records `PlanApproved` with a distinct fix kind when a coding-agent thread calls it. The marker and its evidence commit together. Chat callers and shells without a caller thread write the marker alone.

A full approval prints the next action. After a fresh-session pick, hand off. Otherwise start the first phase in the same turn.

A design approval always records `plan_only` and never permits source edits.
After the user picks `Commit plan for later`, run `lucidos planned approve --plan-only` and land only the plan.
That flag requires that pick or a plain `for later` approval. Plain `approve` refuses an approval for later.
Implementation approval and approval for later are separate; neither substitutes for the other.
A later implementer marks the plan again and asks for implementation approval.

Wraps `POST /api/v1/internal/mark-planned` / `POST /api/v1/internal/approve-plan` / `GET /api/v1/internal/planned-state`.

`mark` and `approve` accept the same callers as `hardened mark`, and refuse anyone else with 403.

```bash
# Complex work: the implementation-plan skill writes the plan, then records this for you.
# This records the AWAITING-APPROVAL `proposed` state. It does NOT unblock editing:
lucidos planned mark --plan docs/plans/2026-06-19-my-change.md

# Present the plan, then ask for approval with your question tool (see below).
# Once the user APPROVES, flip it to gate-satisfying:
lucidos planned approve

# Genuinely local fix that needs no plan: acknowledge instead (no approval needed):
lucidos planned mark --simple "rename a misspelled variable"

# UNATTENDED run only (the nightly security pass): a security fix confined to named
# files, with a regression test. No approval step, and Apply refuses the branch if it
# touched anything outside the list:
lucidos planned mark --security-fix "unscoped local proxy key; proxy_tests::refuses_foreign_host" \
  --files crates/lucidos-agents-api/src/api/proxy_builtin.rs,crates/lucidos-agents-api/src/api/proxy_tests.rs

# Inspect the current branch's marker:
lucidos planned state
```

`planned state` prints `SATISFIED`, `PROPOSED`, `MISSING`, `CHANGED`, or `UNREADABLE`.
`CHANGED` means the plan changed after approval. Restore and commit the approved text to reuse the approval.
If verification fails, check the branch and committed plan file before retrying.

`mark` / `approve` resolve repo_root / branch / HEAD from `$PWD`'s git worktree (like `lucidos hardened mark`). Pass exactly one of `--plan` / `--simple` / `--security-fix`.

**`mark --plan` reuses implementation approval for the same plan revision in
the same repository and thread tree.** With no matching approval, it records
`proposed` and awaits approval.

An integrated sub-thread reports a new plan's committed path and revision to
its parent. The parent brings that revision into its branch and obtains the
owner's implementation approval. The child reruns
`lucidos planned mark --plan <path>` to inherit it.

When the plan needs an owner decision on your thread, run the plan flow's checkpoint
`lucidos successor offer --at plan-approval --ask --progress 0/<phases>` for
its option shape. Summarize the plan on a question card. Ask with `AskUserQuestion` on Claude Code or
`ask_user_question` on Codex. The floor is `Approve`, `Commit plan for later`,
and a real fork if any. A typed reply requests changes; never allocate a
`Request changes` option.

Use the normal user question tool with a short set of relevant choices.
Follow-up questions may choose the backend first, then its rung. The examples
need not list every available rung. Choices sharing one question use slot
priority A, P, X, R, F, T. A approves here; P commits the plan for later;
X names the other backend. R offers a fresh session; F names a fork;
T names this thread's rung.

Display at most four. The recommended option comes first and carries
`(Recommended)`. A recommended R displays R, X, T, A, P, F. With no R, A comes
first.

At plan approval, a non-paying card recommends staying: it reads
`Approve here (Recommended)` and displays A, R, X, P, F. At plan approval,
always include R when handoff is available. Always include X when another
backend ladder is readable, merging it into R when R names that backend.
Other checkpoints require a user request or flow policy permitting X.
T requires a paying card and must differ from R.

Name a fork without a slot as a typed reply.
At repository checkpoints A reads `Continue here` and P is absent.

With an unreadable engine or no handoff available, offer no fresh-session pick.

Picking an `Implement in a fresh session` option approves the plan. The agent runs `lucidos planned approve`, then hands the build to a *successor session* in the same thread with `lucidos successor start` (below). The narrative names the plan path and says the user approved it. The thread keeps its branch, so the plan and the approved marker stay in place.

**A changed plan needs approval for its new revision.** If the new revision has
no matching approval, the edit gate closes and the plan flow returns to its
approval checkpoint.

For implementation, the agent runs `lucidos planned approve` only after the user approves implementation, flipping `proposed` to `planned` (gate-satisfying). The engine checks this and refuses with a 409 (`no-plan-approval`). It wants a tapped approving option, or a typed plain approval such as `approve` or `lgtm`, given after the plan was proposed. A branch with no owning thread, such as a human's own, approves unchecked.

**The marker belongs on the branch the thread applies.** In a Lucidos or app thread, Apply lands only the engine-named branch, even after a `git switch -c`. So `mark` and `approve` from a side branch refuse with a 409 (`marker-off-tracked-branch`) that names the branch to switch back to. Never cut a new branch for new work in such a thread. An external-repo thread is exempt, since it adopts a renamed branch.

A tapped fork is an approval too: revise the plan file to that variant, re-commit, then flip it. A fork pick does not re-mark. A typed reply that asks, suggests or questions is a request for changes. Revise the plan file, re-commit, run `mark --plan` again, and ask again (the marker stays `proposed`).

The card's question carries the plan summary, with the ask on its last line. A message that also calls a tool never reaches the user. A re-ask carries only what changed. `mark --simple` records `acknowledged_simple` directly, since local fixes need no approval.

`approve` also rewrites the plan file's `- **Status**: Proposed` line to `Approved` and commits it. A plan with no such line is left alone. A branch waits for approval while its plan is `proposed`, a branch of `docs/plans/` files alone included: Lucidos does not propose it and Apply refuses it, with the Not ready reason `plan_awaiting_approval`. Once approved, a plan-only branch proposes and applies with no hardening, and the plan lands on its own or with the work that implements it. A plan the user never approves stays on its branch.

`mark --security-fix` is the **bounded security-fix lane**, for one case: a run nobody can be asked. An UNATTENDED session may commit a security fix with no prior plan decision. The fix must stay inside the files `--files` names, and ship a regression test. It records `bounded_security_fix`, which satisfies the gate at once.

The lane is a distinct state on purpose. It claims neither that the work is local nor that a human approved, only that an unattended run bounded itself. **Apply refuses the branch if it touched anything outside the list**, `docs/plans/` excepted. If the fix has to grow, re-run the command with the full list. The engine caps the list and refuses a mark with no bound. If you can ask the user, ask: the lane is for security work only, not a general way past the gate.

Anything wider stays gated. The session commits its plan, leaves the marker `proposed`, and reports that it is blocked on a decision. See ADR 0154.

`planned`, `acknowledged_simple` and `bounded_security_fix` satisfy every gate. `proposed` and a missing marker both block. App coding-agent threads and external repos are exempt (the gate is a no-op there). The `implementation-plan` skill drives `mark --plan` / `approve`, and `mark --simple` is the agent's escape hatch for a change too small to plan. `lucidos cc-plan-gate` is the hidden PreToolUse hook that enforces this, never invoked directly.

### `lucidos successor offer --at <name> [--progress <done>/<total>] [--ask | --default-floor]`

Run a *handoff checkpoint* and print what to do. A repository names its checkpoints in its plan skill, review flow or agent instructions, and the engine knows none by name. `lucidos knowhow read system-knowhow/coding-agent-flows` shows how to wire one, and holds two default flows.

It posts `POST /api/v1/threads/:id/handoff-checkpoints` for `$LUCIDOS_THREAD_ID`. The engine records the run as `HandoffCheckpointReached` and answers with its *handoff estimate*. The CLI compares no number itself.

A refusal reads as an unknown reading, so the thread carries on. A 404 with an empty body means the engine has no such route, so it is older than the CLI. The CLI says so and asks for a rerun after the next switch to the new version. A spawned process normally runs the CLI its engine pinned at startup, so the two match.

- **`--at <name>`**: lowercase letters, digits and hyphens, up to 64. Runs of the same name in the same repository, on the same model and effort, are the history the estimate reads.
- **`--progress <done>/<total>`**: the flow's units of work done. The estimate reads `total - done` units left, else one. A plan flow counts phases.
- **`--ask`**: an offered checkpoint, which prints a question card. Without it the checkpoint is automatic and asks nothing.
- **`--default-floor`**: an automatic run only, so it conflicts with `--ask`. It declares the surface's *default rung* the *default floor* for this run, as the hardening flow does. The engine then may name two rungs from the thread's *intelligence ladder*, each one only to a thread below it:
  - the **floor rung**, the default rung itself;
  - the ***risk rung***, the rung just above the default, never the top.

  It names neither when the ladder does not climb, the thread runs off it, cannot hand off, or holds an open change. The CLI keeps no copy of the ladder: it prints the model and effort the engine returns.

An **automatic** checkpoint prints one of these:

- **Hand off at the context ceiling**: the session is past its *context ceiling*, so it hands off here whatever the estimate says. The engine reports `past_ceiling` once the session tops the ceiling in force. That holds mid-turn too, since a checkpoint is a better boundary than the turn end. Hand off as below, on a floor rung if the engine names one.
- **Hand off**: the estimate pays. Commit, write a *handoff narrative*, then run `lucidos successor start --narrative <file>` with the same `--at` and `--progress` and no implementer flags. The narrative quotes the flow's remaining checkpoint instructions. End the turn.
- **Hand off on the floor rung**: the engine named a floor rung. Hand off as above even when the estimate does not pay, adding the `--coding-agent-model` and `--reasoning-effort` the output names.
- **The risk rung**: when the engine names one, the output adds two tests, and the agent hands off on it instead when the change passes either. Say why in one line. With no other handoff due, carry on unless the change passes a test.
  - **Security work**: a security or authority boundary, secrets, credentials, pairing, sandboxing, or what an agent may do.
  - **A costly one-way door**: a migration or new stored format, a published contract, a release or install format, or destroyed work.
- **Carry on**: the estimate does not pay, or Lucidos could not make a reading.

An **offered** checkpoint prints the card's slot rules and eligible shapes.
It repeats the verdict at the end. Read the whole output without `head` or `tail`:

- The estimate paying or the context ceiling being exceeded recommends a fresh session.
- At plan approval, always offer R. Unless paying or past ceiling, mark `Approve here (Recommended)` instead and display it first.
- Elsewhere recommend a lower rung that fits, or one or two rungs above for hard work when available. Never raise to the top.
- Follow the flow's backend policy in the repository's instructions. Without a policy, keep the current backend. A user request or flow policy may permit comparison or prefer another backend. Intelligence fit takes priority over backend preference. At plan approval, a readable other backend always gets X, merged into R when R names it. Elsewhere alternatives remain optional.
- Plan approval reserves `Commit plan for later`; repository checkpoints never offer it.
- Fill A, P, X, R, F, T and display the recommended option first, at most four, as described under `planned` above. A typed reply requests changes.
- If handoff is unavailable or the ladder cannot be read, offer no fresh session.

Automatic output prints each other backend's floor and eligible risk rung with
full flags. Take one only when the user, plan or approval pick named that backend;
otherwise follow the own-backend verdict.

The card prints this thread's implementer and the ladder with this thread's rung marked (`lucidos intelligence-ladder`). The agent recommends a rung by its judgement of the work left (ADR 0486, ADR 0500). It names the rung by picker label and effort, as the ladder prints them, and the backend only when it is not this thread's. Picking an `Implement in a fresh session` option does what the flow says that decision does, then runs `lucidos successor start` below.

