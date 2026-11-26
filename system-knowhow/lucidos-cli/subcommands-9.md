---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos permissions batch --items '<json>'`

Raise a *batch* on your own thread: one card that the owner confirms once. It
answers open cards in your sub-threads, at any depth, and widens triggers'
grants. Use it when several sub-threads wait on the owner, or when the owner
asks you to grant several triggers. They then answer once, not one by one.

```bash
$ lucidos permissions batch --items '[
    {"kind": "card_answer", "thread_id": "<sub-thread id>", "option": "Approve"},
    {"kind": "card_answer", "thread_id": "<sub-thread id>", "option": "Left",
     "tool_use_id": "<card id>"},
    {"kind": "trigger_grant", "trigger": "daily-digest",
     "side_effects": ["email"], "coding_agents": true}
  ]'
```

A `card_answer` item names a sub-thread and the label of the option to pick.
Add `tool_use_id` only when that thread has more than one open card. For a
tool-permission card (a command, tool or MCP tool card), pass its
`request_id` there. Such a card takes **Allow once**, **Allow for this
thread** or **Deny**, and Lucidos answers it as the card's own button would.
A *permission request* card takes **Allow once**, **Allow for this thread**
where its menu offers it, or **Don't allow**. A batch never picks **Always
allow**.

A `trigger_grant` item names a trigger by id or slug. `side_effects` lists the
categories to allow, spelled as the trigger's grant spells them: `email`,
`external_api`, `cloud_cli`, `out_of_workspace_destruction` or `other`.
`coding_agents: true` lets its runs start coding agents. The item adds only
what the trigger lacks, and never takes a grant away.

A `workspace_classification` item records who made workspaces of origin
`unknown`: `{"kind": "workspace_classification", "workspaces": ["<slug>"],
"origin": "demo", "evidence": "<what shows it>"}`. `lucidos workspaces
classify` raises one for you (§ "The broker verbs").

A `workspace_deletion` item asks to move machine-made workspaces to Trash:
`{"kind": "workspace_deletion", "workspaces": ["<slug>"]}`.
`lucidos workspaces delete <slug>…` raises one and names every target on the
card. Lucidos checks the targets when it runs and reports any refusals.

The HTTP answer is `{ "request_id": "permission-request:<uuid>" }`.
The CLI prints that id and a two-step instruction. Call your question tool
(`AskUserQuestion`, or Codex `ask_user_question`) with exactly that id as the
only question text and any two options. Lucidos replaces it with its own card.

On **Run all** (or **Allow once** for a deletion-only batch), Lucidos executes
the batch once; do not rerun the command.
**Don't run** executes nothing. Preparing alone shows no card.

If the id names an engine card already on screen, end your turn and let the
owner answer it. An unknown id requires a fresh `lucidos permissions batch`
(or `lucidos ask-permission` for one act).

Lucidos words the card itself, one line per item. A card answer names the
thread, the question and the pick. A permission request line names the act,
its target and the scope from the request itself, never from your question. A trigger grant names the trigger, the first
12 characters of its *definition digest*, what it runs, the grants and its MCP
servers. The owner picks **Run all** or **Don't run**. Only the owner's pick
runs it, once.

Lucidos then checks each item again, and skips it when any of these changed:

- the card closed, or its options changed;
- the plan changed;
- the trigger was deleted, or its definition digest changed.

Each item gets a `BatchItemRan` receipt on your thread, which says why it was
skipped.

An approving pick on a thread with a proposed plan binds the committed plan
text. `lucidos planned approve` there accepts that pick only while the text is
unchanged. A revised or re-marked plan needs a new yes.

A batch is refused whole, with the item number and the reason, when an item:

- is a card on your own thread, or on a thread not below yours;
- is a batch card, or a card for a caller Lucidos cannot verify;
- picks an Always allow, which stays the owner's own tap on the card;
- takes several picks, or names an option the card does not offer;
- repeats a card or a trigger the batch already covers;
- names a secret read, which is never grantable, or an unknown category;
- names a trigger that does not exist, or grants nothing it lacks.

A batch carries at most 20 items. Raising one grants nothing: the owner's
confirm is the only thing that runs it.

### `lucidos pinned-apps list --device-id <ID> | pin | unpin --app-id <ID> --device-id <ID>`

Read and change the apps one device has pinned. Take the device id from
`lucidos devices list` and the app id from `lucidos apps list`.

```bash
$ lucidos pinned-apps list --device-id <device-id>
$ lucidos pinned-apps pin --app-id habit-tracker --device-id <device-id>
$ lucidos pinned-apps unpin --app-id habit-tracker --device-id <device-id>
```

`pin` emits `PinnedAppPinned` and `unpin` emits `PinnedAppUnpinned`, so the
device's dashboard follows at once. Repeating either changes nothing and emits
nothing. A SQL write to `pinned_apps` is refused. There is no LLM tool: run it
through `run_bash`.

### `lucidos browser forget-login --domain <DOMAIN>`

Drop the record that the built-in browser is signed in to one domain. It is the
CLI form of the chat agent's `browser_forget_login` tool, and it emits no
event, because nothing displays that record.

```bash
$ lucidos browser forget-login --domain example.com
```

### `lucidos hooks list | arm | disarm | remove`

List every *event hook* under `data/hooks/` with its arming, and arm, disarm or
remove one. Only the owner does the last three, from Settings > Event Hooks or
this CLI.

```bash
$ lucidos hooks list
$ lucidos hooks arm --slug require-a-plan
$ lucidos hooks disarm --slug require-a-plan --reason "The plan rule moved."
$ lucidos hooks remove --slug require-a-plan
```

`list` prints one JSON object, shown formatted here:

```json
{
  "hooks": [
    {
      "status": "valid",
      "slug": "require-a-plan",
      "event_kind": "thread",
      "definition": {
        "name": "Require a plan",
        "event_type": "MessageReceived",
        "script": "check.py",
        "timeout_secs": 5
      },
      "content_hash": "<64 hex characters>",
      "arm_state": "disarmed",
      "armed": null,
      "arm_refusal": null,
      "remove_refusal": null
    }
  ]
}
```

A valid row carries `slug`, `event_kind` (`thread` or `system`), the
`definition` as written, and `content_hash`: the digest of every file in the
hook's folder, read with the definition. An invalid row has `status: "invalid"` and lists
every `problems` entry at once. It carries the `definition` too when the file
parsed. A file directly under `data/hooks/` lists as invalid, since a hook is
a folder. Names starting with `.` are skipped.

Each row also carries its arming:

| Field | Meaning |
|---|---|
| `arm_state` | `disarmed`, `armed`, or `changed`: armed, but a file in its folder changed since, or the hook broke or left |
| `armed` | The arming: `event_type`, `content_hash`, `armed_at` and `actor`, or `null` |
| `arm_refusal` | Why `arm` would be refused now, or `null` |
| `remove_refusal` | Why `remove` would be refused now, or `null` |

A changed hook's events are refused until the owner arms the new content or
disarms it, so an edit never switches a gate off. An armed hook whose folder
is gone lists with `status: "missing"`. `remove` ends its arming.

`arm` binds the hook's current content. It is refused for an invalid hook, an
armed one, or one whose `event_type` Lucidos never asks hooks about. It asks
about four: a thread's first `MessageReceived`, `TriggerStarted`,
`ChangeProposed` and `ChangeApplyRequested` (`system-knowhow/event-hooks`).
`disarm` keeps the folder.

`remove` deletes `data/hooks/<slug>/`, commits that, and ends any arming. A
link in the folder's place goes itself, never what it points at. An entry whose
name the routes cannot carry, or that cannot be opened, is refused: rename or
delete it outside Lucidos.

Settings sends the `content_hash` it showed with an arm, and the engine refuses
the arm when the hook hashes otherwise now. So the owner never arms content
they did not see.

An agent's `arm`, `disarm` or `remove` needs a `--reason` and the owner's
Allow once. The first call exits 2 and prints a request id. Ask the owner
with it, then run the same command again. An arm names the content it allowed,
so an edit in between asks again.

From a terminal outside Lucidos, the request proves nothing about who sent it.
The engine asks the owner on a card in Lucidos, and the command exits 2 with
"Nothing happened yet". The owner's Allow once runs it there, so do not run it
again.

A definition is invalid when:

- the folder name is not a kebab-case slug;
- `hook.toml` does not parse, or holds an unknown key;
- `event_type` names a transient event, a retired one, or nothing Lucidos
  records, which lists the near matches;
- `condition` could never match;
- `script` leaves the folder, is not a `.py` or `.sh` file, is missing, or
  cannot be read;
- the folder holds a link, or anything but plain files and folders;
- `timeout_secs` is outside 1 to 60. Leave it out for 10.

There is no chat tool for this. The Lucidos Agent runs the CLI from its shell
tool. An app frame cannot reach `/api/v1/hooks`, and an app opened in its own
tab is refused there, even with the owner's device.

### `lucidos mcp list | enable | disable | start | stop | remove`

Manage MCP servers: which are running, which threads use them, what tools they
offer, and what those tools cost in context.

```bash
$ lucidos mcp list
$ lucidos mcp enable --thread-id <uuid> --id slack
$ lucidos mcp disable --thread-id <uuid> --id slack
$ lucidos mcp start --id slack
$ lucidos mcp stop --id slack
$ lucidos mcp remove --id backstage
```

A running server's tools reach only the threads that enabled it (ADR 0430).
`enable` switches a server on for one thread, starting it if it is not running,
and wraps `POST /api/v1/threads/<uuid>/mcp-servers/<id>/enable`. `disable` wraps
the matching `/disable` route and switches it off in that thread only. `start`
from the CLI enables no thread. `stop` ends the process and switches the server
off in every thread, and so does `remove`.

`list` returns `servers`, `totals`, `model` and `context_window`. Every server
carries its tools with a `wire_name` (the name a call must use), `chars` and
`tokens`. The token figures are the engine's own estimate, the same one the
Context Viewer shows, so a script must never recompute them from chars.

`tools_source` says where the tool list came from. `live` means the process
answered just now. `cache` means the manifest observed at the last successful
start, and `tools_observed_at` stamps when. `never-observed` means the server
has never connected, so its tool list is unknown: that is NOT the same as a
server with no tools, and a script must not report it as costing nothing.

`totals` splits the cost a thread pays when it enables the servers. `tokens` is
what the running servers would add, `stopped_tokens` what the stopped ones
would add, and `disabled_tokens` what the switched-off tools would add back.
`budget` is the worst case: one thread that enables every server, after the MCP
share of the context. Divide by `context_window` for the share of the resolved
model's window. A thread that enables nothing pays none of it.

**Nothing starts MCP servers at boot.** A server is running only if something
started it in the current engine process, so `running` resets on every restart.
A thread keeps what it enabled, and its next turn starts those servers again.

A server whose id cannot ride a wire tool name reports `dispatchable: false`.
None of its tools can ever be called, `start` refuses it with a 422, and
`remove` is the only useful verb. Registering a server is the chat agent's `mcp`
tool, which takes the command and args this surface does not.

Switching one tool off or back on is `PATCH /api/v1/mcp/servers/<id>/disabled-tools`
with `{"wire_name": "<wire name>", "disabled": true}`. It answers with the stored
set. Each call names one tool, so two clients switching different tools both
land. No CLI flag for it.

### `lucidos changes list [--sub-threads-of <uuid> | --my-sub-threads]`

List pending, set-aside and recently-applied *changes*. Wraps `GET /api/v1/changes` and echoes the engine's payload verbatim to stdout. This is how a script finds a pending change's id before `apply`: read `.pending[].id`. Never scan `ChangeProposed` events for it.

```bash
$ lucidos changes list
{"pending":[{"id":"fbcc4a3a-...","branch_name":"lucidos-claude-code-repo-lucidos-fix-...","description":"fix: …","status":"pending",...}],"set_aside":[],"applied":[...],"total_pending":1,"restart_required":false,"restart_groups":[],"client_update_available":false,"has_more_applied":false}

# Find the single pending change's id (e.g. in a script that applies a pending change):
$ CID=$(lucidos changes list | jq -r '.pending[0].id')
$ lucidos changes apply "$CID"
```

The response carries `pending`, `set_aside` (kept for later, newest first), `applied` (recently applied), `total_pending`, and `restart_required`. Each pending change has `id` / `branch_name` / `description` / `status` / `file_count` / `requires_restart` / `thread_id` / `thread_title`. Its apply state is `apply_ready` / `needs_hardening` / `thread_unsettled` / `thread_settling` / `resolving_conflict` / `apply_phase_started_at` / `predicted_conflict`. Exit non-zero on transport / HTTP error.

`predicted_conflict` is `conflict`, `clean` or `unknown`: would merging the change into `main` right now conflict. `unknown` means git could not answer, so never read it as clean. `apply_phase_started_at` is when a running hardening or conflict resolution began.

**`thread_unsettled: true` means the proposing thread is still working on the change**: mid-turn, on a question card, resolving a merge conflict, or watching an event. `apply` refuses it for exactly that reason, so never report such a change as finished. `thread_settling` is the part of that a *standing apply* can wait out. `resolving_conflict` means an apply of it is merging now.

**`apply_ready: true` means Apply merges the change as it stands**: its thread has finished, and it needs no hardening. It is exactly the Changes panel's **Ready** section, and what Apply All and Discard All act on. `needs_hardening: true` means Apply would run `/harden` first. A branch that carries only implementation plans never needs it.

Narrow `pending` to one subtree:

```bash
# Changes held by any sub-thread of one thread, at any depth (not its own):
$ lucidos changes list --sub-threads-of 6fa459ea-ee8a-3ca4-894e-db77e160355e
# The same, for the thread this subprocess runs in:
$ lucidos changes list --my-sub-threads
```

`--my-sub-threads` reads `LUCIDOS_THREAD_ID`, like `threads list --my-children`, so it works only from inside a Lucidos thread. `total_pending` counts the narrowed list.

> **In-thread agent:** the chat Lucidos Agent has the `changes` tool. Its `list` action returns the same `{pending, set_aside, applied, total_pending}` shape **in-process**, with the same flags filled. Its `sub_threads_of` argument takes a thread id, or `current` for its own thread. Use it from a chat or trigger thread. Use this CLI from a `script:`-typed trigger or a bash or Python subprocess.

