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
- `{"answer": "needs-input", "argument": "<name>"}` refuses for now, with
  cause `missing-input`. No surface offers hook arguments yet.

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

- **A person's first message**: an error toast with the reason. The thread
  shows no message.
- **An agent's new thread**: `run_thread` or `lucidos spawn-thread` returns,
  but the new thread never starts. Only `EventHookRefused` says why.
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
