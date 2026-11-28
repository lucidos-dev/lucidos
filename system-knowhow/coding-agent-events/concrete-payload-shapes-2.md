---
name: Coding Agent Lifecycle Events, Concrete payload shapes (continued)
description: Part of Coding Agent Lifecycle Events. Its index lists the sections it covers.
---

One part of `system-knowhow/coding-agent-events`, which lists every part.

## Concrete payload shapes (continued)

### `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved`

Used by the MCP permission-prompt subprocess (the `mcp__permission-prompt` tool family). Distinct from `UserQuestionAsked`/`UserQuestionAnswered`; the next section says why both exist.

```json
{
  "type": "CodingAgentPermissionRequest",
  "data": {
    "request_id": "req_…",
    "tool_use_id": "toolu_…",
    "tool_name": "Edit",
    "input": { "file_path": "...", "old_string": "...", "new_string": "..." },
    "summary": "Edit /path/to/file.rs"
  }
}
```

```json
{
  "type": "CodingAgentPermissionResolved",
  "data": {
    "request_id": "req_…",
    "allowed": true,
    "reason": null,
    "persist_scope": "session"
  }
}
```

`persist_scope` is `"narrow"` / `"broad"` / `"session"` when the user clicked an "Always allow"-style button. It is `null` for Allow-once, Deny, orphan-recovery, supersession and the session-ended clear. The frontend uses it to check the chosen button on the answered card and strike through the rest.

If a command reaches the permission handler while standing denies are unavailable, the request stamps `grant_offer.withheld` as `standing-deny-unavailable`. For a private-file read it stamps `standing-deny-unavailable-secret-read`. The card explains the failed check and offers Deny and Allow once. The engine checks the rules again before an Allow once takes effect.

If the workspace floor cannot rule out that a command deletes a Lucidos workspace or its database, the request stamps `grant_offer.withheld` as `unproven-workspace-destruction`. The card waits for a human and offers only Deny and Allow once.

**The Codex shapes.** Codex raises the same two events through the app-server approval bridge, under its own item names. A `command_execution` carries `{command, cwd, reason?}` and summarizes as `command_execution <command>`.

A `file_change` is the awkward one. `item/fileChange/requestApproval` carries only `itemId` / `threadId` / `turnId` / `startedAtMs` plus a nullable `reason` and `grantRoot` (both `null` in practice), and **no paths at all**. The paths come from the `item/started` notification codex sends for the same item id just before the approval. The app-server driver copies them onto the input, stripped to `{path, kind}` so a multi-file patch's inline diffs never reach the event store:

```json
{
  "type": "CodingAgentPermissionRequest",
  "data": {
    "tool_name": "file_change",
    "input": {
      "item_id": "exec-32d7f4c4-…",
      "changes": [{ "path": "/Users/me/notes.txt", "kind": { "type": "add" } }]
    },
    "summary": "file_change /Users/me/notes.txt"
  }
}
```

`summary` names up to three paths, then appends `+N more`. A trigger condition should key off `tool_name` and `input.changes[].path`, not the prose. If the `changes` list was never announced, the `changes` key is absent. The summary (and the card) then falls back to `reason` / `grant_root` / the bare tool name.

**The grant offer.** Every card carries an optional `grant_offer`: the patterns its "Always allow" (`broad`), "Always allow `<head>` commands" (`narrow`) and "Allow for this thread" (`session`) buttons store. A missing key means no such button. A command's head is the first command in the line that is not an obvious read, so `cat x; gh run rerun 4` offers `Bash(gh:*)`. When every command is a read, it is the first one. Cards raised before the field existed lack it and offer no grant buttons.

**The agent's own words.** A shell command carries the agent's one line on what it does and why. Claude Code sends it as `input.description` on `Bash`, and Codex as `input.reason` on `command_execution`. The card leads with that line and shows the full command under it. A notification built from this event should prefer it too, since `summary` carries the whole command. Either field can be absent, so fall back to `summary`.

**Supersession.** If the user types a new message while a permission card is pending, the engine resolves the card as `allowed: false` (`reason: "Superseded by a new message"`). It then routes the typed text to the coding agent as a normal follow-up. So the thread is not left stuck on `waiting_for_user_answer` while the agent moves on. This mirrors the AskUserQuestion path, where typing becomes a `FreeText` answer, but a permission has no answer, so it is denied. Emitted from `resolve_pending_permissions_as_superseded` (`engine/cc_permission.rs`).

**A parent's `follow_up_child_thread` does NOT take that path.** Only a human's
message supersedes a pending permission card. An agent-sent message to a
coding-agent child parked on a card becomes a *held message*: the card stays
pending, and the child receives the message once a human answers the card
(ADR 0256). Two related facts about following up a coding-agent child:

- A child parked on a **question** is blocked on a human, not on work. Your
  message is not an answer to that question. It becomes a *held message*: the
  result says `held`, the question stays open, and the child receives your
  message once a human replies. `urgent` does not change that.

  After a human
  cancels the question, later messages stay held until the human writes again.
  A released message reaches the child as input in the answered turn, or as its
  next turn. Its `CodingAgentInputRead` records when the child read it.
- A follow-up that RACES the child's own finish can produce a completion card
  for the turn you interrupted. That does not mean the redirect failed: the
  redirected turn reports separately when it ends.

The glossary covers everything else about follow-ups: queued versus urgent, what
urgency costs, why it is a no-op on Codex, and that the child cap never refuses it.
See the *child follow-up* and *urgent follow-up* entries in
`system-knowhow/glossary.md`.

**Session-ended clear + non-resurrecting resolution.** A permission card is answered by unblocking an **in-memory** broadcast waiter. It never spawns a resume, unlike `UserQuestionAnswered`, which can resume an already-idled thread. Two consequences:

1. A coding-agent session can **idle** with a card still pending: a parallel subagent's card outlived the main turn, or the turn was canceled. `emit_coding_agent_idled` then clears it via `resolve_pending_permissions_as_session_ended`, whose `reason` starts "Coding agent session ended before answering". So a finished thread leaves no clickable card behind.
2. The projection flips a thread to `running` on `CodingAgentPermissionResolved` **only from `waiting_for_user_answer`**. A resolution on an idle or terminal thread leaves the status unchanged: a stale click hours later, or the session-ended clear itself. That keeps a stale tap from making a dead `running` thread.

The same non-resurrecting rule applies to the chat `CommandPermissionResolved` / `McpPermissionResolved` lanes (shared projection arm).

**Only a Deny click reads as "User denied".** `DENIAL_REASON` ("User denied") is reserved for an explicit Deny. Two other endings return `allowed: false` with a neutral reason instead:

- **An engine restart.** The in-memory broadcast channel closes, so the waiting `prompt_coding_agent_permission` call returns `RESTART_INTERRUPT_REASON` ("Interrupted by an engine restart, not a user decision").
- **A card the engine withdraws**, because a new message superseded it or the session idled. The agent reads that sweep's reason plus "not a user decision".

The chat agent's command and MCP lanes follow the same rule. These reasons are only what the agent reads in the MCP or app-server response. The persisted `CodingAgentPermissionResolved` reasons above (orphan recovery, supersession) are unchanged.

A tool call that was *running* at teardown gets no such reason. Claude Code closes it in its own transcript as "The user doesn't want to proceed with this tool use". It adds `[Request interrupted by user for tool use]`, and the resumed session replays both. So every resumed turn whose gap ends in a restart abort carries a restart line in its turn-gap note (see § "The turn-gap note" below). The line says the shutdown wrote that text, the user refused nothing, and the call is safe to run again. Without it, the resumed agent reads a rejection and abandons its plan.

The recovery system prompts (`recovery_system_prompt` and siblings in `engine/agent_session/prompts.rs`) also carry a **restart-not-rejection note**, but only a recovery spawn uses them.

**Every session uses the workspace's grants.** Whoever started a coding-agent session, its permission requests take the same gates, in order:

1. the catastrophic floor, which denies a proven match whatever the grants say. An unproven match goes straight to the card (step 6);
2. the in-worktree write fast path (below);
3. an "Allow for this thread" the user clicked on this thread;
4. the workspace's standing "Always allow" grants;
5. the command gate, which may let the command guard answer;
6. a card that waits for a human, however long it takes.

So a main check's fix thread, a landing thread and a trigger's run all use a grant the owner clicked. Answering a card never changes who is trusted: a click stores exactly the pattern it names.

**The trust decision sits at spawn.** Before any coding-agent run starts, resume and follow-up included, the engine traces it to its cause (`agent_session::spawn_gate::spawn_provenance`). The trace follows provenance through child spawns, top spawns and `lucidos spawn-thread`. A move to top level does not end it. A run whose cause is a trigger starts only when that trigger holds its *coding-agent grant* (see `system-knowhow/triggers/grouping-triggers.md` § "Coding-agent grant"). So does a run that another trigger's fire fed, through a continue key into a thread on the trace.

A run whose cause is a webhook, a system event or an agent with no parent never starts. Neither does a run whose trace the engine could not read.

A refused run ends with a `ResponseFailed`, and a refused trigger run also notifies "<trigger name> failed".

**A catastrophic deny emits the ordinary `CodingAgentPermissionRequest` + `CodingAgentPermissionResolved` pair**, back to back, with the command redacted as on a card. So the timeline says what the engine refused and why. The pair renders as an already-answered card, which never parks the thread. Under its buttons the card shows the first sentence of the engine's reason, as for every resolution a human did not click. So an engine deny never reads as the user's Deny.

**A trigger subscribed to `CodingAgentPermissionRequest` therefore fires for these too**, on a request the engine already answered. Pair it with the resolution, which lands in the same turn, to tell an engine-answered deny from a card a human still owes.

Older threads may also show cards the engine answered for an unattended trigger run. That lane is gone: such a session now takes the gates above.

This covers both backends: CC's MCP path and the Codex app-server bridge funnel through one engine function, `prompt_coding_agent_permission`.

**In-worktree writes never render a card.** Claude Code auto-approves in-cwd writes under `--permission-mode acceptEdits`, **except** under `.claude/` and `.git/`. Those go through the permission-prompt tool in every mode, whatever `--allowedTools` says. Lucidos keeps its agent configuration in `.claude/`, so editing a rule or skill cost a click per save. The persisted "Always allow" scopes cannot suppress it, which is why those cards hide the broad button.

So the engine answers first. A **file write whose target resolves inside the session's own worktree** is auto-allowed with no card and no events. That is safe because the worktree is disposable and every change in it is reviewed in the Diff before Apply. Containment is **resolved against the real filesystem, not matched lexically**. The engine canonicalizes the worktree root and the target's longest existing prefix, so a symlink inside the worktree cannot launder an outside write. Four deliberate limits:

- A `..` component fails containment.
- A relative path fails closed. Resolving one needs the agent's cwd, which is `data/apps/<id>` for an app coding-agent thread. Both backends send absolute paths anyway.
- Any `.git` path component still renders a card, checked before *and* after symlink resolution. Git metadata is not in the reviewed diff, and a written hook would run on the next commit.
- Commands (`Bash` / `command_execution`) are never covered by this check. A workspace grant decides them (below).

A write **outside** the worktree, such as the user's global `~/.claude/settings.json`, still asks. See `cc_permission::{worktree_write_auto_allowed, path_inside_worktree}`.

**Two folders join the worktree as *working directories*.** The engine grants them in `cc-settings.json`, so reaching either raises no card: the workspace's `data/` tree, and the OS temp dir. The `--permission-mode` above is a preference, the *coding-agent permission mode*. Under `auto`, Claude Code's own classifier decides instead of carding.

**"Allow for this thread" survives an engine restart.** The click records a grant the thread holds in the permission store, which a restart keeps in all three card lanes. The pattern is the request event's `grant_offer.session`, the pattern the card named, so the two cannot drift. An Allow-once, a Deny and the engine's own resolutions record nothing. Deleting the thread deletes its thread grants. Allows clicked before the upgrade import once as thread grants.

**"Always allow" binds the session it was clicked in.** Both persisted scopes record a standing tool grant for this workspace. Claude Code gets the standing grants as `--allowedTools` at spawn. That flag is frozen for the subprocess's life, so the engine's gate also reads the grants itself, on every prompt. The grant covers the running session, and every other live thread in the workspace.

Lucidos imports the legacy `cc-allowed-tools` file at boot and deletes it after the grants are saved (ADR 0677). Manage standing grants under **Settings → Permissions → Coding agent permissions** or use **Always allow** on a permission card.

It honours a stored pattern only where `derive_allow_pattern` would have produced it, which is the codebase's own record of what CC respects. A line means the same whichever agent asks (ADR 0571).

A bare `Edit` / `Write` / `NotebookEdit` / `ExitPlanMode` line covers nothing. Only the bare `Bash` line covers a `Bash` or Codex command that may write under `.claude/` or `.git/`. A `Bash(<head>:*)` line never does, so its card offers "Always allow" alone.

Every `file_change` still cards. A command that names those paths is covered when every segment only reads, such as `grep -n x .claude/rules/a.md | head` (ADR 0634). A catastrophic command is still denied, whatever the file says. See `cc_permission::persisted_allow_covers` and ADR 0125.

**The engine decides a card's buttons.** It stamps them on the request event as `grant_offer`: a `broad`, `narrow` and `session` pattern, each present only when storing it would cover this same request. The card renders exactly those, and a click stores exactly the stamped pattern. So no button stores a line that covers nothing (ADR 0571).

**A Codex command card offers the same buttons as a Claude Code one.** "Always allow `<head>` commands" writes Claude Code's own line, `Bash(<head>:*)`, read from inside Codex's `/bin/zsh -lc '…'` wrapper. The bare "Always allow" writes `Bash`, which covers any command for both agents. Two kinds still card under it: a secret read and a catastrophic command.

**The `<head>` is the first command that is not an obvious read.** The gate covers a command only when every segment is granted or is an obvious read on its own, and one is granted. So `cat x; git push` passes on a `Bash(git:*)` line, and `git status && rm -rf x` still asks. A command needing two grants, such as `git push && curl …`, offers no narrow button, since one line could not cover it.

**A grant reads through nested shells, and never names one.** The head comes from the innermost of up to four `-c` layers, so `/bin/zsh -lc "bash -c 'git push'"` offers and matches `git`. A head that runs its own arguments grants nothing: a shell, an exec wrapper such as `timeout` or `xargs`, `sudo`, `env`, `eval` or `source`. No card offers it, and the gate never matches it. A redirect in front of a wrapped shell also cards. See `command_guard::grant_covers_command` and ADR 0568.

A grant naming such a head, such as `Bash(bash:*)`, is left out of `--allowedTools` at spawn, so it is inert.

**A trusted repo script goes to the judge, like `make`, instead of a card.** A path head such as `./scripts/check.sh` is normally refused outright, since the agent may have just written it. The command gate trusts it when it is a tracked file of the session's worktree whose bytes match the default branch. A relative head is trusted only when the request names an absolute `cwd` (Codex does) and the line changes no directory. See `engine/trusted_repo_scripts.rs`.

**A trusted repo script is grantable by its exact path.** The card offers "Always allow `./scripts/check.sh` commands", which stores `Bash(./scripts/check.sh:*)`, the worktree-relative path. The line covers a request only while the script is trusted and no segment runs before it, so `echo x > s.sh && ./s.sh` still cards. Claude Code never sees such a line in `--allowedTools`, since it would match with no trust check. A Claude Code request names no `cwd`, so a relative script head from it offers no path button. A basename line such as `Bash(check.sh:*)` covers no path head (ADR 0571).

**Plugin MCP tools meet the same gate.** A Claude Code session starts the MCP servers of the user's enabled Claude Code plugins, beside the engine's `lucidos_perm` (ADR 0398). A plugin tool such as `mcp__plugin_slack_slack__conversations_replies` cards like any other tool, and "Always allow" records that one tool. To allow every tool of a server, add its server rule (`mcp__plugin_slack_slack`) under **Settings → Permissions → Coding agent permissions**. Claude Code applies it from the next session it spawns. A plugin whose launcher exits non-zero, such as a region gate refusing, adds no tools and leaves the session working.

