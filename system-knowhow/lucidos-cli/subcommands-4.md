---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos spawn-thread --to <WS> --message <M> [--image <path> ...] [--coding-agent <backend>] [--folder <path> | --repo <name>] [--relation child|top] [--wake-mode <mode>] [--integrated] [--title <T>] [--model <M>] [--coding-agent-model <M>] [--reasoning-effort <level>]`

Start a new *thread* in another (or this same) workspace: a *chat thread* by default, or a *coding-agent thread* with a coding-agent flag. `--to` takes an absolute path, or a bare workspace name. A bare name resolves against `$LUCIDOS_WORKSPACES_ROOT` when set, else the directory holding your own workspace, else `~/workspaces`. So a sibling workspace is always reachable by name. Caller provenance (`caller_*` fields) defaults from `$LUCIDOS_WORKSPACE` / `$LUCIDOS_THREAD_ID` / `$LUCIDOS_EVENT_ID`, which the engine sets on every spawned subprocess. Prints a clickable `[title](thread:<ws>/<uuid>)` markdown link on stdout.

**Agents keep the default actor mode.** Omit `--mode`: it defaults to `agent`. The flag controls who authored the request. Never choose `--mode human` from an agent: it claims the user authored the request, and the CLI refuses it before resolving a workspace or sending a request. Do not strip the origin token or use a direct API call to bypass the restriction.

A workspace on another install is not a sibling: a packaged app and a dev checkout serve separate workspace sets. Reach it with `--to <workspace>@<pairing>` through an outgoing *gateway pairing*. `lucidos workspaces targets` lists the pairings under `outgoing`, and the workspace's own topology knowhow may name them too. That is the only route there, since `run_coding_agent(workspace=…)` takes a basename alone. `--via` is not the way out: it selects a return route.

**How the target trusts you.** From a thread, a spawn into another workspace goes to your own engine first. It checks your thread as it checks a coding-agent start, and the gateway carries that verdict to the target. A thread the owner started spawns with no tap. A trigger with no coding-agent grant cannot start a coding agent there either, and the refusal names it.

A workspace on another gateway on this machine works the same way after one *gateway pairing* (`system-knowhow/remote-access/settings-access-2.md` § Pair a gateway). Your gateway asks each paired gateway, and relays the spawn to the one that serves the target.

With no gateway between the two workspaces, an unpaired one, or from a plain terminal, the spawn reaches the target unverified. A coding agent it starts there waits for the owner's Allow. From a thread, the CLI names a missing gateway or pairing on stderr. If the workspace name cannot use gateway routing, it prints a generic direct-call note instead.

When the name or path matches no gateway, the CLI reads this gateway's outgoing pairings. If there are any, it names `--to <workspace>@<pairing>` for each, and repeats that hint if the direct call then fails. With none, it points to Settings → Access → Pair a gateway on the target, then adding that remote gateway here. The wait is not a failure, so do not send the spawn again. Don't allow closes it as refused. If Lucidos cannot raise the card, the run fails with that cause instead.

`--relation top` (the default) starts an independent thread that does not report back. `--relation child` is a same-workspace spawn with a callback: the child reports to the calling thread when it finishes.

Pick `--relation child` only when the calling thread will act on the result. A child's report costs the caller a turn or a filed report, and a top-thread costs it nothing (`system-knowhow/orchestrating-sub-threads.md` § Child or top-thread).

`--wake-mode` sets the child's *wake mode*, so it needs `--relation child`. `wake` re-opens this thread for every report: use it when you need the result to continue. `wake-when-all-done` files each clean report until no other child of the batch still works, then wakes you. `no-wake` files every report, failures included, for a result the user reads in the child that you may still want next turn. Omitted, it is `wake`, or `no-wake` when the calling thread is the home thread. A failure wakes you under every mode but `no-wake`, the mode lasts the child's life, and the engine refuses any other value.

**A spawn needs something to say.** A blank `--message` with no `--image` exits non-zero before any request, and the target engine refuses one with a 400 too. No thread is created either way. The usual cause is `--message "$(cat brief.md)"` where `brief.md` was never written, so guard it: `test -s brief.md && lucidos spawn-thread … --message "$(cat brief.md)"`.

Pass `--hook-arg <name>=<value>` once per declared event hook argument in the
destination workspace. See `system-knowhow/event-hooks.md` § Declared arguments.

Neither relation needs the *workspace owner*. Any thread may create a top-thread, in any turn (ADR 0475). A top-thread still sits under the workspace, not under you, so you gain no reach over it.

**Images:** `--image <path>` attaches an image file to the message. Repeat it for several. The target engine judges each one exactly as it judges an image attached in the app. It reads the format from the bytes, takes PNG, JPEG, WebP, GIF and HEIC, and fits a large image to the model's size limit. Any other format fails the whole spawn with a 415 naming it, before any thread starts. The file extension does not matter, and a missing file fails before any request.

The images travel inside the spawn request as base64, which the engine caps at 100 MiB. So keep one spawn's images under about 75 MiB in total. A larger set fails with a 413.

**Coding-agent backend:**

- `--coding-agent <backend>`: the backend selector, and the spelling to use. Valid values are `claude-code` (alias `claude_code`) and `codex`. Passing it implies coding-agent mode.
- `--cc` and `--codex` are legacy shorthands for the two backends. They still work, but write `--coding-agent`.

**Model and reasoning level (either backend):**

- `--coding-agent-model <m>`: the model the coding agent runs on, as the ladder or the picker names it. The engine checks it against the backend's list when it creates the thread, and answers 400 with the valid ids. The CLI prints that answer and holds no list of its own. A follow-up on an existing thread is not checked.
- `--reasoning-effort <level>`: how hard the coding agent thinks. One of `low`, `medium`, `high`, `xhigh`, `max`. It pins the level for this spawn only. It needs a coding-agent flag, and the CLI refuses an unknown level before sending anything. A chat-thread spawn reads a different ladder and does not take this flag.

**An agent picks an intelligence rung.** From a coding-agent session, whatever `--mode` says, a model or effort you name must be an *intelligence rung* on the backend's *intelligence ladder*. Lucidos refuses any other pick with reason `off-intelligence-ladder`, and the CLI prints the ladder under the refusal. A model with no effort is judged at that model's default effort, on both backends. Naming neither runs on the *default implementer*: Opus 5.5 at `medium` for Claude Code and GPT-6.2 Sol at `medium` for Codex, unless the owner changed it.

A chat thread's `--model` must be a rung on the Lucidos Agent's ladder (`lucidos intelligence-ladder --surface chat`). It is judged at the account's tier for that model, else its default effort.

**Codex offers `max` on the GPT-5.6 and GPT-6 models only, so pair the two.** `--reasoning-effort max --coding-agent codex` needs `--coding-agent-model` naming one of those models, or the CLI refuses the spawn. Naming no model counts too: the engine tests against the model in the request, so `max` with no model is dropped before Codex is asked. Every level below `max` is unrestricted on both backends and needs no model.

**Worktree targeting for coding-agent threads:**

- `--repo <name|uuid>`: create the worktree from a registered *repository*. Defaults from `$LUCIDOS_REPO` (the calling thread's repo), so a coding-agent sidequest stays in its caller's repo. Pass `--repo ""` to force the target workspace's default repo.
- `--folder <path>`: target an app folder instead, spawning an **app coding-agent thread**. A `data/apps/<id>` value (workspace-relative, resolved on the *target* workspace) creates a sparse-checkout worktree narrowed to that app folder. Its *Apply* ff-merges into the workspace's `main`, with no `/harden` and no engine restart. The `run_coding_agent` tool's `folder` argument uses the same machinery. Only whole app folders are valid: the engine rejects other `data/` subtrees, app subpaths, and folders that do not exist.

`--folder` and `--repo` are mutually exclusive. `--folder` requires a coding-agent flag (`--coding-agent`, `--codex`, or `--cc`), or the CLI errors before any HTTP round-trip. `--folder` suppresses the `$LUCIDOS_REPO` default, since the engine rejects a request carrying both a repo and a folder.

```bash
# Spawn an app coding-agent thread to work on an app in this workspace.
$ lucidos spawn-thread --to myws --coding-agent claude-code \
    --folder data/apps/habit-tracker \
    --title "Research session" \
    --message "Run one research session per app knowhow."
[Research session](thread:myws/2f1c…)

# Spawn a Codex coding-agent thread in the dev workspace.
$ lucidos spawn-thread --to dev --coding-agent codex \
    --title "Codex review" \
    --message "Review the app folder and fix the failing test."
[Codex review](thread:dev/7a42…)

# Ask for the hardest thinking the backend offers, for this spawn only.
$ lucidos spawn-thread --to dev --coding-agent claude-code \
    --reasoning-effort max \
    --title "Ideation session" \
    --message "Explore three designs for the capacity policy."
[Ideation session](thread:dev/9b07…)
```

**Integrated sub-threads:** `--integrated` spawns an *integrated sub-thread*, a follow-up of your own work that folds into your branch. Use it for follow-ups of your own Lucidos-source work, so the whole set lands as your one change. It needs `--relation child` and a coding-agent flag, and does not work with `--mode human`. The child must be Lucidos source (no `--folder`), and you must be a Lucidos-source coding-agent thread. The CLI or the engine refuses anything else, naming the flag to fix.

- The child runs the *fast tier* of `/harden` and proposes nothing. It has no change and no Apply.
- Its report names its branch and the head its `/harden` certified. Merge it into your branch with `git merge <branch>` when it arrives. The engine never writes your branch.
- Your own `/harden` then reviews only what no child's run reviewed, and runs every suite once over the merged work.
- If you are archived with a child's branch unmerged, its work is set aside like any archived thread's.

```bash
$ lucidos spawn-thread --relation child --integrated --to "$(basename "$LUCIDOS_WORKSPACE")" \
    --coding-agent claude-code --title "Follow-up: retry the probe" \
    --message "Make the health probe retry once on a refused connection."
```

### `lucidos intelligence-ladder [--thread <thread> | --surface <chat|claude-code|codex>] [--via <gateway>] [--to <workspace[@remote]>] [--json]`

Prints one *ladder surface*'s *intelligence ladder*: the rungs an agent may pick, least to most intelligent. `chat` is the Lucidos Agent's ladder, which binds `run_thread`'s model and a trigger's model. Each line names the rung's picker label, effort and how a pick names it: spawn flags for a coding agent, `model=effort` for chat. It shows the intelligence, cost and time *bands*: rough multiples of the default rung's intelligence, cost and time per task (Claude Code's, for a surface with none). On a free route, such as OpenCode's free tier, cost reads `included`. A paid route reads a price, whoever pays, so a Claude gateway subscription reads one too.

Inside a coding-agent thread it defaults to that thread's backend and marks the thread's rung. `--surface` names another surface, and `--coding-agent` is its old name. `--json` prints the engine's reply from `GET /api/v1/intelligence-ladder?surface=`, which also lists the rungs that are off (`"on": false`) and, for a coding agent, the *long-prompt share*. `GET /api/v1/coding-agent/intelligence-ladder?coding_agent=` is the route's old name.

`--thread` selects that thread's recorded backend, model and effort. A routed
query requires an explicit `--thread` or `--surface`; it never substitutes the
calling source thread for a thread in the selected workspace. Reading a ladder
does not change its preferences or the thread's model.

When the current pair is outside the enabled ladder, a separate line names its model and effort as `off-ladder`, even with no enabled rungs.
This line reports the thread's actual pair without adding a selectable rung.
Plan and successor cards use the same reporting.

Plain `lucidos intelligence-ladder` in a thread prints those other ladders below its own, with full backend/model/effort flags and an explicit comparison statement.
With `thread_id`, the API also returns `other_backend_ladders` for every other installed backend.
These ladders use this thread's reference rung, so estimates compare when both references are present and equal.

Binary detection honours the configured CLI paths and PATH without running either CLI.
Automatic default-floor checkpoints also return `handoff.other_backends`, naming each backend's default and risk rungs.
They appear only when the thread can hand off and holds no open change.
Take another backend only when the user, plan or approval pick named it.


The shipped ladder is Lucidos's own estimate. The owner edits all three ladders per workspace in Settings → Intelligence. The preferences are `intelligence_ladder_chat`, `intelligence_ladder_claude_code` and `intelligence_ladder_codex` (see `preferences.md`). A pair with no estimate prints `not estimated`.

#### Editing a ladder: `lucidos intelligence-ladder set-rungs` / `remove-model` / `sort` / `set-default`

Each verb takes `--surface chat|claude-code|codex` and `--reason <R>`:

| Verb | What it does | Route |
|---|---|---|
| `set-rungs <model=effort>...` | Turns on exactly these rungs, in this order. The default rung must stay on. | `POST /api/v1/intelligence-ladder/rungs` |
| `remove-model <model>` | Turns off every rung that runs the model. Refused for the default's model and the last model. | `POST /api/v1/intelligence-ladder/remove-model` |
| `sort` | Sorts the rungs that are on, least to most intelligent. | `POST /api/v1/intelligence-ladder/sort` |
| `set-default <model=effort>` | Makes the rung the default, and turns it on. | `POST /api/v1/intelligence-ladder/default` |

The owner edits freely. **An agent needs the owner's `edit-intelligence-ladder` permission** (ADR 0750). With none, the command prints a request id and exits 2. Ask your question tool with that id alone, then run the same command again once the owner allows it. `--reason` is required then: the card quotes it. The card offers Allow once (one edit) and Allow for this thread (every edit), never Always allow.

A refused edit changes nothing and gives an Allow once grant back. `lucidos ask-permission edit-intelligence-ladder --reason <R>` asks ahead, with no `--thread`.

```bash
$ lucidos intelligence-ladder remove-model claude-fable-5-2 --surface claude-code --reason "The owner chose to remove it after the swap."
```

```bash
$ lucidos intelligence-ladder
The claude-code intelligence ladder, least to most intelligent. Intelligence, cost and time per task are rough multiples of claude-opus-5-5 at medium.
1. Haiku 5.5, low (`--coding-agent-model claude-haiku-5-5 --reasoning-effort low`): intelligence 0.6×, cost 0.08×, time 0.4×
…
7. Opus 5.5, medium (`--coding-agent-model claude-opus-5-5 --reasoning-effort medium`): intelligence 1.0×, cost 1.0×, time 1.0× [default, this thread]
```

