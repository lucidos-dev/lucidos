---
name: Lucidos Glossary, Advanced (coding agents) (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Advanced (coding agents) (continued)

### Integrated sub-thread
A *sub-thread* spawned to fold into its parent. It runs the fast tier of *hardening*, proposes nothing, and hands its branch up. Its parent is the *integrating parent*. A coding agent spawns one with `lucidos spawn-thread --relation child --integrated`, for a follow-up of its own work in Lucidos's own repo. The default spawn stays independent.

- **It has no change and no Apply.** Once hardened, its work waits as *unproposed work* with reason `integrated_into_parent`.
- **Its ordinary decisions go to its parent.** It runs `lucidos wake-parent yes` and ends its turn with the decision and options in its report. The parent can reply by messaging the child. Prepared permission requests still go to the owner.
- **Its report names its branch and the head its hardening certified.** The integrating parent's agent merges that branch into its own. Lucidos never writes the parent's branch.
- **The parent's own hardening runs every suite once**, over all the merged work. Its review skips what a child's hardening already reviewed.
- **An archived parent sets an unmerged child's work aside**, as for any archived thread with *unproposed work*. Once the parent's change lands, a merged child's commits are on `main` and its worktree is finished.
- **A parent's change that lands without the child's branch sets the child's work aside.** That covers a child that is idle and has reported to the parent. The strip stops naming the parent, and the child goes on hold: **Resume** offers the work, or **Unarchive** if the child is archived.

### Withdrawn change
A *change* that went back to *unproposed work* (`ChangeWithdrawn`). Its row keeps `status = withdrawn`, and the branch keeps every commit. Four things withdraw one:

- a turn that did not finish and moved the branch past a pending or set-aside change;
- a session that was running when Lucidos stopped, for its pending change;
- **Resume** or **Unarchive** returning an incomplete *set-aside change*;
- the first start after ADR 0400, for each change an older Stop proposed.

A withdrawn change never blocks Archive. The thread returns to the inbox, and its next proposal gets a new change.

### Incomplete change
Retired as a kind of pending *change* (ADR 0400). Work from a turn that did not finish is now *unproposed work* with reason `turn_incomplete`. The word survives only as the " · Incomplete" label on a *set-aside change* the archive filed.

### Change summary
One line saying what a multi-commit *change* does, written by a background model once the change is proposed. It heads the change card in the thread, the Applied / Discarded / Reverted toasts, and the Changes panel row. The card and the row unfold to the change's commits, oldest first. A one-commit change gets no summary, since its commit subject is the line. Until a summary lands, the oldest commit subject stands in, never the newest (usually a small fix). New commits clear the summary and a fresh one is written.

The model is set under **Settings → Models → Background tasks → Change summary**, and follows the title model until you set it.

### Claude Code
Anthropic's coding-agent CLI; the default *coding agent* Lucidos integrates (the other is *Codex*). Often abbreviated **CC**. Modeled in code as `CodingAgent::ClaudeCode` (enum, wire value `"claude-code"`). The thread channel value `"claude_code"` is historical and shared by every coding-agent thread: it means "coding-agent channel", not "runs Claude Code". The per-thread backend lives in the `coding_agent` column / event field.

### Codex
OpenAI's coding-agent CLI; the second *coding agent* Lucidos integrates. Modeled in code as `CodingAgent::Codex` (enum, wire value `"codex"`). Picked per thread via the coding-agent chip on the *compose destination* picker. The default is *Claude Code*, remembered per workspace via the `coding_agent_default` preference. The choice locks at the thread's first message. Only a handoff to a successor session may switch it.

Codex sessions run in an OS sandbox scoped to the thread's *worktree*, with two deliberate extras. The workspace's `data/` tree is writable, so `lucidos data write` works, and so is the worktree's shared git dir, so `git commit` works. Nothing else in the *workspace* is writable: not `.lucidos/`, not a sibling worktree.

User questions work as for Claude Code: Codex asks via the `ask_user_question` tool, and the answer renders as the usual question card. Under the default protocol, a permission card appears when a command or file change must escalate past the sandbox. The `exec` escape-hatch protocol runs non-interactively, with the sandbox as the only guard. The Apply / Discard flow, *changes*, and *hardening* work as for Claude Code. Lucidos classifies an escalated command first, and a plain read it recognises raises no card (see *coding-agent permission card*).

### Coding agent
Role: a subprocess driving a *thread* to make code changes inside an isolated git *worktree* (dev). Lucidos integrates two: *Claude Code* (default) and *Codex*. Modeled in code as `CodingAgent` (enum). The thread it drives is a *coding-agent thread*. Its backend stays fixed within a session; a successor session can choose another.

### Coding-agent branch
The git branch a *coding-agent thread* works on, named after the thread so `git branch -a` reads as a list of work: `lucidos-<coding-agent>-<app|repo>-<name>-<slug>-<id>`. Examples: `lucidos-claude-code-repo-lucidos-fix-auth-timeout-401a2d19`, `lucidos-claude-code-app-habit-tracker-add-streaks-401a2d19`, `lucidos-codex-repo-example-repo-fix-auth-401a2d19`. The `lucidos-` prefix marks it as Lucidos-made, which matters most in an *external repo* among your own branches. The short id at the end is the thread's own, as in its *worktree* folder name. It keeps threads started together apart, even when their prompts open alike.

The name is fixed when the branch is created. Renaming the thread does not move it, and a continued thread keeps its branch. *Apply* merges this branch into `main`. Older branches (`claude-code/…`) keep their names and keep working.

### Coding-agent sign-in

A *terminal suggestion* that signs a *coding-agent thread*'s CLI in: an MCP
server for Claude Code or Codex, or a Claude Code account login. An MCP one
appears when the agent calls a tool on a server that needs a sign-in, and says
so. Codex hides such a server's tools, so its one appears when Codex reports
the server signed out. A login one appears when a turn fails because the CLI is signed out. Type
`/mcp` or `/login` to get one on request.
**Run in terminal** types the CLI's own login command into the *thread
terminal*, which carries the thread's account profile, so the login reaches the
agent. After a tap, your next message starts the agent afresh so it sees the
login. While Terminal is off, the suggestion says where to turn it on, and
offers Copy.

### Coding-agent permission card
The approval card a *coding-agent thread* shows when its agent wants something the engine won't wave through: Deny, Allow once, and a **Remember** menu holding Allow for this thread and Always allow. The thread waits until answered. Anything the agent writes **inside its own worktree** is allowed automatically, since the worktree is disposable and you review every diff before Apply. A card appears for a shell command the agent's own gate escalates, or a write **outside** the worktree. It also appears for a write into the worktree's hidden `.git` folder, the one in-worktree place the diff does not show.

**"Allow for this thread" is remembered for the life of that thread**, even across an Apply that restarts Lucidos. "Always allow" applies to every future thread, in an editable list under **Settings → Permissions**. A *trigger* ends the run and asks for a *program grant* on a separate permission request.

One kind of escalated command is answered for you. When Codex asks to step outside its sandbox, Lucidos reads the command first, and a plain read it recognises runs with no card. So a Codex thread watching a process no longer asks on every `ps`. Anything not plainly safe still asks, as does anything reaching for another user's rights.

**Working directories**, plural: "outside" above means outside these, not just the worktree. Two folders join it: this workspace's `data` folder (artifacts, apps, knowhow, triggers) and the OS temp dir. Reading or writing a file in either raises no card. A shell command still can, and four `cd` shapes always do. The `auto` *coding-agent permission mode* goes further: Claude Code's own classifier approves routine actions, so most of the rest never reaches you.

Precisely: a trigger-rooted thread is never ASKED. When the engine refuses one of its requests, it records the card already answered, so you can see afterwards what was refused and why. Nothing waits on you, and the thread is not flagged as needing attention.

### Coding-agent permission mode
Which of Claude Code's own permission modes its threads run in, set under **Settings → Coding Agents → Permissions**. **Accept edits**, the default, lets writes inside the *working directories* through and raises a *coding-agent permission card* for anything else. **Auto** hands those decisions to Claude Code's safety classifier instead. It covers shapes no allowlist can, such as a command that changes directory and redirects output in one line.

Auto has costs. It ignores a blanket `Bash` entry in your coding agent permissions, so more commands reach the classifier, each paying a round-trip. An unreachable classifier denies rather than asking you, and a run of denials falls back to asking anyway. Codex has no equivalent setting. Changes apply to new sessions.

### Background task
Work Lucidos runs for a *thread* so it can outlive one tool call: a test suite, an e2e run, a long build. A *coding agent* starts one with `lucidos background-task run --description "<what it is>" -- <command>`; the *Lucidos Agent* with `run_bash_background`. The description names the task on the thread's waiting row. Lucidos runs it in the thread's worktree.

It is either a *polled background task* or a watched one. A watched task arms an *event wait* on its completion, so the agent ends its turn instead of waiting. When it finishes, the thread re-opens with the exit status and the output's tail. Stopping your own task does not re-open the thread, and `lucidos hardened mark` stops any task the thread still has running, since the hardening supersedes it.

A command the agent backgrounds on its own dies when its turn ends, which is why this exists. Discarding or archiving the thread stops its running tasks.

### Polled background task
A *background task* started with `lucidos background-task run --poll`, the usual choice for long work. The agent stays in its turn and calls `lucidos background-task wait <id>`, which returns within 230 s, until the task finishes. Every return lands inside the prompt cache's 5-minute life, so no wait pays to re-read the whole conversation. A watched task stays for an open-ended wait, such as a run queued behind a held lock.

No *event wait* is armed while the agent's turn runs. If the turn ends cleanly first, Lucidos arms one, and the thread re-opens when the task finishes. A task that already finished while the agent never read its result re-opens the thread at once, a single time. A Stop arms nothing.

### Cache keep-alive
Lucidos pinging the model of a waiting thread, only to keep its prompt cache warm. For a Claude Code agent it runs about every 4 minutes on a 5-minute cache and every 55 on a 1-hour one. It covers an agent waiting on a background task, a lock, a change, or a working child thread not spawned `no-wake`. Lucidos resumes that idle agent with a hidden ping. Each ping leaves no trace: the thread keeps showing Waiting, and no parent, wait or trigger hears of it. A message sent meanwhile waits the few seconds until it ends, and Codex agents get none.

It also covers an agent waiting on your answer to a question card or a permission prompt. That agent is busy mid-turn, so Lucidos asks a copy of its session instead. The agent and its card never notice. The run leaves a card under the question, inside the turn, and the question keeps its buttons.

A Lucidos Agent thread is kept warm too: a chat, Home or a trigger thread. It qualifies while it waits on a sub-thread, a keep-warm event or its own question card. Lucidos sends the start of its last request again and asks for no reply, about every 4.5 minutes. That works only where the model's provider has a cache that a read refreshes, such as Anthropic. A thread waiting only for your next message gets no pings.

Lucidos times a ping only from a cache lifetime the model's reply reported. With no such reply, as with Codex or most OpenRouter models, it sends no pings, and the card says the lifetime is unknown.

The keep-alive is off by default. Turn it on with the `cache_keep_alive_mode` *preference*, under **Settings → Models → Cache keep-alive**. A run pings until the thread's *expected wake* is safely past, and only when those pings cost clearly less than one reread. It never pings past the *reread cap*: from about 40 minutes to 3 hours on a 5-minute cache, by model, and far longer on a 1-hour one. For a card it learns when to stop from this workspace's cards asked near the same hour of day. With too few, it stops at half the cap.

You see the keep-alive as a whole, never the pings. With **Cache cards** on, each wait it kept warm leaves one card in the transcript, a parked wait included. That switch sits under the keep-alive's own and shows only while the keep-alive is on, since no card draws while it is off. It carries a state word: Warming, Stopped, Ended or Turned off. Its title reads "Cache kept warm" for a run whose pings kept the cache warm (Warming, Ended) and "Cache keep-alive" otherwise. A wait that never got a ping leaves no card.

One fact line follows, such as "12 of 31 pings · $0.31 · until 15:40". When the run ends it reads "23 pings · $0.29 · 14:02–15:34 · saved about $0.57". The saving shows only when your next request found the cache still warm. The card stays as a record after the run ends.

The waiting panel's Prompt cache section says the same line, and also says when the cache is not kept warm, and why. Where nothing pings a warm cache, a "Prompt cache" card under the thread says "Not kept" and when the cache goes cold. The Token Cost app labels these calls "Cache keep-alive".

Will you answer much later? Then **Turn off for this thread** stops the pings for that thread alone. It sits on the live card and in the waiting panel's Prompt cache section. It also sits in the one popover that the card and the panel each open. It turns back on by itself when the thread runs again, and **Turn back on** undoes it while the cache is still warm. The Lucidos Agent's `threads` tool does the same when you ask it in words.

### Reread cap
The pings that together cost one reread of a waiting thread's context, at the model's own prices. No *cache keep-alive* run pings past it, since pings past it would cost more than the reread they save. A card's run stops at a cap learned from the cards asked near its hour. It prices every plan: pings run toward the *expected wake* only when they cost at most 80% of one reread. With no expected wake, a run pings to the cap. A run whose wake comes inside the cap is *expected to break even*.

### Expected wake
When a waiting thread will next wake: the earliest of its event waits and the child threads that report to it. One estimate per turn, for the whole thread, recorded as `WakeExpected`. The agent states it with `expect_wake` or `lucidos expect-wake`. The newest one stands, and the *cache keep-alive* plans its pings from it.

### Expected wake decision
The background call that fills in an *expected wake* when a turn leaves its thread waiting and the agent stated none. It runs at the turn end beside the read decision, only while the *cache keep-alive* is on, and never for Codex. It reads what the thread waits on, the latest output of running background tasks, the reply, past waits like these, and earlier estimates that passed. Its chat model picks one time bucket; a System One model gives a probability per bucket. A failed call records nothing. Its model is the Expected wake row in Settings → Models (Background tasks).

### Permission answer
The background call that reads your spoken words against an open *permission request* and says which option they choose. Spoken words also answer a tool's permission card (a command, an MCP tool or a coding agent's): it stores what its button stores. Only a clear pick counts: an unsure answer or a failed call picks nothing, and a tap still works. A tap is the only answer to a request an agent asked for. Your words still reach the thread as a message either way. Its model is the Permission answer row in Settings → Models (Background tasks), and only you set it.

### Coding-agent thread
A *thread* driven by a *coding agent* (Claude Code or Codex) inside an isolated git worktree. Marked by `is_coding_agent = true` on `thread_summaries`. A `SessionStarted` opening an agent session sets it, as does another event on the `claude_code` channel. A *resume boundary* alone never does: `ContinuationStarted` fires on chat and trigger threads too.

The persisted `source` is `"claude_code"` for every coding-agent thread (the historical, backend-agnostic channel name). Public source filters use `coding-agent`, accepting `claude_code` only as a legacy alias. The `coding_agent` column names the product (`'claude-code' | 'codex'`, NULL = legacy Claude Code row). The `coding_agent_kind` column names the worktree flavor (`'lucidos' | 'app' | 'external'`). It emits `CodingAgent*` events instead of chat `Response*` events. Three flavors:

- **Lucidos-internal coding-agent thread**: works on the Lucidos workspace repo itself. Produces *changes* surfaced via the Apply / Discard UI on completion.
- **App coding-agent thread**: works on one app folder (`data/apps/<id>/`) via a sparse-checkout *worktree* of the workspace git. Produces *changes* with the same Apply / Discard UI; Apply does **not** restart the engine, and Lucidos's `/harden` does **not** run.
- **External-repo coding-agent thread**: works on a user-registered external git *repository*. It uses a different worktree-creation path and a minimal system prompt, and **skips** the Lucidos change-proposal flow on session end.

See also: `system-knowhow/coding-agent-events.md`.

### Side question
A quick question put to a thread from *side-question mode*: type it, then press the round Ask button or Enter. A typed `/btw` is ordinary text. It may carry images.

The thread's agent answers from the thread's full context, beside any running turn, with no tools. A Claude Code thread asks a copy of its session; a Codex thread asks an ephemeral fork. A Lucidos Agent thread asks its own model once. The answer shows on a card at the moment it was asked, with later output below it.

It is kept as events **no agent ever sees** (ADR 0320), so the card survives reloads and shows on every device. A tap on its head folds it to one line; a second tap unfolds it. Coding-agent threads need a saved session first. An older Codex CLI that cannot fork ephemerally shows update instructions.
See also: `system-knowhow/coding-agent-events/billing-evidence.md` § Side questions are recorded, and hidden from every agent.

### Side-question mode
The composer state where the round Ask button asks the box as a *side question* instead of sending a message. The question-mark toggle in the composer row turns it on, once the thread has started. So does ⌥↵ (Alt+Enter). Any draft stays in the box. No composer button has a hold.

A "Side question" pill above the box shows the mode. The pill's ×, Escape, a second ⌥↵, a second tap on the toggle, or asking turns it off. It wins over a waiting card, so the × is the way back to answering. It is kept with the draft, so it survives a reload and shows on every device.

### External-repo coding-agent thread
A *coding-agent thread* running against a user-registered external git *repository*, not the Lucidos workspace. It has no Apply / Discard surface: the user reviews diffs in the external-repo diff viewer. Its worktree creation and system prompt differ from the Lucidos-internal variant, as `docs/plans/2026-03-17-external-repos-plan.md` documents.

