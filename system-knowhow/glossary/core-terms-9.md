---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### Trigger deny
A program or folder one *trigger* may never use, saved when you pick **Always deny** on its card.
A run that reaches a denied entry stops at once, with no card, and its thread says why.
A program deny matches its exact pattern. A folder deny also covers its sub-folders.

Saving a deny removes a matching grant, and saving a grant removes a matching deny. Folders match when one holds the other.
Trigger details lists the denies under Denied, and only the owner lifts one.
Other threads and triggers are unaffected. The workspace-wide refusal is the command card's Always deny.

### Signer manifest
The `<name>.manifest.json` sidecar next to a `<name>.wasm` signer artifact in `data/auth-modules/`. Carries WASM-host metadata (`secret_handles`, `body_mode`, `capabilities`). The engine never loads provider config from it: `data/config/apis.json` is the single source of truth for proxy entries.

### Slowness warning
An amber bar atop a workspace window saying Lucidos is slow, and why when it can tell. It picks the first reason that holds:

- **Almost out of disk space.** Under 2 GB free where Lucidos keeps its workspaces or database. The bar says how much is free and to free up space. It shows in every workspace, since the disk belongs to the whole computer.
- **Database not responding.** A workspace answers, but its database does not. The bar says to restart Docker, or Lucidos on an installed app. If the database answers but every connection is busy, it says Lucidos is waiting for a free database connection. It shows only in the reporting workspaces' windows.
- **Short on memory.** The bar lists the biggest memory users, summed by app, with Lucidos (engines, coding agents, database) counted as one. It shows in every workspace.
- **Responding slowly.** None of the above holds. The bar lists the apps using the most processor time, grouped the same way. It shows only in the slow workspace's windows.

For memory and slow responses it recommends one thing: quit or restart the busiest app, or stop idle coding-agent threads when Lucidos is the biggest. Only when nothing stands out does it suggest restarting the computer if slowness lasts.

One stretch of slowness is an *episode*. It opens when a workspace answered slowly or lost its database for most of the last five minutes. Sustained memory pressure opens one too; on a Mac, real swap use must come with it. The bar names memory when the OS reported memory pressure for at least half that time. The episode closes after five quiet minutes.

Dismissing the bar hides it on that device until the next episode. The gateway measures it once for the whole machine.

### Skipped slot
A cron slot of a *trigger* that passed with no run, because the computer was asleep or Lucidos was not running. A slot from the past hour still fires once when Lucidos wakes or starts. An older one never runs. Lucidos posts one inbox notice instead, with a push, per trigger per gap. It names how many runs it skipped, and the tap opens the trigger. A gap the trigger ran in after all gets no notice.

### Source event
The specific *event* a notification points to, stored as `notifications.event_id`. The *in-app surface* uses it to tell whether the user is looking at that very thing. If the page is on its thread AND it is in viewport, the notification is auto-marked read with no toast and no badge increment. A notification with `tap = { kind: 'navigate', to: { target: 'thread', id: '...', event_id: '...' } }` also lands on it (scroll + pulse).

Both uses resolve the event the same way, whether it starts a turn or is folded into one as a step. A step is addressed by its own card (a failed response by its failure card), not by the surrounding turn. A source event that never renders is reported, and the transcript stays where it was.
See also: `system-knowhow/notifications/2-the-fan-out-rule.md` §2, `system-knowhow/notifications/3-the-presencecheck-protocol.md` §4.

### Spawning thread
The *thread* that issued the `run_thread` / `run_coding_agent` / `lucidos spawn-thread` call. For `relation: "child"`, it IS the parent. For `relation: "top"`, there is no parent and no callback wiring. Either way the spawn is *attributed*: the spawned thread's first message records its launcher, so its route popover links back here. Attribution is not linkage: a top-thread never reports back, counts as a child, or inherits the spawning thread's permissions.

### Stopped thread
A *thread* whose turn you ended with Stop. It takes no automatic engine wake
until you resume it with a device message, Continue, a card answer, Harden or
Apply. Event waits and child reports remain waiting for that resume.

A message
from Home or a parent can still start a turn, but does not lift the hold on
engine wakes. The hold survives an engine restart and applies to chat and
coding-agent threads. Distinct from a *hold*, which no message lifts and
which reaches the sub-threads too. See ADR 0696.

### Stopped child
A *stopped thread* that is a *child thread*, not yet continued, archived or discarded. Cancel on its question card counts as a Stop. The child is alive, and one message continues it. Its *parent thread* gets a quiet `ChildThreadStopped` note and no turn, and is still owed a result.

The child's next finished turn sends the usual completion card. Archiving or deleting it, or discarding its change, sends one with status `canceled`. Meanwhile it counts toward *attention*, and a notice at the end of its transcript names the waiting parent. It blocks nothing, so the parent can still be archived.

Distinct from a *thread pause*, which the engine resumes by itself. See ADR 0252.
See also: *child thread*, *parent thread*.

### Standing deny
A command rule you set that always refuses commands running one program, such as every `curl` command. Set it with **Always deny** in the Remember menu of a *command permission card*, or add a line under **Settings → Permissions → Standing denies**. The line has the shape an Always allow stores: `Bash(curl:*)`, `Bash` for any shell command, `Python` for any Python run.

A matching standing deny beats every allow. The Lucidos Agent, triggers and coding agents check it before allows or the judge. The rule holds with the *command guard* on or off. The refused command's result names the rule. Only the workspace owner can set or remove it.

If the rules are unavailable when a coding agent requests approval, its card explains the fallback and offers Deny or Allow once. Commands run independently by provider extensions are outside these checks.

It is a **command rule, not file protection**. It matches the program's name wherever it runs in the line, so a script that calls the program inside itself is not stopped. To keep a file from being read or changed, use a *private path*.

Claude Code and Codex read the current rules on their pre-execution hook and permission paths. A rule set during a session applies on its next check, in either *agent sandbox* mode.

### Status filter
The `--status` flag on `lucidos threads list` / `count`, and the matching `status` parameter on `GET /api/v1/threads/{list,count}`, the `threads` tool and `lucidos.threads`. It names exactly the *thread* statuses to keep, out of `idle`, `running`, `waiting`, `waiting_for_user_answer`, `paused`, `failed`. These are the values each returned *thread summary* carries in `status`, so a caller filters on what it reads. It is the precise form of the *Active (thread state)* union: `status=running` asks whether the workspace is busy, `status=waiting_for_user_answer` whether anything waits on you, and `active=true` both. Passing `status` with `active` is refused rather than intersected, as is an unrecognized or empty value. A *held* thread parked on a question matches neither `status=waiting_for_user_answer` nor `active=true`, since the Blocked section hides it.
See also: `system-knowhow/lucidos-cli/subcommands-2.md` § `lucidos threads list`.

### Superseded question
A *question card* the engine resolved because a follow-up arrived that could not be its answer. Unlike a canceled one, the user did reply, just not to this question, and that reply drives the next turn. The card reads "Replaced by your next message" with its buttons spent; on the wire it is `UserQuestionAnswered { answer: { kind: "Superseded" } }`.

Coding-agent lane only, to break a deadlock only that lane has. The agent is parked inside the call that asked, so only an answer releases it, and the follow-up's prompt event has already killed the card. A chat thread keeps its question live, queuing the follow-up as an injection until the user answers.
See also: `system-knowhow/coding-agent-events/billing-evidence.md` § `UserQuestionAsked`.

### Protected surface
A surface where the user grants, denies, answers or confirms: permission and question cards, the credential and email forms, the Apply controls, plugin install panels, and the confirm, prompt and progress dialogs. For text, fills and confirm or deny colours it reads only the `--protected-*` palette. So no *theme*, *theme part* or *style override* can make it unreadable or misleading. The engine derives that palette from the active theme and clamps it. Text and button labels reach WCAG AA, confirm stays green, deny stays red, and a blocking dialog's scrim always dims. See `system-knowhow/themes/switch-themes.md` § Protected surfaces.

### Style override
One entry in the `style_overrides` *preference*: a CSS custom property name and its value, applied on the app's root element. Writing one repaints every connected client live, over the `PreferencesChanged` fan-out, so a design value can be retuned with no rebuild. Device-scoped like other appearance preferences, so tuning on a phone leaves a desktop alone. Values only: it can retune a colour, a size or a duration, never move a control or change behavior.

An override never sets a `--protected-*` token, a `--z-*` stacking token, the UI font tokens, `--user-ui-scale` or the scanlines. It cannot reach inside a *protected surface*. A shadow override stays within 2rem of its box, as a theme's must.

Two ways out if a value makes the UI unusable: **Settings → Appearance & Behavior → Style overrides → Clear style overrides**, or `?style-reset` on the URL. The URL flag clears them before first paint, so it works when nothing is readable. It also resets the *theme* to the default.

### Style remote
The app that writes *style overrides*: sliders and colour pickers, one knob per custom property. As an *app* it is workspace data, so its knob list is edited in place and never goes through *Apply*.

<!--gloss-sub-thread-start-->
### Sub-thread
Any descendant in the *thread* tree (transitive): a *child thread* or a grandchild alike. Say *child thread* for the direct relationship and *sub-thread* when depth doesn't matter.
<!--gloss-sub-thread-end-->

### Thread
A single conversation: a stream of events sharing one `aggregate_id`. Every chat reply, trigger run, and *coding-agent thread* run is a thread. Its persisted `source` is `chat` / `trigger` / `claude_code` (*channel* identifiers; see dev glossary). User-facing and API source filters call the coding-agent bucket `coding-agent` and accept legacy `claude_code`.

A thread has a compose state (`composing` / `active` / `discarded`; at runtime running / idled / failed). Its archive flag (`inbox` / `archived`) is orthogonal: an archived thread keeps `state='active'` and only flips `archive_state`. A thread may spawn other threads.

### Event wait
The internal name for a **thread subscription** (see *event subscription*): a *thread* asking to be re-opened when something happens, instead of polling. The word survives on disk, in the persisted `EventWait*` events and the `await_event` tool's name. The screen says neither: the conversation row is a **Waiting for** card with the agent's reason, and its state word says how the wait ended.

The agent says what it waits for (optionally filtered), why, and for how long, then finishes its turn. The thread holds and blocks nothing while it watches. Lucidos re-opens it when a matching *event* arrives, or tells it the wait timed out. The *Lucidos Agent* and a *coding agent* can both use it, list their waits and cancel them (`list_event_waits` / `cancel_event_wait`, or `lucidos event-waits list` / `cancel`).

A watching thread reads as **Waiting**, like one waiting on its *sub-threads*, even after a reload; the *waiting indicator* says what for. A *read request* shows instead when its agent made one. Its *todo list* agrees: unfinished items at park time are marked `waiting`, not `abandoned`. The same holds for a thread waiting on a *live child* still on its edge; one moved to top level reports to nobody.

Two things separate it from a *trigger*. **Where the answer goes:** a trigger runs in its own thread and reaches you as a *notification*, while an event wait resumes the conversation you are reading. **How long it lasts:** a trigger starts a NEW thread every time, indefinitely. An event wait resolves on the first match; the agent re-arms it per event, up to a cap on re-arms without you.

So "tell me **here** when a change is proposed" is an event wait, though it sounds like a standing rule. "Notify me whenever a change is proposed, from now on" is a trigger. Often both are right: watch here now, and add a trigger to keep going after this conversation.

**Neither a message nor Stop ends it.** A message runs an ordinary turn and leaves every subscription as it was, and **Stop** ends only the running turn. A wait that *fires* is used up, and the agent must subscribe again for the next one.

**Four things end one, and each says so:** **Stop waiting** in the *waiting indicator*, archiving the thread, discarding it, and the agent standing it down. Archiving asks first, naming every subscription it would stop, *sub-threads* included. Each leaves a line in the conversation saying what stopped and how, so a watch never ends in silence. None re-opens the thread, so ending the last one on an idle thread settles its *todo list* too.
See also: *event subscription*, *trigger*, *waiting indicator*, `system-knowhow/thread-events.md`.

### Waiting indicator
The prompt-bar control showing what the open *thread* is waiting for. It appears whenever the thread is parked: on a live *thread subscription* (an *event wait*), or on unfinished *sub-threads* (working, or asleep on their own event wait). A subscription lists the agent's reason, the event watched, a countdown to its deadline, and a **Stop waiting** button. A sub-thread is listed by title, and tapping it opens that thread. Without scrolling back, it tells a stuck thread from one asleep on purpose. The **Waiting** status says *that* it waits, on every list; this says *what for*, on the open thread.

A sub-thread row links rather than stops; you end a sub-thread on the sub-thread itself. A thread with a proposed change that waits only on sub-threads reads **Changes to review** instead, since its change can be applied. A change the *landing lane* holds is the exception.

Each subscription reads **watching for** and the event in plain words ("background job finished"), with its meaning on the tooltip. An event watched under a `condition` says **(with a condition)**, and tapping it opens the condition, exact event type included. The transcript's record of the wait opens the same thing.

The subscriptions section is headed **EVENTS**, since people wait for things to happen, not for subscriptions.

### Grouping button
The *thread drawer* header button that swaps between the *Folders grouping* and the *Ongoing grouping*. It shows where a tap goes: a folder while you are in Ongoing, an inbox while you are in Folders. Under Folders it carries the *Blocked* count. A tap into Ongoing selects Blocked when the button carried that count. Otherwise it keeps your last pick while that group has threads, or moves to the first group that has some.
See also: *thread drawer*.

### Folders grouping
The *thread drawer*'s default grouping: any *custom folders*, then the Pinned, *Current* and Archive sections. Each section collapses on its own, and the thread filter shapes what they list. Reached with the drawer header's *grouping button*, which shows a folder while the Ongoing grouping is on screen.
See also: *custom folder*, *Ongoing grouping*, *thread drawer*.

### Ongoing grouping
The *thread drawer* grouping by what is still going on with each *thread*: *Blocked*, *Review*, **Drafts** and *In flight*. Each group is a tile at the top showing its count, always all four and always in that order. One tile is selected, and only its threads are listed below the tiles. Tapping a tile only selects it: the open thread stays where it is, and a tap on a row opens that row. An empty tile is dimmed but can still be selected, and then the list says it is empty. A thread that matches no group is not listed here: find it under the *Folders grouping*.

Opening a thread from Blocked or Review restores where you last read if you already reached the target turn.
Otherwise it lands on what needs you. Which target depends on the tile you opened it from.

A thread shows in every group it matches, so a failed thread with a ready *change* is in Blocked and in Review. Only one group is listed at a time, so no row shows twice. Every thread type is listed: the thread filter shapes the *Folders grouping* only, and its button hides here. Reached with the drawer header's *grouping button*, which shows an inbox and carries the Blocked count while the Folders grouping is on screen. **Show in Folders** always goes to the Folders grouping, where the thread has its place.
See also: *Folders grouping*, *thread drawer*.

### In flight
The *Ongoing grouping* group for work that is not finished and does not need you. It lists every Current and Saved *thread* whose status reads **Running** or **Waiting**: a turn running now, a thread watching for an *event wait* of its own, or a parent waiting on its *sub-threads*. A sub-thread sits under its parent when both are in flight. A thread waiting for your answer or one that failed is in *Blocked* instead. A finished one with a ready *change* is in *Review*. One whose agent made a *read request* is in Review too, and stays in In flight while it still waits.

Its tile also works as a filter. Press the selected In flight tile again to list only the running threads, again for only the waiting ones, and once more for all. The filter skips a kind with no threads, and with only one kind there is nothing to filter. The tile marks the count it lists. When the listed kind runs out, or you select another group, the whole group lists again.

### Blocked
The *Ongoing grouping* group for threads that cannot go on without you. It lists every Current and Saved *thread* waiting for your answer or permission, one that failed, and a stopped *sub-thread*. It also lists one resting with *unproposed work* that is not hardened, whose *Not ready* strip offers **Harden**. An *integrated sub-thread* with only a hardening hold stays out of Blocked. Its parent receives a failed handoff with retry instructions, and the child still shows the *Not ready* strip.

Questions come first, then the newest. Its count rides the *grouping button* under Folders, and the thread-drawer toggle while the list is hidden. Opening a thread from Blocked lands on the open question or permission card, or on the failure.
A saved position in that turn or later, including the live edge, restores instead. It was called Needs attention until ADR 0409.

While it holds threads, its tile wears the waiting colour. A halo breathes around it while a blocked thread is still unseen. Selecting Blocked sees them all, and opening one sees that one. Only a newly blocked thread starts the halo again. With Reduce motion on, the halo holds still.

### Root thread
A *thread* with no parent: the top of its *sub-threads*. In a *team workspace* a root thread and its sub-threads are one piece of work. Its *driver* and *watchers* hold for the whole family. The retired name was *work item*.

A person starts one by writing a thread. A *trigger* starts one per fire, unless the fire's *continue key* matches an open thread, which then gets the fire as a new message.
See also: *sub-thread*, *continue key*, *driver*, *gate*.

