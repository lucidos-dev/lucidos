---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### Top-thread
A spawn with `relation: "top"` (the CLI default for `lucidos spawn-thread`). It has no parent and no callback wiring, so it appears in the main thread list as an independent thread. The *spawning thread* is **not** resumed when it finishes. It still records WHO launched it: the route popover on its first message names and links the spawning thread. "No parent" is about the callback, not provenance.

**No parent means the workspace, and two top-threads are siblings.** Every top-thread sits directly under the workspace root (ADR 0168 clause 1). The workspace is a container, never a place work runs, so nothing holds a turn there or can be delegated to it. Neither sibling has standing over the other: a top-thread reaches its own descendants on its own authority, and anything wider is the *workspace owner*'s button. Creating one is not: any thread may spawn a top-thread (ADR 0475).

### Standing instruction
What lets a *thread* press one of the *workspace owner*'s buttons while the owner is not watching. A thread acts inside its own subtree on its own authority; anything wider needs this. Two shapes qualify and no third.

**A turn the owner opened.** They spoke into the thread from one of their own devices, and their words in that turn are the press. Lucidos may resume the turn after a restart, an API error or a hang. That is the same turn carrying on, so it keeps the instruction.

**A *trigger* firing the owner authorized.** The same decision, made in advance: they wrote the trigger, or switched it on. A trigger an agent wrote and fired carries nothing, or any thread could hand itself the owner's authority.

Nothing is inherited. A thread you spawn opens its own turn, so it carries none of yours. A thread without one can still ask you for a *permission* through a *permission request*. No standing instruction answers a card: a thread relays an answer through a *batch* the owner confirms. See `system-knowhow/orchestrating-sub-threads.md` for what this permits and refuses.

### Owner approval
Retired (ADR 0688). The single-use **Allow once** card a thread asked for to act outside its own subtree is now a *permission request* with its action's scopes. The agent asks with `lucidos ask-permission`. Use *permission request* for the card and *permission* for what your answer grants.

### Permission request
A request Lucidos writes itself, asking you to allow one *action*, with options it words. The UI shows it as a permission card. Every way you answer picks one of those options: a tap, or spoken words on a call when the card supports voice. Picking the option that refuses grants nothing.

**A thread asks for one when it needs your say outside its own reach.** Examples are applying another thread's change, creating a workspace, or submitting one of your saved *drafts*. The agent asks with `lucidos ask-permission`, and its reason shows below as the agent's. Each action offers its own scopes. Creating a workspace can be allowed once, for this thread or always. Deleting a workspace, discarding a change or sending a draft is Allow once only, because it destroys work or sends your words.

A permission request Lucidos raised stays open until you answer it, whatever the agent does meanwhile. While it is open, a message you type waits behind it, and its row reads "Queued until you answer the open card" (ADR 0803).

Words count only when a judgment confirms they choose that option. With no model to judge, or an unsure judgment, they grant nothing, and a tap still works. Spoken words count only from your own transcript on a call your device started, never from the voice agent's summary. Lucidos reads a wide grant back to you before your spoken yes counts.
See also: *permission*, *batch*.

### Batch
One *permission request* that carries several items at once: question cards, permission request cards, tool permission cards, plan approvals and trigger grants. A thread gathers them from the *sub-threads* below it, and Lucidos words the batch itself. One confirm from you runs each item once. Lucidos checks each item again as it runs: an item whose card closed, whose plan changed, or whose trigger changed is skipped and named. A batch never holds another batch, or a card of the thread that raised it. It never picks an Always allow: that stays your own tap on the card.

### Permission
What your answer to a *permission request* grants. It names an *action* and how wide it reaches, such as one command or every command with that head. It also says how long it lasts and who holds it. It lasts once, for this thread, until the plan lands, or until you revoke it. A thread, a *trigger* or the whole workspace can hold one.

A tool card's "Always allow" is one the workspace holds, and its "Allow for this thread" one the thread holds. Both survive a restart.

### Action
One thing Lucidos can do that a *permission* can cover, from one fixed list in the code. Examples are Apply, Archive, run a command, grant a trigger its folders, and continue hardening. A button, a tool call, a CLI command and your voice name the same action. So anything you can tap, you can also ask for.

<!--gloss-trigger-start-->
### Trigger
A workspace configuration that fires on a schedule (`run.cron`) or on one of its *event subscriptions* (`on`). The `run` takes one of two shapes:
- `run.type: "intent"`: spawns a *trigger thread* whose LLM gets `run.intent` (the user's voice, non-technical prose) as a user message. It finds the knowhow it needs via `load_knowhow` at fire time; there is no per-trigger knowhow allowlist.
- `run.type: "script"`: runs the *script* at `run.path` directly, no LLM. On event fires the engine sets `TRIGGER_EVENT_TYPE` / `TRIGGER_EVENT_PAYLOAD` / `TRIGGER_EVENT_ID` / `TRIGGER_EVENT_THREAD_ID`. So the script can branch deterministically and deep-link a notification to the originating event. Right for a deterministic transformation that needs no LLM judgement.

Lifecycle (both shapes): defined by `TriggerCreated`; each firing emits `TriggerStarted` then `TriggerCompleted`. The panel row shows its **last-run status** (OK / failed) beside the last-run time; there is no built-in run-history view. For more, ask the *Lucidos Agent* ("what has this trigger been finding?") or build an *app* on its *event* stream.

The row also shows the **next runs**: the next few fire times, merged across every cron expression. A cron that can *never* fire (`0 0 9 31 2 *`, Feb 31) is rejected at create and update, naming the offending fields. A trigger stored before that guard still loads, but wears a **schedule error** rather than the "No more runs" of a spent one-shot. Neither has a next run, but one never worked and the other finished its job.
See also: `system-knowhow/triggers.md`, `docs/taxonomy.md` § Triggers.
<!--gloss-trigger-end-->

### Interrupted run
A trigger run cut by an engine restart. Its thread stays failed in the inbox, with a notification and a notice explaining the retry decision. A qualifying run retries once in a new thread; its header links back to the interrupted run. A retry cannot retry again. A working automatic resume after a user-initiated switch does not count.
See `system-knowhow/triggers/writing-cron-expressions.md` § "A fire due while the engine restarts" for eligibility and deadlines.

### Off-schedule run
A firing of an existing *trigger* asked for by a person rather than by its schedule or an *event subscription*: `triggers(action="run")`, `lucidos triggers run --id`, `lucidos.triggers.run(id)`, or the **Run once** button on the trigger's panel row. It is **indistinguishable downstream** from a scheduled fire. Same `TriggerExecuted` / `TriggerCompleted`, `last_run` and last-run status, *trigger thread*, `go_to_review` routing and *program grant*, and no actor stamp. A manual run also suppresses a redundant catch-up of the slot it covered.

Distinct from the *Thread Queue* panel's **Run now**, which force-admits an *already queued* entry and cannot create a fire. Refused when the trigger is paused (resuming restores the schedule but runs nothing). Refused too when it has no cron schedule; emit its subscribed event instead to reproduce an event-driven fire. A fire of the same trigger already active or queued is reported, not started, since cron fires coalesce to one pending run.
See also: `system-knowhow/triggers/running-an-existing-trigger-once-off-sch.md` § "Running an existing trigger once, off-schedule".

### Trigger definition
The on-disk `trigger.toml` at `data/triggers/<slug>/trigger.toml`: a **derived read-model** of a *trigger*'s durable config. The engine writes it on create and update, removes it on delete, and rebuilds it from events on boot. It is NOT the source of truth (events are), and **not version-controlled** (it sits in the repo's local `.git/info/exclude`). The scheduler never reads it, so a hand-edit changes nothing that fires and is overwritten. It makes a trigger inspectable, and lets a *plugin* SHIP a trigger by declaring one (install parses it into a `TriggerCreated`). See ADR 0019, `system-knowhow/triggers/running-an-existing-trigger-once-off-sch.md` § "On-disk trigger definition".

### Definition digest
A hash of what a *trigger* runs and when: its `run`, its `on` list, its cron, its MCP servers and its app. Its name, pause, model and grants do not count. A *batch* that grants to a trigger shows the first 12 characters and stores the whole digest. Your consent binds to that digest: if the trigger's definition changes before the item runs, Lucidos skips it. Lucidos computes the digest on read, so older triggers have one too.

### Event subscription
A standing request to be told when a matching *event* happens: an `event_type` plus an optional payload `condition` scoped to that event. One shape, one matcher, two species. Without a condition it matches every event of its type; with one, only events whose payload satisfies it.

A **trigger subscription** is one entry in a *trigger*'s `on` list. On a match it **spawns a new thread** and **stays armed**. That makes a trigger a standing rule: it outlives every thread it starts and fires until paused or deleted. A trigger may carry several, and each filter constrains only its own event, so payload shapes never interfere.

A **thread subscription** is one an existing *thread* armed for itself, with `await_event` (the *Lucidos Agent*) or `lucidos await-event` (a *coding agent*). On a match it **resumes that thread** and is **spent**, so the next one needs a new arming. The code and event log call it an *event wait*.

The same matcher decides both, so a `condition` that fires for one fires for the other. A thread screen shows only the thread species.

**What can be subscribed to: any persisted event.** A thread event, a *domain event* your workspace emits, or a persisted system fact such as `BackupCompleted`. A transient engine frame writes no row and reaches no matcher, so both species refuse it at the tool boundary. A transient *domain event* is the exception, since your workspace named it. It matches live only, with no row for a catch-up to replay.

**The name is checked when you subscribe, not when nothing arrives.** Matching is exact, so a typo would arm clean and wait forever. Both species refuse a nonexistent engine name, naming the near match, and a retired one, naming its replacement. Any other name is accepted, with a warning when unseen here, since a domain event not yet emitted is legitimate. Look names up with the `events` tool's `event_types` action.

**The condition's paths are checked too.** A *field path* that no payload of that type carries fails as silently as a misspelled name. So each is checked against the most recent stored payloads, and a path in none of them draws a warning naming the real path. Not a refusal: an optional field can be absent from a sample.
See also: *trigger*, *event wait*, *waiting indicator*, `system-knowhow/triggers/when-a-trigger-is-the-right-answer.md` § "One trigger, multiple events", `system-knowhow/thread-events/one-table-two-enums.md` § "Check the name before you subscribe".

### Field path
A key in an *event subscription*'s `condition`, naming one value inside the event payload. A bare name reads a top-level field, and dots read downwards: `workflow_run.event`. Both subscription species use the one matcher, so a path means the same to a *trigger* and an *event wait*.

Three rules. Resolution prefers a key that exists verbatim at every level, so a payload field literally named `a.b` stays nameable. A path that resolves to nothing is JSON null, like a missing top-level field. A numeric segment is an object key, never an array index, so an array anywhere on the path ends resolution.

See also: *event subscription*, *trigger*, `system-knowhow/triggers/writing-cron-expressions.md` § "What a condition can say".

### Continue key
An opaque value that ties later *trigger* fires to the thread an earlier fire started. A trigger's key spec lists parts. Each part is a *field path*, or a list of field paths where the first with a value wins. The key is the JSON array of the resolved values. Keys are shared across triggers, so two triggers that build the same key reach the same thread.

A fire whose key matches an open thread becomes a new message there. If that thread waits on a *gate*, the fire answers it as free text, and the agent asks again if it does not. Otherwise a new thread starts and keeps the key. One key maps to at most one open thread. The Archive action releases a thread's key, and unarchiving does not take it back. An unattended run that lands in Archive on its own keeps its key.

The key is only for input. Lucidos compares it and never parses it, so it names no provider. Replies go out through workspace script triggers, addressed from the thread's first delivery.
See also: *root thread*, *field path*, *actor*, `system-knowhow/triggers/writing-cron-expressions.md` § "Continue key".

### Actor
The user behind a *trigger* fire, named by email. A trigger's `actor` field names the payload field that holds the email. Lucidos maps the email to a *user*, and an email no user holds reads as a stranger. A source that sends ids instead of emails gets a workspace script trigger that looks the email up and emits a domain event carrying it.
See also: *user*, *continue key*, `system-knowhow/triggers/actor.md` § "Actor".

### Trigger thread
A *thread* spawned by a *trigger* firing, marked `source = 'trigger'`. Its LLM has the same knowhow access as a chat thread: the system prompt advertises the intent registry, and the LLM calls `load_knowhow` when relevant. No per-trigger knowhow allowlist. Terminal event: `TriggerCompleted`.

### Trigger group
A user-visible folder that organizes *triggers* in the triggers panel. Pure label: no agent, no schedule, no code. A trigger belongs to at most one group via `group_id`; ungrouped triggers show under an implicit "Ungrouped" section. It can gather an emergent workflow (triggers chained by `events(action="emit")` → `on_event`) into one group without changing how they fire. Lifecycle: `TriggerGroupCreated`, `TriggerGroupRenamed`, `TriggerGroupReordered`, `TriggerGroupDeleted`. A group with members cannot be deleted until its triggers are reassigned or deleted; panel order follows `order: i32`, ascending.

### Event hook
A *script* Lucidos asks before it records an *event*, so a workspace can stop progress until its own rule holds. The hook allows, asks for input, or refuses. Like a *trigger*, it is workspace content: you author one, or a *plugin* ships one. Its *hook definition* names the event.

A hook is asked only where the act has not happened yet, so a refusal can stop it. Reacting after an event is a trigger's job. A transient event is never recorded, so no hook can name one.

A hook runs only once the owner **arms** it, in Settings > Event Hooks or with `lucidos hooks arm`. Arming binds the hook's current definition and script. An edit leaves it **changed**: its events are refused until the owner arms the new content or disarms it. An agent arms, disarms or removes a hook only with the owner's Allow once.

**Today no hook can be armed yet,** because Lucidos asks hooks about no event so far. A definition changes nothing yet.
See also: *hook definition*, *hook answer*, *trigger*, `system-knowhow/lucidos-cli/subcommands-9.md` § `lucidos hooks list | arm | disarm | remove`.

### Hook definition
The file `data/hooks/<slug>/hook.toml` that declares an *event hook*. You write it, and Lucidos never rewrites it, unlike a *trigger definition*. Its keys:

- `name`: a label.
- `event_type`: the event's name, a thread event or a persisted system event.
- `condition` (optional): a payload filter in the *event subscription* syntax. Lucidos checks it before it starts the script.
- `script`: a `.py` or `.sh` file inside the hook's folder, named by its path from there.
- `timeout_secs` (optional): 10 seconds by default, at most 60.

`lucidos hooks list` shows every definition, valid or not, with its problems. An unknown key, an event no hook can name, a bad condition or a missing script each make it invalid.
See also: *event hook*, *field path*.

### Hook answer
What an *event hook*'s script decides about one pending event. The script runs as a trigger script does. It reads the hook's slug and the event on stdin, with every stored secret masked out. It prints one JSON object, and its `answer` key names the answer:

- `allow`, with an optional `events` list of domain events to record beside the event;
- `needs-input`, naming the `argument` it needs;
- `refuse`, with a `reason`.

A hook fails closed. A crash, a run past its `timeout_secs`, or output that is no answer each count as a refusal. An event Lucidos records itself is no answer, so a hook returns domain events only. Nothing asks a hook yet, so no script runs.
See also: *event hook*, *hook definition*.

### Typed answer
What you type into the composer while a *question card* waits. It is that card's answer, so it shows on the card under **Your answer**, never as a message below. While sending, the card's header reads **Sending**. If Lucidos does not answer, it reads **Not sent**, with a retry icon under your answer and the options live again. Tapping an option or typing a new answer replaces it.

A typed answer need not answer the question. It may ask back or say something else, and the agent then responds and asks again.

A tapped option behaves the same way: with no answer, it stays marked **Not sent** with a retry icon. Tapping **Not sent** shows what went wrong.
See also: *unsent message*.

### Unattested caller
A caller Lucidos cannot tie to a signed identity: it carries no valid *thread-bound origin token* and no gateway stamp, the gateway's signature on a device or workspace claim (ADR 0647). A typed device id, a `caller_workspace` body field and the machine-local token all land here. The chat shows its message as "Message from an unattested caller". Recorded as `MessageOrigin::UnattestedCaller`, with the claims kept as display hints (`claimed_device_id`, `claimed_workspace`, `claimed_local_token`).

**It may read, chat and do ordinary writes under its own actor.** An owner-only act, including starting a coding agent, gets an immediate 403. Use the Lucidos app or use a trigger so the job runs as an agent.

Not the *API caller*, which presents no credential at all. An unattested caller presents one, but nothing signed.

### Unsent message
A message you sent that got no answer from Lucidos, so it may never have arrived. Lucidos first retries it quietly, up to three times over about 13 seconds, while it still shows as sending.

Only then does it stay in the *thread* under a **Not sent** card that quotes it, with a **Retry** button. A *typed answer* to a waiting question card stays on that card instead. It never vanishes or reads as a failed reply. Retry sends the same message again. If the first did arrive and only the answer was lost, Lucidos recognizes the repeat and runs it once.

It is kept on the sending device, so a page reload brings it back, Retry included. That covers a send the reload cut off, whose card says so. If the message did arrive after all, it shows as sent. **Discard** removes the card without sending. Other devices never see it, and your *draft* never holds a copy.
See also: *draft*.

### Update channel
Where this machine checks for Lucidos updates. **Stable** carries published releases; **Beta** carries tested release candidates for the packaged macOS app.
**Nightly** is reserved and unavailable. **Local** reads a candidate staged on this machine, through its gateway.

A **Custom** channel reads a remembered *custom slot*. Switching channels keeps every slot and never installs an older or equal version.
Every installable update still needs the Lucidos signature.

### Custom slot
A remembered name and *update address* that a Custom *update channel* selects. Switching channels preserves the slot; removing it is a separate action.
Names contain 1 to 64 characters, with no surrounding whitespace, control characters, colon or slashes.

