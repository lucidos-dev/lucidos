---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### Gate
A question card in a *root thread* or one of its *sub-threads*: the point where the work waits for a user. Which work stops where is policy, written in knowhow. No engine code decides when to stop.

A gate is live. Anyone's answer clears it from every view, for every *user*. A gate always reaches someone: with no reachable *driver*, its push goes to the whole team.
See also: *driver*, *Blocked*, *root thread*.

### Driver
The one *user* responsible for a *root thread* and its *sub-threads* right now. A thread you start makes you its driver. A trigger's root thread starts with none. Answering a *gate*, or tapping **Take**, makes you the driver, and **Take over** moves the role in one tap.

**The driver is soft.** Anyone can still answer any gate. A gate its driver leaves past the team's timeout escalates once: it pushes the whole team and shows in the Team view. The driver keeps the root thread. An answer from the source, through a keyed trigger fire, sets a driver only when its *actor* matches a user.

The timeout is the `driver_timeout_hours` preference, and `0` never escalates. **Release** gives the role up.
See also: *watcher*, *gate*, *user*.

### Watcher
An *user* who follows a *root thread* without being responsible for it. Tap **Watch** in the thread's users panel or in the *factory app*. A *user mention* also makes you one. A watcher gets the root thread's gate pushes beside the *driver*, and sees it under Mine. **Unwatch** in the users panel, or on a *user-only message* push, stops it.
See also: *driver*, *root thread*, *user mention*.

### User mention
`@Name` in a message, naming another *user*. Mentioning yourself does not address a user or start a pause. It pushes that user and makes them a *watcher* of the *root thread*.

The composer suggests names as you type `@`, and **Tab** takes the first. A message that mentions only users is a *user-only message*, and it starts the *agent pause*. Team gateways only.
See also: *agent mention*, *agent pause*, *user*.

### Agent mention
`@agent` in a message. It wakes the thread's agent, and ends an *agent pause*. One message may carry it beside *user mentions*: then the agent answers, and only the users it names get a push.
See also: *user mention*, *agent pause*.

### Agent pause
The state in which device-origin typed messages in a thread stop waking its agent. CLI, API and cross-workspace sends still wake it.

A message that mentions only users starts it. While it holds, an untagged message stays between users too. An *agent mention* ends it. The pause belongs to the thread and reads the same for everyone. The transcript draws nothing for it.

While it holds, the empty composer reads "Agent paused. Mention @agent to wake it". A pause never stops a running turn. Input that no user typed (a trigger fire, a *sub-thread*'s report, a question answer) still wakes the agent. Team gateways only (ADR 0649).
See also: *user-only message*, *agent mention*.

### User-only message
A message a user writes to the other users in a thread, not to its agent. It starts no turn. The agent reads it, under the writer's name, the next time something wakes it. It pushes the users it mentions plus the *driver* and *watchers*, minus the writer, and one banner per thread replaces the last. It draws as an ordinary message bubble, with no mark of its own.
See also: *agent pause*, *user mention*, *watcher*.

### Notification audience
The devices a notification is for: the users a notice names, or else the root thread's. A notification that opens a thread is for its *root thread*'s *driver* and *watchers*. It widens to the whole team when the root thread has no driver, has an escalated *gate*, or names nobody who holds a device. Every other notification, and every notification on a gateway with one *user*, is for everyone. Push skips devices outside it, where it reads as read.
See also: *driver*, *watcher*, *gate*, *read state*.

### Read state
Whether you have read a notification, or seen a reply a *read request* points at. On a gateway with two or more *users* it is per user: a read on one of your devices counts on all of them and on nobody else's. With one user it is shared, as before.
See also: *user*, *notification audience*.

### Config change
A repository, trigger, MCP server or credential that someone added, changed or removed. On a *team workspace* each one tells the rest of the team who made it, with **Undo** where the change can be inverted. Undo never brings back a credential's value, only its other settings.
See also: *team workspace*, *user*.

### Factory app
The Lucidos app a *team workspace* installs from its setup knowhow (`system-knowhow/team-workspace`). It lists open *root threads* with their *driver* chips, split into Mine, Team and All, and offers **Take** and **Watch**. Team lists the *gates* nobody owns: no driver, or escalated past the team's timeout. The core thread drawer does not change.
See also: *team workspace*, *driver*, *watcher*.

### Translation script
A script *trigger* in a *team workspace* that turns a source's raw input into a *domain event* the intake trigger can read. It resolves the source's user id to an email, skips the factory's own bot and strangers, and names the source in the event. The engine never learns what the source is.
See also: *team workspace*, *actor*, *continue key*.

### Review
The *Ongoing grouping* group for finished work that is your turn to look at. It lists every Current and Saved *thread* whose turn has ended and that holds a ready *change* or a *read request*. A change the *landing lane* holds leaves Review, since the lane decides when it lands. Opening one from Review lands on the change's turn when a change is ready, and otherwise on the start of the newest turn.
A saved position in that turn or later, including the live edge, restores instead. A read request adds nothing to the Blocked count.

Its tile splits the count into changes and unread. A thread with both a ready change and a read request counts as a change. Press the selected Review tile again to list only the changes, again for only the unread ones, and once more for all. The filter skips a kind with no threads, and with only one kind there is nothing to filter. The tile marks the count it lists. When the listed kind runs out, or you select another group, the whole group lists again.

### Read decision
The yes or no every agent turn ends with, about whether its reply is worth reading. A yes is a *read request*. The agent decides with the `request_read` tool when it is sure, or a coding agent with `lucidos request-read yes|no`. When a ready *change* or a harden hold already lists the thread, the answer is no, with no call.

No answer is assumed. When a turn ends without deciding, one background call decides it (Settings → Models → Background tasks → Read decision). It tries the chat models Lucidos measured for this decision first, and you can pick any model there, OpenAI Decisions included (ADR 0472). That call reads what opened the turn, a question you answered inside it, and the reply. A call that fails or gives no answer counts as a yes.

A turn can end with work still in flight. Its *sub-threads* still run, its *event wait* is armed, or a successor session carries the work on. A progress report from such a turn is a no. A yes is for what you should read before the work finishes, such as a failure, a finding or a decision. The background call learns how the work continues (ADRs 0424 and 0761).

A *trigger* run says no when it found nothing new, including when it writes a routine report. It also says no when a notification already carried the news, unless the reply holds a failure the notification omitted. When the agent says yes or leaves the decision open, the background call decides. This includes wakes from a *sub-thread* report or an *event wait* that continue the run. It sees the completed reply and what the turn already notified you about (ADRs 0472 and 0747).

A *sub-thread* that another agent started reports to that agent. When its report wakes the parent, the parent acts on it and you read the parent's reply. A routine result from such a turn, such as a review verdict, is a no. The parent asks you for any approval. A yes is for a failure, a gap you must know about, or something only you can do or decide. The background call is told when the report wakes the parent, unless you opened the turn or answered a question in it (ADR 0446).

The sub-thread's own agent is told the same, so it decides the same way. A child spawned with `wake` or `wake-when-all-done` always learns that its parent acts on its report. One that decides its own wake learns it for the reports that wake the parent. A `no-wake` child and a top-level thread are not told, since you read their reports in the thread itself. A turn you open in the sub-thread is answered as usual (ADR 0607).

A no changes nothing you see, and never clears an earlier read request you have not read yet. The exception is a sub-thread spawned with `wake` or `wake-when-all-done`: its parent already has the earlier report, so a later no clears it. A request from a turn you opened, answered a question in, or wrote into in that sub-thread stays (ADR 0417). Answering a question the thread asked does clear it, since the question sits at the end of the reply, as sending a message does.

### Wake mode
What a *child thread*'s report does to its *parent thread*, declared by the spawn: `wake_mode` on `run_thread` and `run_coding_agent`, `--wake-mode` on `lucidos spawn-thread --relation child`.

- `no-wake`: the user reads the result in the child, and the parent may still want it. Every report is a *filed report*, which the parent reads on its next turn. A failure or an `interrupted` turn is filed too, since the child shows it in Blocked.
- `wake`: the parent needs the result to continue, so every report wakes it.
- `wake-when-all-done`: the parent combines several results. A clean report wakes it only when no other child of that batch is still working. The earlier ones are filed.

A parent that will not act on the result spawns a *top-thread* instead, which sends no report.

A child of the *home thread* defaults to `no-wake`, and a child of any other thread to `wake`. Under `wake` and `wake-when-all-done`, a failure, a cancel, an `interrupted` turn, an archive and a discard wake the parent. The mode lasts the child's life: a *child follow-up* keeps it, so a parent that needs an answer from a `no-wake` child spawns a new one with `wake`. A top spawn takes none. Stored as `thread_summaries.wake_mode`, from the child's first `MessageReceived`.
See also: *wake decision*, *filed report*.

### Wake decision
The fallback for a *child thread* with no *wake mode*, one spawned before modes existed: the yes or no its turn ends with, about whether its report wakes its *parent thread*. Yes when the parent must act now: it uses this result for its next step, it combines results and this is the last, or it must retry or decide. No when the report only informs: the parent waits for more children's results, or you read the child's findings in the child's own thread. A chat child decides with the `wake_parent` tool, and a coding agent with `lucidos wake-parent yes|no`. A turn that skips it is decided by one background call, and any doubt wakes the parent.

That call runs on OpenAI Decisions while the OpenAI provider is set up, and on a chat model otherwise. Unless you pick one, that chat model is one Lucidos measured for this decision. With none of those served, no model is asked and the parent wakes. Pick its model under Settings → Models → Background tasks → Wake decision.

Only a clean finish reads it. A failure, a cancel, an `interrupted` turn and a turn the engine cut short always wake the parent. Each turn starts undecided. When both decisions are left to the background, the wake decision goes first, so the *read decision* knows whether the parent acts on the report.

A child with a wake mode makes no wake decision: the tool is not offered, and a call answers that the parent already declared the mode. Events: `ParentWakeRequested` and `ParentWakeNotRequested`, on the child.
See also: *wake mode*, *filed report*, *read decision*.

### Filed report
A *child thread*'s report, delivered to its *parent thread* without waking it. Its *wake mode* is `no-wake`, which files every report, failures included. Or it is a clean early report of a `wake-when-all-done` batch, or a clean one its *wake decision* said no to. The event is `ChildReportFiled`, with the same content as the waking `ChildThreadCompleted`. It starts no turn and moves no thread in the drawer. The parent reads it on its next turn: a chat parent in its history, a coding-agent parent at the front of its next prompt.

In the parent's chat it wears the child report card with the line "Filed without waking this thread".
See also: *wake mode*, *wake decision*, *child thread*.

### Read request
A *thread*'s flag that its latest reply is worth reading, so *Review* lists it: the yes half of a *read decision*. The thread's agent can set it with the `request_read` tool, or a coding agent with `lucidos request-read yes`. It does so only when the reply holds what you asked for, such as a report or findings. When a *trigger* run asks to be read, the background call decides whether to set it. A report on work still in flight says no, unless it holds something you should read now.

Its row wears a filled dot, while a ready *change* wears a diff glyph.

A thread whose *change* already lists it never has one: a ready change in Review, or work held for a missing harden in Blocked. So applying a change leaves no unread dot behind.

It clears once the end of the latest reply has been on screen for a second, the same dwell a *seen target* uses. Your next message or an archive also clears it. Reading on one device clears it on all of them. A *trigger* run with a read request lands in the *Current section*. It is not a notification: it neither pushes nor toasts. Not to be confused with a *read marker*, which says the agent took in your message.

### Thread link
The address that opens a *thread*, written as a markdown link target: `[Plan dinner](thread:myws/<thread_id>)`. Every row the `threads` tool and `lucidos threads` return carries one as `link`, as does a spawn result. Its workspace is always the one that served the row. A *draft* or a *held message* has no address of its own, so its link is its thread's.

### Settings link
The address that opens one Settings page, written as a markdown link target: `[Settings → System → Backup](settings:backup)`. The part after `settings:` is the same view id `navigate_ui` takes as `settings_view`. A `#<row>` after it names a row on that page, the same as `navigate_ui`'s `fragment`: `[xAI](settings:providers#xai)` opens Settings → Providers at the xAI block. The label is the breadcrumb route, so a surface that shows plain text still reads the route. Engine notifications that name a Settings page link it this way, and the link opens the same page as the notification's tap.

### Reader fields
Four fields on each row that `lucidos threads list`, `lucidos threads search` and the `threads` tool return, saying what each *thread* holds and how to open it. `has_draft` says whether it holds a *draft*. `draft_preview` and `draft_length` give the draft's first 200 characters and its length, only when there is one. `link` is its *thread link*. Other thread reads leave them out.

### Thread liveness
What a *thread* is doing right now, as Lucidos reports it: one `state` plus the evidence behind it. Every thread-scoped read carries it as `liveness`: an `events` read of one thread, each `threads` search and list row, and `lucidos.threads.liveness()`.

| State | Meaning |
|---|---|
| `working` | A turn runs and was active within 10 minutes, or a successor session is about to start. |
| `quiet` | A turn runs but nothing happened for 10 minutes or more. Not a verdict. |
| `waiting_for_user_answer` | Parked on a question card the owner is asked to answer. A held thread never reads this. |
| `paused` | A version switch interrupted it, and Lucidos resumes it. |
| `failed` | The last turn failed, and nothing started since. |
| `queued` | The Thread Queue holds the thread until capacity frees. |
| `held` | An owner Stop or a *hold* holds every wake. A Stop lifts with your next message, a hold only with Resume. A held thread parked on a question reads `held` too: its card stays open, but nobody is asked to answer it. |
| `stalled` | A *handoff* happened, and the successor session has not started for 2 minutes. |
| `waiting` | No turn, but it wakes itself: an *event wait* or a sub-thread it waits on. |
| `idle` | Nothing runs and nothing is owed. |

Read what a thread is doing from `liveness.state`. The adjacent `status` is the *turn status*. `status: idle` means no turn is open, and `status: waiting` is a legacy value. A thread can have turn status `idle` while its liveness is `waiting`, `held` or `queued`, and turn status `waiting_for_user_answer` while its liveness is `held`.

Liveness gives the seconds since the last event and the current agent session's ordinal, opening and age. `handoff: "pending"` shows while a handoff waits, and `as_of` says when Lucidos looked. The following evidence appears when present.

- `live_event_waits` names each live *event wait*, oldest first. Each entry gives its `reason`, the subscriptions it listens `on`, `expires_at`, and `expires_in_secs` at `as_of`. An engine-owned background wait may give `background_timing`.
- `awaited_children` names the direct *awaited children* by `thread_id` and `title`.
- `expected_wake` gives the time, source and reason of a waiting or held thread's newest *expected wake*, provided it was recorded after the last turn started.
- `hold` says what holds a wake: `stop`, `open_card` with the card's id, or `automation_pause` with its reason.

Say what a thread is doing now (waiting, running, stalled, done) only from a liveness read in the same turn. Each turn of a thread with child threads opens with one: its *child-status block*. Without a read, say what it reported, and when. An older read, an earlier reply, or a child's earlier report is history.

