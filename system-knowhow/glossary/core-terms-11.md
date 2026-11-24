---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### Thread drawer
The pane listing your *threads*, in one of two groupings: the *Folders grouping* (Pinned, *Current*, Archive) or the *Ongoing grouping*. Its header's title names the one on screen, or reads **Filters** while the thread filter is open. It hides when the header is too narrow to show it whole.

The **grouping button** in that header swaps between them. It shows where a tap goes: a folder while you are in Ongoing, an inbox while you are in Folders. Under Folders it carries the *Blocked* count; under Ongoing the Blocked tile shows it. While the list is hidden, the same count rides the **thread-drawer toggle**, so a thread waiting on you stays visible. Hidden means the drawer is closed on desktop, or another pane shows on mobile.

On mobile the toggle leads the *thread pane* header and opens the threads pane. The hamburger **menu drawer** sits at that header's trailing edge and slides out from the right. The thread drawer is the first of the three panes, and with the *thread pane* makes up the *Conversation* side. CSS container `.thread-drawer` (`FocusedPane = 'drawer'`; on mobile the leftmost swipe pane, `MobileView = 'threads'`). Always say *thread drawer*, never a bare "drawer". The hamburger **menu drawer** (Files / Apps / Plugins / Triggers plus pinned *apps*, `Drawer.tsx` / `drawerOpen`) is a different surface.

On desktop the toggle rests top-left in the header, drawer open or not. In the Mac app it sits right after the window buttons. Its icon is a window with a left column; on a phone, a bulleted list. With the thread drawer open, Filter follows the toggle, and the grouping button and Search sit at the far end of its header. On a phone, Filter and the grouping button lead the header.

Filter shows under the *Folders grouping* only, and pressed only while the thread filter is open. Its icon is an outline funnel, or a filled one while thread types narrow the list. Under the *Ongoing grouping* it fades out where it stands, so the grouping button beside it never moves.

Switching grouping, or between the list and the thread filter, dips through the background: one view fades out, then the other fades in. The header title switches at once, and the button icons fade quickly. With motion reduced, every change is instant.
See also: *Conversation*, *thread pane*, *content pane*, *Current section*.

### Focused thread
The *thread* open in the *thread pane* on this device. A *draft* and the *Home thread* count; the empty compose view has none. An *app* or panel in the *Canvas* leaves it alone. Apps and widgets read it with `lucidos.ui.focusedThread()` and follow it with `lucidos.ui.onFocusedThreadChange()`, so a widget on the desktop *dashboard* sees the thread open beside it.
See also: *thread pane*, *dashboard*, `system-knowhow/js-sdk/lucidos-ui-ui-control.md` § Reading the thread the user has open.

### Thread pane
The pane showing the open *thread*'s transcript plus the prompt input, where you read what the *Lucidos Agent* did and type the next message. Second of the three panes, the other half of the *Conversation* side beside the *thread drawer*. CSS container `.pane-thread` (`FocusedPane = 'thread'`; on mobile the middle swipe pane, `MobileView = 'thread'`).
See also: *Conversation*, *thread drawer*, *content pane*.

### View menu
The Mac app's View menu. It opens with four items, one per pane: Thread Drawer, Thread Pane, Content Pane and Dashboard. Each reads "Show …" while its pane is hidden and "Hide …" while it shows, as Finder and Safari do. A click shows or hides that pane in the focused window, and on a phone-width window it swipes there.

Thread Drawer, Thread Pane and Content Pane show their pane shortcuts, and a rebind under **Settings → Keyboard Shortcuts** updates the label. The shortcut itself still runs its usual focus-or-hide steps. Thread Drawer greys out while the window is too narrow for it. Dashboard greys out while there is no room for the rail, or no *Home thread* yet.
See also: *thread drawer*, *thread pane*, *content pane*, *dashboard*.

### Thread Queue
System-wide admission control for the shared thread pool. Every path that creates running work shares one pool. Background spawns are an event *trigger* fire, a scheduled (cron) fire, or an agent-driven *sub-thread* or *coding-agent thread* spawn. Those come via `run_thread` / `run_coding_agent`, agent-mode `lucidos spawn-thread`, or cross-workspace task POSTs. User-initiated work is a person's chat or typed coding-agent thread. Within the *capacity policy* work runs at once; over capacity it waits.

User-initiated work is **prioritized, not exempt** (ADR 0008). It counts against the ceiling, drains ahead of background, and ignores the per-kind and per-trigger caps. It queues only at true pool-max, where a person briefly sees "requesting". `reserved_background` keeps that priority from starving triggers and cron.

Background ordering is FIFO, strict per trigger and best-effort across triggers. Cron fires **coalesce** to at most one entry per trigger: a cron fire carries no distinct payload, so a redundant one is dropped. A restart's duplicate cron rows collapse to one on recovery; event triggers keep strict FIFO.

Background entries persist in the `thread_queue` projection, event-sourced from `ThreadQueued` / `ThreadQueueAdmitted` / `ThreadQueueDropped` / `ThreadQueueCompleted`. So a restart re-queues work that never ran and drains it as capacity frees. User-initiated work counts while its thread is running, and is never re-fired after a restart. The **Thread Queue panel** shows it all (Running counts background + user; run now, drop, edit the capacity policy). A badly delayed trigger or a full pool raises notifications that tap through to the panel.
See also: `system-knowhow/thread-queue.md`, ADR 0008.

### Title style
One short instruction that shapes how the title model words automatic *thread* titles, for example "lower-kebab-case, at most 4 words". It is the `title_style` *preference*, stored per workspace, and it overrides the standard length and wording. Set it in **Settings → Models → Background tasks → Title generation**, or tell the agent. **Reset** deletes it and restores the standard titles. It shapes only titles written from then on, never existing names. See ADR 0586.

### Thread triage
The Lucidos Agent sorting the user's inbox threads into proposed actions, each with a reason drawn from facts, never from a title. It is the `threads` tool's `triage` and `apply_triage` actions. A *triage action* is one of `archive`, `pin`, `delete`, `follow_up` (needs the user), `dismiss_question` or `keep`. Trigger runs group per trigger: the newest is kept, older ones are proposed for archive, and an older run's lone question for dismissal.

`triage` records a `ThreadTriageProposed` event, and `apply_triage` runs only after the user replied to it in the same thread, re-checking every thread first. It never archives a thread that needs the user, and never deletes: a delete is the user's own, from the drawer (ADR 0192). See ADR 0349.

### Thread summary
A projection of a *thread*'s metadata: title, source, status, activity, links and coding-agent flags.
The engine derives `thread_summaries` rows from events and exposes them as `ThreadSummary`.
Read them through `GET /api/v1/threads/list`, `lucidos threads list`,
`threads(action="list")` or `lucidos.threads.list()`. The thread's event sequence remains
the source of truth.
See also: `system-knowhow/lucidos-cli/subcommands-2.md` § `lucidos threads list`, `system-knowhow/js-sdk/lucidos-threads-thread-management.md` § `lucidos.threads`.

### Self-curated context mode
An **experimental** way of running a chat or trigger thread, behind the `self_curated_context_mode` *preference*, off by default. It changes how long a tool result stays in front of the agent, and what the agent is told about its own context. The agent does the curating, never the engine. Older prose and ADRs 0085 to 0110 call it *context mode*.

**Tool results are swept away in batches**, each with the call that made it, and nothing stands in their place (see *swept window*). What the agent does instead is write its *working understanding*, the one thing that outlives a turn. To hold one item longer, it names that item's *handle* under `[KEEP OPEN]` (a *keep*). The one exemption left: the trimmer at the wall never cuts a result that errored. The *context panel* shows what is resident and what each item has left.

Everything else is unchanged. Long-term memory, the conversation history and every loaded knowhow doc ride in the prompt as with the mode off. The previous turn's tool calls are not re-sent, and the conversation summariser does not run. `todo_write` is withdrawn, since the checklist moved into the agent's working understanding.

The objective is focus, not a smaller bill: clutter is out of the agent's way. The cost is a re-fetch, a round to read back a result the agent needed and did not write down. Nothing is lost, since the event store holds it all.

*Coding-agent threads* are out of scope, since Claude Code and Codex build their own context. The *context-handling benchmark* measures whether the mode becomes the default. There is no bar to clear: ADR 0110 retired those, and a human reads the axes.

### Context panel
The `[CONTEXT PANEL]` block of a *self-curated context mode* prompt, appended to the newest message every round. It states how full the prompt is, as a percentage and a token count. Then what is resident, what is held open and its cost, and what a budget pass let go. Then the rounds until the next sweep, stated once. Then one row per addressable item: its *handle*, size, age in rounds, and rounds left.

On the round before a sweep it names the addresses that go next. That is their last round in front of the agent, where a rewrite of the *working understanding* is worth most.

It names the system instructions and tool definitions too. Nothing the agent writes reaches them, but without them the addressable total reads as the whole bill.

It replaced the *context ledger*, which showed no size, age or total (ADR 0109). It is appended, never edited into an older message: a cache breakpoint sits on the last message, so an earlier byte change rewrites everything after it.

### Round stub
Retired with the *swept window*: the line a tool result once became after its round. A pair now leaves whole, with nothing in its place. Distinct from a *budget stub*, which the trimmer still leaves at the wall.

### Body
Retired with ADR 0109: the droppable half of something assembled into the prompt. Memory, past turns and knowhow ride in the prompt again, so only a tool result ever leaves.

### Handle
The `evt-<hex>` address of the event something came from, stated on every tool result and every *context panel* row. Writing it under a `[KEEP OPEN]` heading makes a *keep*. `events(action="query", event_id=…)` reads the original back. A *working understanding* entry anchors itself with one, to say which result it is about.

Always the whole 32 hex digits. A shortened address resolves to nothing, so a keep or read-back with one quietly does nothing.

### Keep
The agent setting one item's clock back to zero, by writing its *handle* under a `[KEEP OPEN]` heading in its *working understanding*. Recorded as a `ContextKeptOpen` thread event. It applies once, from the reply that wrote it, so a later rewrite cannot re-assert it.

**A keep moves the clock and nothing else.** It exempts the item from no pass. The trimmer at the wall can always cut, so a keep never wedges the agent's turn. At the wall it buys ordering only: held items go last, but still go. There is no cap on held items; the panel states the bill. Silence holds an item until the sweep, so a keep is only for an item the panel shows running down that the agent still uses.

### Context ledger
Retired with ADR 0109, replaced by the *context panel*. It listed each *body* with its address and fetch-back call, but no size, age or total. The agent saw what it held but not how much, so it never curated.

### Scratchpad
Retired, replaced by the *working understanding*. Both the `[SCRATCHPAD]` block and its `scratchpad` tool are gone. The name invited brevity: against a cap of 8,000 chars, Opus averaged 1,150 and Sol 701.

### Working understanding
The agent's picture of the job under *self-curated context mode*: what it worked out, what it decided and why, what a result told it, what it ruled out, and what it means to do next. It comes back at the tail of every round, touched or not. "Working" means provisional, in use and current, true on a two-round thread and a 400-round one alike.

It is **ordinary text inside a reply**, between `[WORKING UNDERSTANDING]` and `[/WORKING UNDERSTANDING]`. It rides in the same reply as the next tool call, so it never costs a round. `[WORKING UNDERSTANDING: ADD]` opens the append form. The engine asks for an append on an ordinary round, and a whole rewrite when the panel says a sweep is next.

Three headings travel inside the block. `[CONSTRAINTS]` renders every round, empty or not, so a dropped convention is visible. `[TODO]` carries the *todo list*, the same list the user sees. `[KEEP OPEN]` carries one *handle* per line. Only the body and the constraints are stored; the other two are applied and dropped, so a rewrite cannot re-assert an old keep.

Raw content may be copied in, exact bytes included; there is no keep-a-slice verb. Past a soft threshold the block's header asks for a rewrite. Nothing is ever refused or truncated.

### Swept window
How long a tool result stays in front of the agent under *self-curated context mode*. Two numbers: `self_curated_context_expire_after_rounds` (5) and `self_curated_context_sweep_every_rounds` (10). Every tenth round the sweep takes everything past the expiry age, so an item lives 6 to 15 rounds, ten on average.

**The clear-out is a schedule, not a per-round drop.** Removing a pair mid-request invalidates every cached byte after it. Paying that every round costs about ten times what it saves, so nine rounds in ten are pure appends. The panel states each item's exact remainder, so the variable lifetime never surprises the agent.

**Silence keeps**: doing nothing holds a result until the sweep. Only a *keep* moves a clock.

Both numbers are provisional, on three grounds. Four in five tool results are never observably used after their arrival round. Of the fifth that are, 21 of 23 finish inside ten rounds. JetBrains tuned the same window to 10 turns on SWE-bench Verified.

### Todo notes
Retired: a free-text block the *Lucidos Agent* once wrote beside its *todo items* in the same `todo_write` call. No tool schema offers it now. Notes on older threads still show, and the engine carries them through untouched when it settles a list. The *working understanding* replaced it, with a result's lesson in its body, anchored to its *handle*.

### Todo item
One row of a *todo list*, with three fields:

- `content`: the imperative form ("Run tests"), which is what the item is.
- `active_form`: the present-continuous form ("Running tests"), shown only while `in_progress`; any other status renders `content`.
- `status`: one of `pending`, `in_progress`, `completed`, `waiting`, `abandoned`.

The first three statuses are LLM-writable via `todo_write`. `waiting` and `abandoned` are engine-only: the engine settles every open item when the thread stops working the list (see *Todo list*). Which one it writes tells the user an agent that parked from one that walked away. The item shape, minus those two, matches *Claude Code*'s `TodoWrite` items on purpose.

### Todo list
A per-*thread* list of *todo items* the *Lucidos Agent* works through during a response. The agent sets it with `todo_write`; each call replaces the whole list (at most 50 items, at most one `in_progress`). The prompt bar shows a collapsible `completed/total` indicator that expands to the list. Only the LLM writes it; the user reads but does not toggle. *Coding-agent threads* use *Claude Code*'s own `TodoWrite` rendering, inline on the tool-call step. Distinct from *intent* (the user's stable goal) and *scheduled task* (a cron job).

**Settling.** When a response terminates (`ResponseGenerated` / `ResponseCanceled` / `ResponseAborted` / `ResponseFailed`), the engine reads the latest `TodoListWritten`. An all-`completed` list is left alone and persists for the thread's lifetime. If any item is open (`pending`, `in_progress` or `waiting`), the engine emits a new `TodoListWritten` settling those items, completed ones untouched. The status it writes depends on the thread:

- **`waiting`** when the thread still holds a live *event wait*, or a *live child* still owes it a report: it stopped on purpose and something will re-open it or report to it. A `no-wake` child counts too, since it files its report for the next turn. Waiting rows keep full-strength text, a clock marker and a `waiting` tag.
- **`abandoned`** otherwise. Abandoned rows show a dashed strike-through and an `abandoned` tag.

A wait that resolves without the agent picking the list back up settles those items to `abandoned`. Usually that happens at the next terminator, since a delivery or a timeout re-opens the thread. **Stop waiting** re-opens nothing, so cancelling a thread's last subscription settles the list itself. Without that, an idle thread whose watch the user called off would read `waiting` forever.

Children work the same way. A child's completion wakes the parent, and that turn settles the list. A filed report, a stopped child and a child moved to top level re-open nothing, so the last of them settles the list itself. A child that dies with an error sends no card, so the parent's list reads `waiting` until its next turn.

The engine leaves the list alone on a cancel or a child card while a turn is live or promised. An example is the agent standing its own watch down mid-turn. The agent can still finish the list there, and `abandoned` is final. A later subscription never un-abandons an item. To avoid the settle, the agent finishes the list or drops it with `todo_write` and `[]`.

**The agent gets asked before it walks away.** A turn about to end with open items and nothing to re-open the thread is stopped once, before its answer is sent. A *live child* still on the edge counts as something. The agent then subscribes, finishes the list, or says it is not watching for anything. So an `abandoned` row usually means the agent chose to leave it. This stops a reply that promises to keep watching while nothing is.

**A settle is corrected by writing, never inferred.** The panel shows the agent's last list plus the engine's settling. So an item finished in a LATER turn reads `abandoned` until the agent calls `todo_write` again. The engine cannot see from the work that a plan item is done. So the tool tells the agent to re-write a list it picked back up.

**The agent can read the list.** Every `todo_write` call returns the resulting list, and `todo_write(action="read")` reads without writing. Both give each item's `content`, `active_form` and current status, engine-written ones included. A turn whose thread holds any non-`completed` item opens with the list in front of the agent. A thread with no list, a cleared one, or a finished one carries nothing. None of this applies under *self-curated context mode*, which has no `todo_write` and renders the checklist in the *working understanding*.

