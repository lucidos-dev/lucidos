---
name: Event Hooks
description: Load to write, arm or debug an event hook: "stop X until Y", "block Apply until tests pass", "why was my message refused". Covers asked events, hook files, answers and refusals.
---

<!-- domain-event: TicketLinked: Example domain event an event hook returns beside the message it allowed. -->

# Event Hooks

An *event hook* is a script Lucidos asks before it records an event. The hook
allows the event, or refuses it and so stops that progress. Like a trigger, a
hook is workspace content: you write it under `data/hooks/<slug>/`, and only
the owner arms it. The terms are in the glossary: *event hook*, *hook
definition* and *hook answer*.

A hook stops an act that has not happened yet. To react after an event, use a
trigger instead (`system-knowhow/triggers`).

## The events Lucidos asks about

Lucidos asks hooks about these four events only. A hook on any other type
cannot be armed.

| Event | When Lucidos asks | A refusal stops |
|---|---|---|
| `MessageReceived` | A thread's first message only. The hook reads `"first_message": true` in the payload. A follow-up message is recorded without an ask. | The thread's start. The message is not recorded, and no turn runs. |
| `TriggerStarted` | A trigger fire, before its thread starts. | The fire. It fails, and the scheduler sends its failure notice. |
| `ChangeProposed` | A coding-agent turn end that would propose a change. | The proposal. The work stays unproposed with reason `hook_refused`, and the next turn end asks again. |
| `ChangeApplyRequested` | An Apply of a pending change. | The Apply. It fails with the hook's reason, and nothing merges. |

Two `ChangeProposed` records are never asked, because stopping them would hide
the user's work: leftover work set aside after a thread ends, and a correction
to an existing change.

A turn end whose proposal matches the open change records nothing, so no hook
is asked. That holds for the same description, files and flags over new
commits. The open change carries those commits, so hook `ChangeApplyRequested`
to check what lands.

## The hook folder

```text
data/hooks/require-a-ticket/
  hook.toml
  check.py
```

`hook.toml` holds `name`, `event_type`, an optional `condition`, `script` and
an optional `timeout_secs` (10 by default, at most 60). Unknown keys make the
hook invalid. `lucidos hooks list` shows every problem at once.

```toml
name = "Require a ticket"
event_type = "MessageReceived"
condition = { first_message = true }
script = "check.py"
timeout_secs = 5
```

The `condition` uses the trigger condition syntax, over the payload the
script reads. Lucidos checks it before it starts the script, so a hook on a
busy event costs a script start only when the event matches.

**Arming binds every file in the folder.** A helper the script imports is
content too. After any edit in the folder, an armed hook is *changed*. A
changed hook refuses its events until the owner arms the new content or
disarms it. A link anywhere in the folder makes the hook invalid.

**A hook keeps no state in its folder.** Each ask copies the folder, checks
the copy against the arming, and runs the copy. The agent sandbox makes that
copy read-only, so a write beside the script fails, and Lucidos deletes the
copy after the ask. To remember something, return a domain event and read it
back later with `lucidos events query`.

## Hooks from plugins

A plugin ships its definitions in `hooks/<slug>/hook.toml`. The install
panel lists the hooks and their event types under **Can stop progress**.
Only the owner can confirm a plugin install that includes hooks.

An update asks again when it adds a hook or changes its event type.
A hook on the same event keeps the owner's arming choice. If the update
receipt asks you to review a hook, open **Settings > Event Hooks** and review
its installed content before arming it. The list shows which plugin owns
the hook. Uninstall the plugin to remove its hooks and end their armings.

## Declared arguments

A hook on `MessageReceived` can offer choices before the first message.
Each argument has a kebab-case name, a label and a script inside the hook
folder.

```toml
[arguments.ticket]
label = "Ticket"
choices_script = "choices.py"
```

The choices script reads one JSON object on stdin. It receives `hook`,
`argument`, `query` and `limit`. An empty `query` asks for the first page.
Return at most `limit` choices as a JSON array. Each row has a unique string
`value` and a display `label`.

```json
[{"value": "TK-42", "label": "Login timeout"}]
```

The script can read the workspace projection through the read-only
`LUCIDOS_DATABASE_URL`. It runs only for an armed hook that still matches
its content hash. It needs a sandbox that confines writes. Create or link
records in the hook's answer to the first message.

| Surface | How to supply an argument |
|---|---|
| Composer | Open the labeled picker and search for a value. If choices are unavailable, type a known value and select it. A saved draft keeps the selection. |
| CLI | Read values with `lucidos hooks choices --slug <slug> --argument ticket --query login`. Pass `--hook-arg ticket=TK-42` on `lucidos spawn-thread`. Repeat the flag for each argument. |
| Agent tools | Pass `hook_args: {"ticket": "TK-42"}` to `run_thread` or `run_coding_agent`. Use the declarations from the target workspace. |
| Queue recovery | `ThreadQueued`, `OrdinarySubmissionAccepted` and `DraftSubmissionAccepted` keep supplied values under `request.hook_args`. |

The hook receives the supplied values it declares under `arguments` on
stdin. Treat each value as untrusted input and verify it against the
projection. A picker is optional. The hook decides what to do if a value is
missing or stale.

## What the script reads

The script runs as a trigger script does, under the agent sandbox, with no
credential in its environment. It reads one JSON object on stdin:

```json
{
  "hook": "require-a-ticket",
  "event_type": "MessageReceived",
  "payload": {
    "text": "Fix the login timeout, ABC-123",
    "mode": "human",
    "origin": { "kind": "device", "device_id": "<id>" },
    "thread_id": "<uuid>",
    "first_message": true
  }
}
```

`payload` is what the event store would record, plus the `thread_id`. Every
stored secret is masked out of it. `origin`, or `actor` on the change events,
says who caused the event, so the hook decides itself whether engine-started
work counts.

For `ChangeProposed` and `ChangeApplyRequested`, the script also reads
`payload.change_context`. Lucidos adds it after it checks the hook's
condition. The event store keeps the original payload.

| Field | Value |
|---|---|
| `coding_agent_kind` | The thread's repository kind. |
| `landing_files` | The proposed files, or the current committed and uncommitted files at the Apply request. `null` means Git could not give a list. |
| `plan_marker.state` | `present`, `missing`, `changed`, or `unreadable`. |
| `plan_marker.kind` | The marker kind if present, such as `planned` or `plan_only`. |
| `plan_marker.file_bound` | The files named by a bounded security fix, or `null`. |

The Apply file list describes the branch when the hook runs. Treat an
unknown list as unknown, and check again if the candidate changes.

The script may run the `lucidos` CLI. Lucidos records anything it emits as the
hook's own, with actor `event_hook`, and never asks a hook about it.

## What the script answers

It prints one JSON object on stdout, and exits 0:

- `{"answer": "allow"}`, with an optional `events` list. Each entry is
  `{"event_type": "<DomainEvent>", "payload": {...}}`. Lucidos records them
  together with the hooked event, in one transaction, or records none of them.
  Name only your own domain events: an engine event name is a refusal.
  Include a `summary` in each payload, as `lucidos events emit` asks.
- `{"answer": "refuse", "reason": "<what to do>"}`. The person or agent that
  caused the event reads the reason, so say how to fix it.
- `{"answer": "needs-input", "argument": "<name>"}` asks for a declared
  argument on the first message. A person sees a picker beside the unsent
  message. An agent spawn returns `status: "needs-input"` with a
  `hook_input` object that names the hook, argument, label and top choices.
  The agent retries with `hook_args`. Cancelling the picker or asking when
  nobody can answer records a `missing-input` refusal.

Set `create_label = "Ticket title"` on an argument to offer inline
creation. The hook receives the person's new value under
`create_arguments`, keyed by the declared argument name. It should
validate the value and return the creation and link as domain events in
its `allow` answer. Lucidos records both with the first message in one
transaction. The choices script performs reads only.

Pass a creation value from an API caller as a `hook_args` entry keyed
by `hook_input.creation.key`. A pick uses the argument name as its key.
An armed hook receives a creation value only for its own argument
with a `create_label`.

The first ask shares one deadline across hooks. Routed spawns have a
shorter deadline so the refusal can reach the caller through the gateway.
While first-message hooks are armed, Lucidos refuses routed starts that
still require permission. Start the thread from the receiving workspace.

A hook fails closed. A crash, a run past `timeout_secs` and output that is no
answer each refuse the event. With several hooks on one event, Lucidos asks
them in slug order, and the first refusal wins.

No hook script starts before the engine answers requests, so a script may call
the engine. An event asked while the engine starts waits for that, and
`timeout_secs` counts from the script's start.

## What a refusal looks like

Every refusal records `EventHookRefused`. It names the hook's `slug`, the
stopped `event_type`, the `thread_id` and the `actor` who caused the event. It
also carries a `cause`, the `reason`, and a `detail`: the tail of a failed
script's stderr, secrets masked. The causes are `refused`, `crashed`,
`timed-out`, `invalid-answer`, `changed`, `missing-input` and `unavailable`.
Read them with `lucidos events query --type EventHookRefused`.

What the actor sees:

- **A person's first message**: the send shows the refusal reason.
- **An agent's new thread**: the spawn returns the reason. If it is a child
  thread, the parent also receives a child failure report.
- **A trigger fire**: it fails, and the failure notice names the hook.
- **A proposal**: the thread's not-ready strip reads "Not ready: an event hook
  stopped it".
- **An Apply**: it fails, and the API answers `400` with the hook's reason.

## Arming

The owner arms a hook in Settings > Event Hooks, or with
`lucidos hooks arm --slug <slug>`. An agent can ask: its arm needs a
`--reason` and the owner's Allow once. No hook can stop arming, disarming or
removing a hook. The commands are in `system-knowhow/lucidos-cli/subcommands-9.md`
§ `lucidos hooks list | arm | disarm | remove`.

Test the script before you arm it. Pipe a sample object into it, and check
that it prints an answer:

```bash
echo '{"hook":"require-a-ticket","event_type":"MessageReceived","payload":{"text":"hi","first_message":true}}' \
  | python3 data/hooks/require-a-ticket/check.py
```

## Worked example: every new thread names a ticket

`check.py`, beside the `hook.toml` above:

```python
import json
import re
import sys

event = json.load(sys.stdin)
payload = event["payload"]
match = re.search(r"\b[A-Z]+-\d+\b", payload.get("text", ""))
if match:
    print(json.dumps({
        "answer": "allow",
        "events": [{
            "event_type": "TicketLinked",
            "payload": {
                "summary": f"Thread linked to {match.group(0)}",
                "ticket": match.group(0),
                "thread_id": payload["thread_id"],
            },
        }],
    }))
else:
    print(json.dumps({
        "answer": "refuse",
        "reason": "Name a ticket, such as ABC-123, in the first message.",
    }))
```

`TicketLinked` is recorded only when the thread starts, so a trigger or an app
can trust it.

## Questions to settle with the user before creating

- Which act should stop: a new thread, a trigger fire, a proposal or an Apply?
- Which ones does it cover? That becomes the `condition`: every new thread, or
  only coding-agent ones, or only one trigger's fires.
- What must be true to let it pass, and where does the script read that?
- What should the refusal tell the person or agent to do?
- Should an allowed event record a domain event beside it?
- The owner arms it. Tell them where, and that an edit needs a new arm.

## Common mistakes

- **A hook on any other event.** Lucidos asks only about the four events
  above. To act after any other event, write a trigger.
- **A refusal reason that names no fix.** The actor cannot see the hook. Say
  what to change.
- **State in the hook folder.** The script runs from a read-only copy that
  Lucidos deletes after the ask.
- **Editing an armed hook and expecting the edit to run.** It runs nothing:
  the hook refuses its events until the owner arms it again.
- **A slow script.** Every matching event waits for it. Keep it under a few
  seconds, and narrow the `condition`.

### Shadow checks for changes

Proposal and Apply shadow checks let an armed hook observe the candidate
a built-in plan check evaluated. They run for held and clear candidates,
including proposals evaluated during recovery. The request
includes `thread_id` and `change_context`. Paths that cannot be passed
unchanged through secret masking cause an unavailable verdict.

Shadow replies are diagnostic. They neither gate a change nor append
events. The engine logs both decisions and the armed hook content hash
under `[ChangeHookShadow]`. An unavailable verdict does not count as a
match.
