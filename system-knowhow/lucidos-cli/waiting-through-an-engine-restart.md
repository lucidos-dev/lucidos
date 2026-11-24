---
name: Lucidos CLI (lucidos), Waiting through an engine restart
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Waiting through an engine restart

Engine requests retry refused or unreachable connections with exponential
backoff, from 100 ms up to 1 second, for 30 seconds by default.
This lets script triggers and app scripts ride out a normal engine restart.
Set `LUCIDOS_CLI_CONNECT_WAIT_SECS` to a nonnegative whole number of seconds
to override the window. `0` disables retries; unset or invalid values use
the default.

Retries apply to every HTTP method, including mutations, only when a network
error proves the connection was never established. HTTP error statuses, TLS
failures, response timeouts and interrupted responses are never retried.
Engine requests do not follow redirects or use protocol-level automatic retries.
Streaming request bodies that cannot be cloned are sent only once.

Connection and response deadlines remain specific to each command.
An in-flight attempt can outlast the retry window; no new attempt starts after it.
Best-effort build-slot announcements and Git diff refreshes do not retry.
When the window expires, the existing connection error reports the actual elapsed time.

## Never post to the engine API as the user, and never route around a tool

Read this before anything below it. Three rules, and the third matters most.

1. **Never fabricate a human turn.** A message you author is an *agent* message.
   Never POST `mode: "human"` to `/api/v1/chat/stream` (or anywhere else). A turn
   recorded as the user looks, in the timeline and the event log, exactly like
   something they typed. It also sets the thread's initiator to the user and
   bumps the drawer's recency sort. The engine refuses a human claim it has no
   evidence for, but the rule binds you whether or not the engine catches you.

2. **Never hand-roll HTTP to the engine to get past a restriction.** If a tool
   or a CLI subcommand will not let you do something, that is the answer. Do not
   reach for `curl`, `urllib` or `fetch` against the engine's own `/api/v1`
   surface to do it another way. Use the CLI (it forwards the attribution
   headers and resolves the right engine) and stop where it stops.

3. **When a tool refuses you, TELL THE USER IT IS NOT POSSIBLE.** An agent once
   curled the engine to message threads that were not its children. Six agent
   messages were recorded as the user's, on another workspace's engine, as six
   phantom threads.

   The honest turn is short: say what you cannot do, say why, and offer what you
   can. Name the threads and let the user send the message. A refusal reported
   plainly is a good turn. A refusal worked around is a broken one, however well
   it seems to succeed.

What a failed request looks like:

- **403** on a `mode: "human"` POST means the request carried no registered
  device, so the engine will not record it as the user. Do not try to acquire
  one.
- **404** on a `thread_id` means no such thread exists *on the engine you
  reached*. It is no longer created for you. Check `GET /api/v1/health`, which
  names the workspace the answering engine serves.
- **409** naming a different workspace means you reached the wrong engine. The
  body names the right one. One machine runs one engine per workspace, each on
  its own port, so never guess a port. The CLI resolves the target, and
  `$LUCIDOS_API_BASE_URL` is the base for this workspace's engine.

The CLI asserts its workspace on every request (`x-lucidos-target-workspace`,
from `$LUCIDOS_WORKSPACE`), which makes the 409 possible.
`lucidos spawn-thread --to <ws>` asserts its given target instead. Hand-rolled
HTTP asserts nothing, and whichever engine holds the port serves it.

Some subcommands are hidden and engine-internal. They appear here only so the
CLI surface is complete. Scripts and users do not invoke them.

## When to use this

- **Python scripts** (`apps/<name>/scripts/*.py`, `triggers/<name>/scripts/*.py`, `knowhow/*/scripts/*.py`): call `subprocess.run(['lucidos', 'data', 'write', ...])`. The CLI is on PATH and `LUCIDOS_WORKSPACE` is set. Runtime state the script rewrites on every run (a cursor, a last-seen id) is the exception: write it directly, as `system-knowhow/plugins/authoring-a-marketplace.md` § "Where a plugin keeps its runtime state" shows.
- **Bash scripts** (same locations, `*.sh`): call `lucidos` directly. PATH is set before the script runs.
- **Coding-agent subprocesses**: these run in a worktree under `<workspace>/.lucidos/worktrees/<id>/`. Editor writes land in the worktree, not the workspace, so dev-server links 404. Use `lucidos data write` instead.

App UIs in the browser use the JS SDK (`lucidos.data.*`, `lucidos.events.*`). The CLI is for shell and subprocess contexts only.

**A fact worth keeping goes in `data/`, never in the agent's own memory directory.** Claude Code writes per-user memory under `$CLAUDE_CONFIG_DIR/projects/<cwd>/memory/`, outside both the worktree and the workspace. The engine never indexes it, and Codex, the user and the next session on another machine never read it. Write the fact to `knowhow/<topic>/`, or the relevant app's or plugin's knowhow, with `lucidos data write`. In the Lucidos repo itself, a convention for coding agents goes in `CLAUDE.md` or `.claude/rules/`, committed with the change.

The CLI embeds the coding-agent effort menus from `crates/lucidos-core/src/llm/`.
Its data path tests check the prefix list in `crates/lucidos-core/src/core/data_prefixes.rs`.

## Subcommands

### `lucidos help`

Read help for the CLI or any command path. `lucidos help events query` and
`lucidos events help query` describe the same command as `lucidos events query --help`.
Every command group provides this generated help route. It mirrors the
subcommands documented below and performs no action.

### `lucidos workspaces`

Workspace discovery: this machine's listing, routed targets, the gateway's cleanup review, and the
*workspace broker*'s verbs (ADR 0671). Every verb asks this workspace's engine,
which holds the local token and asks the gateway. So each one works inside the
agent sandbox, and the token never reaches you.

### `lucidos workspaces list [--json]`

Print this machine's workspaces, one per line, tab-separated: slug, display
name, port, health, autostart (`on` or `off`) and origin. Health is `healthy`,
`booting`, `restoring` or `unhealthy`; a stopped workspace reads `unhealthy`.
Origin is who made it: `user`, `agent`, `demo`, `eval`, `e2e`, or `unknown`
for a workspace made before the gateway recorded it. `--json` prints the rows
with `path` and `machine_made` too. Over `GET /api/v1/workspaces`; needs no
approval.

```text
demo-rig	Demo rig	5180	healthy	off	agent
dev	Development	5173	healthy	on	user
```

### `lucidos workspaces targets [--via <gateway>]`

Print the source gateway's routing directory as JSON: local workspace names,
outgoing gateway names, and available return routes. With `--via`, ask that
routing gateway for its directory. Use the reported gateway id when labels are
ambiguous. Discovery reveals no pairing credentials or workspace filesystem
paths and changes no preferences.

Routed commands keep the calling thread's source-engine environment. Do not
point `LUCIDOS_API_BASE_URL` at another engine or copy the origin token there.
The source engine verifies the thread and sends a signed operation; the token
stays at the source. Pairing alone gives no return access. The routing gateway's
owner enables it under Settings → Access → Remote gateways, as described in
`system-knowhow/remote-access/settings-access-2.md` § Agent return access.

A refusal for missing return access or a missing pairing names the Settings
step the user takes. Ask one question that quotes that step, with two options:
`I enabled it, retry` and `No`. Retry the same command on the first answer.
Offer no workarounds until the user says no. Other refusals have no switch to
enable, so report them as they are.

`--via` selects the routing gateway. `--to` selects one of its local workspaces,
or `workspace@remote` through one of its outgoing pairings. Omitting `--via`
uses the source gateway. Names and thread links identify targets; they grant
no authority. These read-only commands print JSON on stdout. Event queries
also print the selected thread's liveness on stderr when available.

```bash
lucidos workspaces targets
lucidos workspaces targets --via "Desktop gateway"
lucidos threads get --via "Desktop gateway" --to research@cloud --thread THREAD_ID
lucidos threads messages --via "Desktop gateway" --to research@cloud --thread THREAD_ID
lucidos threads liveness --via "Desktop gateway" --to research@cloud --thread THREAD_ID
lucidos events query --via "Desktop gateway" --to research@cloud --thread-id THREAD_ID --limit 20
lucidos intelligence-ladder --via "Desktop gateway" --to research@cloud --thread THREAD_ID --json
```

The same selectors support `threads list`, `threads count`, `events count`,
and a new top-level `spawn-thread`. A routed spawn uses the source thread's
spawn permission and always starts a new thread. Its JSON result reports
`thread_id`, `via`, `to`, `title` and `remote_start`. `remote_start` says
`Started.`, or that the thread waits for the target gateway's owner to allow
it (`system-knowhow/remote-access/settings-access-2.md` § Remote start setting). Preserve those selectors for subsequent
reads; browser thread links use a different gateway namespace. Child callbacks and integrated sub-threads remain local.

Other mutations
are refused before sending a routed request. Missing protocol support names
the gateway or engine to update. A failed or uncertain spawn is never retried
automatically; inspect its reported thread id through the same route first.

### The broker verbs

Individual broker calls take a `--reason`. Who decides depends on who made the
workspace (ADR 0676):

- **A machine-made workspace is yours.** `stop`, `start`, `autostart` and
  `rename` on one act at once, with no card. Each is recorded on your thread as
  `MachineMadeWorkspaceChanged`.
- **Every other call needs a live permission**: any verb on a `user` or
  `unknown` workspace, and `create`, `delete`, `restore` and `device-session` always.

A call that asks needs a live *permission* for that verb. With none, the call
records a *permission request*, prints its id and how to ask with it, and
exits 2. Ask `AskUserQuestion` with the id as the only question. Once the user
allows it, run the same command again, in this turn. The permission is spent
before the gateway call. A refusal explicitly reporting no change restores that
Allow once; a timeout or unknown outcome leaves it spent.

The card offers the scopes the verb allows (ADR 0688):

| Verb | Allow once | Allow for this thread | Always allow |
|---|---|---|---|
| `create` | the asked slug | any slug | any slug |
| `stop`, `start`, `autostart`, `rename` | that workspace | that workspace | that workspace |
| `delete`, `restore` | that exact target | no | no |

Allow once is spent by one call and ends at your thread's next turn start.
Allow for this thread lasts while the thread does. Always allow covers every
thread until the user revokes it (§ "`lucidos permissions`"). So a film rig's
creates need one Always allow tap, not one card per take.

```bash
$ lucidos workspaces stop dev --reason "Free the port for the rig."
This needs the owner's permission. Requested permission-request:<uuid>. ...
# ask AskUserQuestion with `permission-request:<uuid>`; once allowed:
$ lucidos workspaces stop dev --reason "Free the port for the rig."
# demo-rig is machine-made, so this acts at once:
$ lucidos workspaces stop demo-rig --reason "Reset the rig between takes."
```

#### `lucidos workspaces delete <slug>…`

Prepare one permission card listing the local workspaces to move to Trash.
Pass the printed request id as the only question to your question tool.
On **Allow once**, Lucidos runs the batch. The broker refuses the agent's
own workspace, missing workspaces, and workspaces of origin `user` or
`unknown`. See the batch receipt for
the result. The single-workspace form with `--reason` uses the individual
broker flow above.

#### `lucidos workspaces device-session`

`device-session <slug> --device <device-id> --reason <R> --out <file> [--ttl <minutes>]`
grants a direct-port page a session device identity.
Allow once binds one workspace, device and lifetime. When the gateway records
that this source workspace created the target, the card also offers Allow for
this thread and Always allow. Those cover the same device and exact requested
lifetime on current and future machine-made targets created by threads in this
source workspace. Other threads here may use Always allow; other source
workspaces cannot.

A different lifetime, even shorter, asks again. Missing or
foreign creation provenance leaves Allow once. Revoke lasting consent with
`lucidos permissions revoke`; restoring a snapshot ends earlier grants.
The gateway revalidates creation provenance under its operation lock for every
mint using lasting consent. No global device-session grant is offered.

Only machine-made workspaces with a gateway stamp key qualify.
User and unknown workspaces, stopped workspaces without a stamp key, and ids
belonging to a paired device are refused before a card is raised. A paired id
is refused with `device_session_owner_device`; choose a distinct id for the page.
Session ids cannot contain `:`, which is reserved for relayed device identities.
The gateway rechecks at mint time and refuses if its device directory is unavailable.

The lifetime defaults to 120 minutes, with a maximum of 240.
The credential goes only to `--out`, written atomically with mode 0600.
Stdout prints its expiry, never the credential.
The rig reads the file and supplies `x-lucidos-device-session` as a browser-context extra header.
The page still sends its device id; its own script need not read the credential.

Sends carry the standard device origin, and widget fit reports resolve for that device.
Requests with a session header cannot reveal stored secrets.
Mint again after an engine restart or gateway re-key, since the old signature then fails.

#### `lucidos workspaces create` / `stop` / `start` / `autostart` / `rename` / `delete` / `trash` / `restore` / `classify`

| Command | Does | On |
|---|---|---|
| `create <name> [--slug <slug>] [--autostart] --reason <R>` | Creates a workspace | a new one |
| `stop <slug> --reason <R>` | Stops its engine | any workspace but your own |
| `start <slug> --reason <R>` | Starts its engine | any workspace |
| `autostart <slug> --on\|--off --reason <R>` | Sets whether it starts with the gateway | any workspace |
| `rename <slug> <name> --reason <R>` | Changes the display name; the slug stays | any workspace |
| `delete <slug> --reason <R>` | Moves it to the Trash | a machine-made workspace, not your own |
| `trash` | Prints the Trash as JSON, each entry's `id` for `restore` | |
| `restore <trash-id> --reason <R>` | Restores it from the Trash | a machine-made entry with `restore_available` |
| `classify <slug>... --origin <O> --evidence <E>` | Raises a batch to record who made them | `unknown` workspaces only |

- **Create** derives the slug from the name when `--slug` is absent. A taken
  slug or name is refused with a 409, never suffixed, so the card names the
  workspace that gets made. A broker-made workspace records origin
  `agent` and your thread, and starts with autostart **off** unless you pass
  `--autostart`.
- **A machine-made workspace** has origin `agent`, `demo`, `eval` or `e2e`.
  Delete and restore refuse a `user` or `unknown` one before any card. The
  owner deletes those from the picker.
- **Restore needs a database copy.** Deleting a machine-made workspace keeps
  none, so its Trash entry reads `restore_available: false` and restore
  refuses it with a 409 before any card.
- **No restart.** There is no restart verb, and `POST
  /api/v1/workspaces/<slug>/restart` answers 405. Stop the workspace, then
  start it.
- **Classify** groups workspaces by the origin you propose: `agent`, `demo`,
  `eval`, `e2e` or `user`. `--evidence` is what shows it beyond the name, such
  as a rig receipt. The owner reads it, quoted as yours, on one batch card with
  one line per group, and only **Run all** records it. A workspace whose
  origin is no longer `unknown` by then is skipped. It is a `lucidos
  permissions batch` item underneath (§ "`lucidos permissions batch`").

  The command prints a prepared request id and the question-tool instruction;
  ask with that id alone. On Run all, Lucidos records the classification.
  Do not run the classify command again after approval.
- **Refusals** show only the engine's error text. A caller with no
  thread-bound origin token gets a 403: a terminal user uses the picker. A
  protected target gets a 403 too. A taken slug or name, or stopping or
  deleting your own workspace, gets a 409.

`lucidos ask-permission <action> --workspace <slug> --reason <R>` asks for
the same permission ahead of the call, with the action names
`create-workspace`, `stop-workspace`, `start-workspace`,
`set-workspace-autostart`, `rename-workspace`, `delete-workspace` and
`restore-workspace`. Ask ahead only for a call that asks: a machine-made
workspace's stop, start, autostart and rename never need one.

Routes, all under `/api/v1`: `POST /workspaces` (`{name, slug?, autostart?,
reason}`), `POST /workspaces/<slug>/{stop,start,delete}` (`{reason}`),
`/autostart` (`{enabled, reason}`), `/rename` (`{name, reason}`), `GET
/workspaces/trash`, and `POST /workspaces/trash/<id>/restore` (`{reason}`).

### `lucidos workspaces review [--port <port>]`

Print the gateway's cleanup review as JSON, including registered workspaces and
discovered cleanup candidates. Requires the machine-local token. When several
gateways are running, pass `--port` to choose one; their registries stay separate.
Inside the agent sandbox the token is hidden, so the command fails there.

The review is a cached observation. If `scanned_at` is null, the initial scan is
pending: empty rows do not mean no workspaces exist. The CLI prints that warning
to stderr. Run it again after the scan finishes. `refreshing` indicates a scan
is still running; unknown sizes or failed observations never authorize deletion.
This command does not freeze or confirm a batch and does not mutate workspaces.

### Hidden: `lucidos coding-agent-diff-hook`

A git `post-commit` hook in Lucidos-managed coding-agent worktrees. When the
process has `LUCIDOS_THREAD_ID` set, it posts the repo root and branch to the
parent engine. The engine then moves the thread's `coding_agent_change_state` between
`none` and `unproposed` right after a commit. It is silent and best-effort. It
creates no `ChangeProposed` event and no Apply-able change: the formal proposal
still happens when the coding-agent turn idles.

Do not call it from scripts. It needs the subprocess-origin headers the CLI
attaches in spawned subprocesses.

### Hidden: `lucidos merge-driver`

A git merge driver, run by git for a path whose `.gitattributes` line names
`merge=lucidos-sorted-lines` or `merge=lucidos-blocks`.
Git passes the base, ours and theirs files. The driver writes the merge over
ours and exits 0 for a clean merge or 1 for a conflict.

- `sorted-lines` merges a list of numbered `- ` lines by number and keeps it
  sorted.
- `blocks` merges markdown by heading and top-level bullet, so two entries
  added at one spot both stay. An entry both sides edited gets git's line
  merge.

Exit 2 means it did not merge, for example a file it cannot key. The
registered command then merges the path as plain text. Nothing registers these
drivers except a repository's own setup, so the subcommand is inert elsewhere.

