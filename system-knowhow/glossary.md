---
name: Lucidos Glossary
description: Canonical user-facing terms: app, artifact, intent, knowhow, plugin, script, thread, trigger, workspace, event, and the coding-agent ones (Apply, change, Claude Code, coding agent, hardening). Load to disambiguate one, or when a concept seems to have two names.
---

# Lucidos Glossary

Canonical definitions for the words Lucidos uses with the user: one term, one place, one definition. Other prose (`system-knowhow/*.md`, the engine system prompt, app/trigger intents, UI strings) uses *this* meaning, never a synonym.

Use the canonical word, not a near-synonym: *sub-thread* for any descendant (not *child thread*), *intent* (not *task*), *knowhow* (not *recipe*), *artifact* (not *attachment*). Add a missing concept in the same change that introduces it.

Two sections: **Core** terms most users meet, and **Advanced (coding agents)**: Claude Code, worktree-based code changes, the Apply / Discard flow, hardening.

This file is the **base layer**. `docs/glossary.md` adds dev-only terms (aggregate, actor, ActorMode, EventBus, BusEvent, ThreadEvent, projection, worktree, …) that the workspace LLM doesn't need.

## Core terms

### Active device
A device currently reporting itself visible to the engine. On desktop: `document.visibilityState === 'visible'` AND `document.hasFocus()`. On iOS PWA standalone: `visibilityState === 'visible'` only (Safari leaves `hasFocus()` false even in the foreground).

The Tauri desktop client also requires the *native window* to be active (focused AND on-screen). WKWebView can't see macOS `orderOut:`, so a trayed window keeps `visibilityState='visible'` / `hasFocus()=true`. The AppKit state reaches the page via a `native-window-active` event instead.

When any device is active at notification time, no OS push fires anywhere: the active device gets the *in-app surface* via the `NotificationCreated` SSE channel. The PresenceCheck protocol decides this per notification, not a stale heartbeat window.

A device with an app in *fullscreen* (native or pseudo) is NOT active, since only the Lucidos shell can show a toast. It then matches an app in its own window, which always took the push.

See also: `system-knowhow/notifications.md` §§1, 3.

### Active (thread state)
A *thread* currently doing work (the system's turn), shown by the row's status icon, **not** by a separate section. It stays in the *Current section* and shows or drops the Active indicator in place. Contrast *attention* (the user's turn: a pending *change*, an awaiting answer, or a failure), shown by the *Blocked* and *Review* groups of the *Ongoing grouping*.

As a **query filter**, "active" is the **union** of the `running` and `waiting_for_user_answer` statuses. It is `--active` on `lucidos threads list` / `count`, and `active` on `GET /api/v1/threads/{list,count}`, the `threads` tool and `lucidos.threads`. That union answers "is anything busy?" wrongly, since a thread parked on a question waits on the human. Ask that with the *status filter* instead.

### Agent archive
An agent archiving a *thread* itself, through the `threads` tool's `archive` action or `lucidos threads archive`. It reaches only its own thread and its own direct *child threads*. It runs the same *cascading archive* as the Archive button, so the same states refuse it. A *pinned thread* is the one difference: an agent is refused one (`thread_pinned`), and a pinned sub-thread stays open while the rest goes (ADR 0312). A child is archived at once.

An agent's own thread is archived once its turn ends and it has settled, and a new message before then keeps it open. Lucidos never archives a thread by itself, not even after its change is applied (ADR 0310).

*Thread triage* is the one wider path: under a triage the user replied to, an agent may archive any inbox thread that needs nothing from the user (ADR 0349).

### Archive all
The **Archive all** action on the drawer's *Current section* header, under the *Folders grouping*. It archives only what is safe, so a thread that needs the user stays, and so does a running one. Needing the user means an unanswered question, a pending *change*, unproposed branch work, an unsent *draft* or a failed run, here or in a sub-thread. A *pinned thread* sits in Pinned and is never touched, and neither is any thread of its family, even below an archived one.

A confirm counts what goes and what stays, and the engine archives only the threads it listed, if they are still safe. Each kept family is counted with its reason, and its other threads beside it. Every count is threads not archived, sub-threads included, as the section badge counts them, so the two totals add up to the Current badge. The result toast offers **Undo**, which *unarchives* that batch (`ThreadUnarchived`). It is the owner's button only: an agent uses *thread triage* instead, through the same classifier (ADR 0349).

A thread in the Archive section comes back through **Move to Current**, in its thread menu and its composer row. It unarchives the thread and the sub-threads under it, as Archive took them. A thread stored archived but still shown in Current offers Archive instead.

### Action blocker
Why Archive or Delete cannot run on a thread, as one reason. The reasons are the home thread, a running turn, a question waiting for the user, a pending *change*, and a *held idle proposal*. Each of the last four also counts when a *sub-thread* holds it. When several hold, the strongest is named.

The thread menu shows a blocked Archive or Delete dimmed with that reason, never hidden. When sub-threads hold it back, the menu lists each one with its state, strongest first, and a tap opens it. A refusal toast offers **Show sub-thread**, which opens the first. The user resolves the blocker: answer, Stop, Apply, Discard, or Stop waiting. Nothing in the menu cancels it. The engine's refusal names the same blocker (`thread_lifecycle::action_blocker`, ADR 0378).

<!--gloss-app-start-->
### App
A user-installed mini-application with its own UI (HTML/CSS/JS) at `data/apps/<id>/`, plus optional *knowhow* / *intents* / *scripts* / *triggers*. Chat is not per-app: every conversation is a regular *chat thread*. While an app is open in the panel-overlay slot, its `manifest.json` and discovered context flow into the *Lucidos Agent*'s prompt. Quick edits use the agent's file tools; heavier edits spawn an *app coding-agent thread*. The *app manifest* is user-facing metadata; `knowhow/` and `intents/` are engine-facing context loaded while the app is active. Its interactive surface is the *app UI*.

An app folder has a kind: an ordinary app, listed in the apps panel, or a *widget*, shown in a thread and never listed.
See also: `system-knowhow/building-an-app.md`, `docs/taxonomy.md` § Apps.
<!--gloss-app-end-->

### App-icon badge
The unread-notification *count* painted on the installed app's **icon**: a PWA's home-screen icon via the web Badging API, or the Tauri macOS **dock** tile. The macOS client also shows the count in its **menu-bar tray-icon title**, always, even when menu-bar-only. Distinct from the *in-app surface*'s bell badge inside the Lucidos UI.

The install origin decides how many workspaces it counts. Two installs show the total across running workspaces: the **Tauri desktop app**, and a PWA from the **gateway**. The gateway serves the picker at `/~/` and each workspace at `/<slug>/`, both with an origin-wide manifest. One icon covers them all, so one workspace's count would hide the rest. A PWA from a **direct engine** origin shows that workspace's count only. The gateway HTTP-polls each running engine (it holds no database handle), so a stopped workspace contributes nothing.

The icon and the bell must never disagree *for the workspace on screen*. The OS also writes the icon from a push payload's `app_badge`, so the page **re-asserts** the count rather than writing only on change. It takes this workspace's share from the same live unread set the bell projects. While the app is closed, the icon can lag a read made elsewhere until the next push or open. See `system-knowhow/notifications.md` § App-icon badge.

An open page **mirrors the icon's number in-app** from the same computed, so the two cannot disagree. One mirror is a count badge at the **Lucidos mark**'s bottom-right corner, leaving the top corner to the sparkle and the engine-state badge. The other is a group of rows atop the **Lucidos menu**, one per workspace with unreads, each routing to its notifications view. The mark carries a count onto the thread pane and the threads drawer, where the bell never appears.

### App manifest
The metadata file for an app at `data/apps/<id>/manifest.json`. Holds name, description, icon and `reveal` (see *ready signal*): what the UI shows. A *widget*'s manifest also holds `kind`, `origin_thread_id` and `reusable`. **Not** loaded into the LLM context; operational knowledge belongs in `knowhow/`.

### App storage
Per-app, per-device key-value state an app keeps through `lucidos.storage.local` and `.session`, which stand in for `localStorage` and `sessionStorage` inside an app. The values live in the Lucidos shell's own browser storage, so they stay on one device. Each app sees only its own keys. An app awaits `lucidos.storage.ready` before its first read. State every device should share belongs in `lucidos.data` instead.
See also: `system-knowhow/js-sdk.md` § `lucidos.storage`.

### App UI
The iframe that renders an app's HTML/CSS/JS (from `data/apps/<id>/ui/`) in Lucidos's panel-overlay slot. Distinct from the *app*, the whole installed unit. "Open the app" means open its UI inline and make its chat the active conversation. "Refresh the app UI" means reload the iframe without changing chat context. The `navigate_ui` tool's `app-ui` target and the `AppUiRefreshRequested` event both mean this iframe.

### API caller
An external HTTP caller of `/api/v1/...` that did NOT identify as a known actor (`You`, `Lucidos Agent`, `Lucidos Engine`, `System`). It sends no `x-lucidos-device-id` (no browser session) and no `x-lucidos-agent-origin-token` (not a Lucidos-spawned subprocess). UI actor-chip label: "API caller", with a plug; the origin popover shows the User-Agent. The reserved label stops an anonymous mutating POST from impersonating the user as "You".

The venv agent-origin shim (`crates/lucidos-engine/src/runtime/python.rs`) auto-forwards the agent-origin token on Python calls to the engine port. So a `run_python` subprocess that hand-rolls `urllib.request` lands as `Lucidos Agent`, not here. The shim is a `.pth`-loaded `_lucidos_agent_origin` module that survives a host `sitecustomize.py` such as Homebrew's.

<!--gloss-artifact-start-->
### Artifact
A user-owned file under `data/artifacts/`. Git-tracked, never auto-deleted. Includes notes, imported API data, project folders, screenshots, generated images. The durable counterpart to ephemeral runtime state under `.lucidos/`.
See also: `system-knowhow/best-practices.md` § `artifacts/`.
<!--gloss-artifact-end-->

### Auth module
A WASM signer (plus optional `<name>.manifest.json` *signer manifest*) installed under `data/auth-modules/` to sign outbound proxy requests. Plugins can ship auth modules in their `auth-modules/` directory; the install-time LLM walks the user through wiring the matching `apis.json` snippet. Engine-side mechanics (host imports, capabilities, body modes) live under *signer* in `docs/glossary.md`.
See also: `system-knowhow/building-an-auth-handshake.md`.

### Blocking descendant
A *sub-thread* whose state currently prevents its ancestor from being cascade-archived: Running, paused on a user question (WaitingForUserAnswer), or a *coding-agent thread* with pending *changes*. A *held idle proposal* also blocks the cascade, but this count leaves it out. The menu then offers Archive, and the engine's refusal names the sub-thread. Counted in `blocking_descendant_count` on the thread aggregate; while it is non-zero, the thread menu shows Archive blocked with the *action blocker*'s reason.

### Attention-needing descendant
A *sub-thread* that needs user action to progress: paused on a user question (WaitingForUserAnswer), or a *coding-agent thread* with pending *changes*. A strict subset of *blocking descendants* that drops `Running`, since running work is delegated. Counted in `attention_descendant_count` on the thread aggregate. It bubbles the ancestor chain to the **Current** section, so the user sees the attention card while siblings still run.

### Canvas
The right-hand side of the side-by-side desktop layout, where the live system appears and you act on it: an open *app*'s *app UI*, a file or *artifact* preview, a *change*'s diff, settings, a URL. You direct the *Lucidos Agent* in the *Conversation*, and its output appears, live and usable, in the Canvas. Apps there read real workspace data through the SDK, so changes made from the Conversation show at once (see *Live co-creation*). On mobile it is one swipe away rather than alongside. Canvas names a **side**, not a pane; the single pane filling it is the *content pane*. Say "Canvas" for the side or the Conversation↔Canvas back-and-forth, and "content pane" for the pane itself (where a view lands, a shortcut, a resize).
See also: *Conversation*, *content pane*, *Live co-creation*.

### Build slot
One of N permits to run a heavy build on the host. A slot is an OS file lock in a machine-wide pool under `$HOME/.lucidos/build-slots/`. The build process holds it, so the kernel releases it on death and a killed build never wedges the pool. N resolves from `LUCIDOS_MAX_CONCURRENT_BUILDS`, then a capacity file beside the pool (`lucidos build-slot --set-capacity <n>`), then host RAM.

Over the limit a build waits rather than failing: `lucidos build-slot -- <command>` blocks until a slot frees, then runs the command as its child. It is **not** a queue: whoever samples a freed slot first takes it. Taken by `make lint`, `make test`, the e2e build phase, and the engine's own rebuilds; frontend commands are too cheap to gate. Without a `lucidos` binary on PATH the build just runs. Every release is announced as `BuildSlotReleased`, so a session that gave up on `--max-wait` and subscribed is always woken. `BuildSlotWaiting` and `BuildSlotAcquired` fire only under contention.

A slot also sets the build's CPU share. The holder runs at `nice +10`, inherited by the compile tree, with `CARGO_BUILD_JOBS` set to cores divided by slots held. The share never falls below `cores / capacity`, so a solo build keeps the machine and full contention divides it evenly. A caller's explicit `CARGO_BUILD_JOBS` wins, and `LUCIDOS_BUILD_SLOT_NICE` overrides the increment (`0` opts out).

The engine's own Apply rebuild waits as a **priority waiter**: ordinary builds leave a freed slot to it, and it runs un-niced. It never exceeds the count and never stops a running build. While it waits, the Lucidos menu calls it a **queued build** ("New version queued", not "Building new version"), with a still hourglass for the spinner.

It exists because each *coding-agent thread* has a *worktree* with its own `target/`, so parallel sessions mean N full compiles at once. Distinct from the engine build lock, which serialises engine builds inside one checkout's shared `target/`, and from the e2e lock, which hard-fails a second run.
See also: ADR 0070, ADR 0210, ADR 0304, `system-knowhow/lucidos-cli.md` § `lucidos build-slot`.

### Capacity policy
The configurable caps governing the *Thread Queue*. The fields:

- `max_concurrent_total`: the shared ceiling for ALL threads, background spawns and user-initiated work alike.
- Per-kind caps (event trigger / scheduled / sub-thread / coding agent), background only.
- Per-*trigger* concurrency, plus a per-trigger queue ceiling with a choice of overflow behavior. Overflow either drops the oldest waiting fire and notifies, or pauses the trigger and notifies.
- `reserved_background`: slots background can reclaim ahead of user-initiated work, so user priority can't starve triggers/cron. 0 = pure user priority.
- `max_event_trigger_depth`: how many trigger fires one *event chain* may make before the rest are stopped and the user is notified. Default 5. A spawn does not consume a hop, so a fire's sub-thread, coding agent or script runs at the fire's own depth.
- `max_concurrent_children_per_thread`: how many *live children* one thread may have at the same time. Default 10. `run_thread` is refused at the cap, and a finished child frees its slot. Not a pool cap: admission never reads it.

Concurrency caps of 0 mean "hold": admission pauses and the queue accumulates until the cap is raised. `max_queued_per_trigger`, `max_event_trigger_depth` and `max_concurrent_children_per_thread` must be at least 1. Edited in the Thread Queue panel (`PUT /api/v1/thread-queue/policy`). Stored event-sourced, so the latest `CapacityPolicyChanged` event IS the policy.

### Cascading archive
Archiving a parent *thread* also archives every *sub-thread* under it, atomically. Disabled while any descendant is a *blocking descendant*. **A thread waiting on your answer is never archived**, by any path: answer the question or press Stop first. A question asked in an archived thread moves it back to *Current*.

**An archived thread stays fully searchable**, and what Lucidos learned from it stays in memory. Archive only moves it to the *thread drawer*'s Archive section; *Delete (a thread)* removes it. See also *agent archive*.

### Chat thread
A *thread* whose `source = 'chat'`: the user typed the opening message in the Lucidos chat UI. Answered by the *Lucidos Agent*. Contrast with *trigger thread* and *coding-agent thread*.

### Home thread
One *chat thread* per workspace, titled "Home", that never ends. **Every workspace has one**, and there is no switch for it. On a new install, Lucidos opens Home first, with the welcome in it. A device that has opened a thread before opens that thread instead. It can be neither archived, deleted nor pinned.

**Never using it costs nothing.** Home works only when you write in it, call it, or a thread it started reports back. Left alone, it makes no model call and asks for nothing.

**You open it from Home, not from the thread list.** The Home icon in the thread pane's header opens it. On a phone it sits beside the Lucidos mark, and the menu that the thread drawer's mark opens carries Home as its first row. The *thread drawer* draws no row for it, but its *sub-threads* show in the drawer's sections as threads of their own.

**Only you name it.** You can rename it by hand, from its menu. Lucidos never names it: it gets no automatic title and offers no suggested name, because one topic in a thread that never ends is not its name.

**It reaches further than any other thread, with no more authority.** It reads any thread, and sends a *child follow-up* to any thread, *coding-agent threads* included. A thread it never spawned reports to its own parent, not to Home, so read its reply afterwards. It presses a button that belongs to you only while your words in that turn ask for it. That covers Apply, Archive and a permission card on another thread. The two widest "Always allow" grants are never an agent's to press.

*Voice sessions* live here and nowhere else.

**It also holds the cost of model calls no thread made**, such as a memory summary of a file written outside any thread. So the Token Cost app counts every call. Those calls run whether you use Home or not.
See also: *voice session*, *child follow-up*, *standing instruction*, *setup interview*, ADR 0362, ADR 0381, ADR 0411.

### Child follow-up
A message from a *parent thread* to one of its own *child threads*, sent with the `follow_up_child_thread` tool. The one privileged cross-thread write: redirect a child, hand it something a sibling learned, or tell a stalled child to continue. A thread can address its **direct** children and nothing else: not a sibling, not a grandchild (go through the child), and nothing cross-workspace. The engine looks the relationship up from the child's `parent_thread_id`. The one exception is the *home thread*, which may address any thread in its workspace.

A follow-up returns once the message is on the child's timeline and does **not** wait; the child reports back when its turn ends. It creates no thread, and the child cap never refuses it, so reviving a child is cheaper than spawning one. The revived child counts as a *live child* again while it runs. By default it **queues**: a mid-turn child reads it at its next natural break. To stop the child's current turn instead, see *urgent follow-up*.
See also: *child thread*, *parent thread*, *urgent follow-up*.

### Held message
An agent-sent message a *coding-agent thread* keeps back while it waits on a human: an open question, a pending permission card, or older held messages. Usually a parent's *child follow-up*, whose result reports `held`. The transcript shows it dimmed, saying it is not delivered yet, and the question stays answerable. On delivery, the delivered message replaces the held row, naming its sender and saying it was held.

An answer to the question or card releases held messages, oldest first, as does the user's next message or Continue. A Cancel keeps them held, since Cancel means stop. Events: `MessageHeld`, then `HeldMessageReleased` just before the ordinary `MessageReceived`. See ADR 0256.

A callback that lands under an open question dims too, on any thread: a returned child or an event-wait delivery. It says the agent has not read it yet. It is queued rather than held: the agent reads it right after the answer (ADR 0255). A queued message of yours dims too while a question blocks it.
See also: *child follow-up*, *read marker*.

### Held idle proposal
A *coding-agent thread* that ended its turn while waiting on an event, with work on its branch that no *change* carries yet. The agent is still working, so the engine holds the proposal until the wait re-opens the thread and the agent finishes (ADR 0395). Until then the thread cannot be archived or deleted. **Stop waiting** proposes the work at once.

### Child thread
A direct descendant *thread* created by a `relation: "child"` spawn (`run_thread` / `run_coding_agent` / `lucidos spawn-thread --relation child`). When the child terminates, the engine resumes the *parent thread* with its result, so the parent never waits. A crash that cuts the child's turn reports it as `interrupted`, since nothing resumes it. A child is a *sub-thread*; the reverse isn't true. A *child follow-up* from the parent earns another report when that turn ends.

Identifiers: the child row's `parent_thread_id` points up to the parent. `child_thread_id` names the child, on the `Callback` struct and the `ChildThreadCompleted` event alike.

**Two ends of a turn send no callback.** A user Stop makes the child a *stopped child*, and its parent gets a note instead. A turn that ends holding an *event wait* sends nothing, because the turn the wait wakes reports.

A child stops being one when it is *moved to top level*.

### Command guard
An opt-in safety gate over the *Lucidos Agent*'s shell/Python tools, under **Settings → Permissions → Command safety** (off by default). A fast static check settles the obvious cases, and a cheap LLM **judge** decides the ambiguous middle, erring toward asking. Most commands run untouched, including reads anywhere and writes inside the workspace.

Three lanes. A clearly catastrophic command (`rm -rf /`, a fork bomb, formatting a disk) is refused. An irreversible real-world side-effect, or destruction outside the workspace, pauses on a *command permission card*. An in-workspace deletion or overwrite runs after a *command checkpoint*, leaving a one-click Undo.

Two sub-settings apply while the guard is on. The **LLM judge** switch: off falls back to a static classifier (the dangerous-command list plus a destruction scan), so the lanes still hold. A judge that is on but fails, for example because its model is no longer served, asks instead, and the card says why. The **judge model** defaults to Gemini 3 Flash, chosen by measurement.

"Always allow" commands live in an editable list under **Settings → Permissions → Lucidos Agent permissions**. The list is **per workspace** (`<workspace>/.lucidos/agent-allowed-commands`), so another workspace asks again.

A *trigger* fires unattended, so it cannot be asked. It runs irreversible commands only within its declared *side-effect grant*, and an ungranted one is blocked and fails the run. See `system-knowhow/running-python.md` § The command guard.

### Command checkpoint
A pair of snapshots of the workspace's tracked content, taken by the *command guard* around an in-workspace delete or overwrite (the "reversible" lane). Instead of asking, the guard snapshots, runs the command, snapshots again, and shows a card with **Undo** and **Diff**. The two snapshots show exactly what the command did. Undo restores what it deleted or overwrote **and** removes files it created, leaving alone any you edited since. If the command changed nothing visible (a git-ignored target the snapshot never captured), no card appears. Snapshots are kept 30 days, then reclaimed.

Out-of-workspace destruction can't be checkpointed, so it takes the *command permission card* lane. Only taken while the *command guard* is on.

### Command permission card
The approval card the *command guard* shows when a shell/Python command needs the user's go-ahead. Same UI as the *coding-agent permission card*: Deny, Allow once, Allow for this thread, or Always allow (remembered for similar commands). The thread waits until answered. Unlike the coding-agent card, "Allow for this thread" here is forgotten if Lucidos restarts.

### Compose destination
The compose view's single "who/where" picker for a new *thread*. It is the *Lucidos Agent* (default, a *chat thread*), or a coding target that spawns the matching *coding-agent thread*: the Lucidos source, an *app*, or a registered *repository*. A one-line caption states the pick's consequence. The Lucidos Agent can hand off to a *coding agent*. A coding target produces a reviewable *change*, except an *external-repo coding-agent thread*, which reviews the diff from the thread.

The Claude Code vs Codex pick is a separate chip, shown only for coding targets. It is remembered per workspace (`coding_agent_default` preference) and locked at the thread's first message.

The Lucidos source target appears only on a dev build with a source checkout. A packaged install hides it, gated on the `/health` `packaged` flag. The *Lucidos Agent* follows the same signal: its system prompt states whether platform source exists. `run_coding_agent` with `folder` omitted is refused without it, so picker and agent always agree.

### Config
Workspace configuration files under `data/config/`, principally `apis.json` (proxy entries, signer wiring, OAuth flows). Users edit these directly or via the engine's auth-handshake flow.
See also: `system-knowhow/building-an-auth-handshake.md`.

### Connected account
A service the user has signed in to, so Lucidos can act on their behalf: the
stored result of an OAuth authorization (access token, refresh token, granted
scopes, and the account's email where reported). Listed under
**Settings → Accounts → Connected accounts**, one row per provider. Created by
the *Lucidos Agent*'s `connect_oauth_account` tool or that page's Connect button.
Both open the provider's authorization page in the user's configured browser
(in-app panel, system browser, or new tab) and store the tokens on return.

A connected account is a **sign-in**. The OAuth Client *credential* beside it is
the **app registration** (`client_id`, optional `client_secret`, endpoint URLs)
that made it possible. So one provider shows one row in each list, which is not
a duplicate. The Connect flow creates the registration, prefilled from the
*OAuth provider registry*, and saving it continues straight into the browser. A
provider name may be a *derived provider*.

It records **two** scope sets: **granted** and **asked for**. They differ when a
provider refuses part of a request, which is a real state, not an error. An
example is a Dropbox app whose Permissions tab lacks a scope. *Reconnect*
re-requests the asked-for set, since the granted set holds only what the account
already has.

Backup uploads read the connected account for their `backup_provider`. The Backup
page links here, handing over the provider AND the scopes an upload needs, so one
authorization covers both.
See also: *credential*, *OAuth provider registry*, *derived provider*,
*OAuth client type*, *OAuth redirect URI*,
`system-knowhow/oauth-providers.md`.

### Connected-but-hidden
A device whose Lucidos page is alive (SSE EventSource streaming) but not *active*: another tab is selected, the window is behind another app, or the iOS PWA is in the app switcher. It receives the `NotificationCreated` SSE message and updates its bell badge silently, with no toast. Eligible for an *OS surface* push, subject to global suppression in §2 of `system-knowhow/notifications.md`. Distinct from *Offline*, where there's no SSE at all.

### Content pane
The pane where an opened thing lands and runs: an *app*'s *app UI*, a file or *artifact* preview, a *change*'s diff, settings, a URL. The single pane filling the *Canvas* side, third of the three panes beside the *thread drawer* and the *thread pane*. CSS container `.pane-content` (`FocusedPane = 'content'`; on mobile the rightmost swipe pane, `MobileView = 'content'`).
See also: *Canvas*, *thread drawer*, *thread pane*.

### Conversation
The left-hand side of the side-by-side desktop layout, where you talk with the *Lucidos Agent* to direct the work. The Conversation is where intent is expressed; the *Canvas* is where the result lives and runs. Like Canvas, it names a **side**, not a pane, but it covers **two** panes: the *thread drawer* (your threads) plus the *thread pane* (the open thread's transcript and prompt input). On mobile you swipe between Conversation and Canvas.
See also: *Canvas*, *thread drawer*, *thread pane*, *Live co-creation*.

### Current section
The *thread drawer* section holding the live working set: every *thread* not pinned or archived. That covers running threads (shown by the *Active* row indicator), threads awaiting the user, and recently idle ones. A thread never jumps sections as turns pass. Current is ordered by creation time, newest first, and never reshuffles as agents work.

Threads that need you are NOT bubbled up. The *Ongoing grouping* lists them instead: *Blocked* for an awaited answer or permission or a failure, *Review* for a ready *change* or a *read request*. The Blocked count rides the drawer header's grouping button and the thread-drawer toggle badge.

The drawer's other sections are Pinned and Archive. "Pinned" is the user-facing word; the section key `is_saved` and the `ThreadSaved`/`ThreadUnsaved` events still say "saved".

### Context window
How many tokens a *model* can hold in one request, prompt plus reply. The engine
reserves room for the reply, then trims the oldest *conversation* history and the
largest tool results until the rest fits. A window set too low throws away context
the model could have held.

Each *model registry* row may declare its window (Settings → Models → Context
window). Left blank, the engine guesses from the model id. The guess knows only
Claude and GPT-5 ids and treats everything else as 200k. So an OpenRouter, xAI,
Gemini, or local model is under-budgeted until you set it. Every guess errs low on
purpose: too low only trims early, while too high builds a prompt the provider
rejects. Builtins declare theirs where it could be verified.
See also: *Model registry*, *Provider*.

### Credential
A secret Lucidos stores on the user's behalf: an API key, bearer token, username
and password, mailbox password, a plain **secret**, or an OAuth client
registration. Listed under **Settings → Accounts → Credentials**, keyed by a
**service name**. Every subprocess Lucidos spawns gets it as `CRED_<NAME>`, plus
an optional custom env var name as an extra alias. The proxy auth pipeline
(`data/config/apis.json`) also resolves it to sign outbound requests. Distinct
from a non-secret *environment variable*.

A credential is identified by its service name **together with its auth type**.
That matters for one pair only: an OAuth Client registration may share a name
with an ordinary credential. So `google` can be both an API key and the Google
app registration, two rows told apart by their type badge. Every other name is
unique, because `CRED_<NAME>` and `apis.json` resolve it. The two engine-owned
types once used an `oauth:` / `email:` name prefix. The type carries that now,
so the name is just the provider or the mailbox account.

An OAuth Client is the one type NOT injected as `CRED_<NAME>`: only the OAuth
flow reads it, from the database. A secret is never a *preference*.

**The `secret` type names no transport.** Every other type says how the value is
sent (a key, a bearer token, basic auth, an app registration). A `secret` is a
shared secret something signs with, so it takes no base URL and no header. A
*webhook* signing secret is one; generating it in **Settings > Webhooks** saves
it here.
See also: *connected account*, *environment variable*, *config*, *webhook*.

### Cross-gateway link
A link to a *thread* in a *workspace* your OTHER Lucidos install serves. Most machines run one install, where no link is cross-gateway. A machine running the packaged app beside a dev checkout runs two, each with its own workspaces.

It opens in a tab on that install's own address, port and all. Pairing is per gateway (see *paired device*), so a browser that install has never met lands on its pairing screen. You reach it from the machine, or wherever you reach Lucidos on its own port. A forwarded address (`tailscale serve`, an ssh tunnel) has only one install on the far end. There the link says where the workspace lives instead of opening a tab that cannot load. A workspace neither install knows still says it is not available.

### Draft
What you typed into a *thread*'s composer and have not sent: text, images, or both. It lives with its thread and follows you to your other devices. A never-sent thread is all draft and shows in the **Drafts** group of the *Ongoing grouping*. A thread with history can hold one too, archived or not.

The *Lucidos Agent* reads drafts and never writes them. The `threads` tool's `drafts` action lists each with a preview, last edit time and owning thread, and returns one whole by `thread_id`. `lucidos threads drafts` is the same read. A draft has no link of its own, so the agent gives its thread's *thread link*. Distinct from a *held message*, which an agent wrote and Lucidos keeps back.

### Disabled tool
One tool on an *MCP* server that the user switched off, so the *Lucidos Agent*
is never offered it. The rest of that server's tools keep working.

Every enabled tool's definition rides on **every** request, so a forty-tool
server costs tokens each turn even when the agent needs two. Switching one off
removes its definition, and the per-request token total drops. A call already
under way is refused too, so the switch takes effect at once.

The selection is stored per server, keyed on the name the agent sees, and
survives restarts. A change is announced, so the timeline records who narrowed
the tool surface and when. A tool the server later renames no longer matches, so
a stale entry disables nothing rather than the wrong thing.

### Derived provider
A provider name that is not a service itself but a second, separately scoped
connection to one. An example is a health-only connection on Google's endpoints
under its own name, held apart from the everyday *connected account*. Some APIs
require this, refusing any token that carries unrelated scopes.

It gets its own *credential* and connected-account row, and runs on the base
provider's endpoints. Aliases are ad hoc, so a derived name is absent from the
*OAuth provider registry* and never guessed from its spelling. The Connect form
asks which known provider it runs on, then fills in those endpoints and keeps
your name.
See also: *connected account*, *OAuth provider registry*.

### Delete (a thread)
Permanently removing a *thread*, its *sub-threads*, everything said in them, and what Lucidos learned from them. *Archive* only moves a thread to the Archive section, still fully searchable. Delete cannot be undone, though an earlier backup still holds it. Code already applied from a *coding-agent thread* stays; unapplied branch work goes, with the thread's *worktree* and branch. The thread's own *widgets* go too, in one commit. *Reusable widgets*, apps made from a widget, and other threads' widgets stay.

Offered to the workspace owner only, in the thread's own ⋯ menu. **Never to the *Lucidos Agent* or a *coding-agent thread***: no tool, CLI verb or SDK method exists. The route refuses any caller that is not a signed-in device, even one carrying the owner's *standing instruction*.

It cascades like *cascading archive*, with the same refusal: no member may be running, waiting on an answer, or holding a pending *change*. One confirmation names how many threads go. It also offers **Archive instead**, wherever Archive is still available.

Distinct from discarding a draft, which throws away a thread that was never sent.
See also: `docs/adr/0192-thread-delete-is-the-one-sanctioned-removal.md`.

### Domain event
An *event* the workspace itself emits via the `emit_event` LLM tool or `lucidos events emit` CLI: anything observable about the user's world (`MorningRoutineCompleted`, `JobListingFound`, `PanasonicHeatpumpAdjusted`). Persisted with the inner event type (not the literal string `"DomainEvent"`). It always flows through the trigger matcher, so a *trigger*'s `on_event:` can subscribe to any domain event name. Persisted `ThreadEvent` variants are also subscribable, except the per-token streaming ones and the side-question events. See *scheduler blocklist* (dev).

**The name must be your own.** Every engine event name is refused, `SystemEvent` and `ThreadEvent` alike (including legacy spellings like `Thinking`). A domain event's `aggregate_id` is its event TYPE, where a thread event's is a thread uuid. So a borrowed name writes a permanent row that breaks any query reading the name as an id.
See also: `system-knowhow/thread-events.md` § "Today the scheduler uses a blocklist", the Event APIs section of `.claude/rules/rust.md`.

### Dynamic bars
On a phone, the header, the thread title and the prompt glide away as you scroll down a thread. They glide back as you scroll up or swipe to another pane. The header returns near the top of the thread, the prompt near the end, and the prompt stays put while you type. A reply never moves them, even while the follow toggle carries you through it. Off by default. Set in Settings > Appearance & Behavior > Mobile, or the `mobile_dynamic_bars` preference.

### Endpoint catalog
The knowhow half of a *derived proxy entry*: a `data/knowhow/<name>-api.md` file listing the endpoints observed on a site, each with its params, response shape and quirks. The `apis.json` entry beside it is pure transport and names no paths. Without the catalog the LLM knows only that a proxy exists, so a derivation that emits one without the other has failed. It never records the user's own rows, only field names and types.
See also: *derived proxy entry*, `system-knowhow/deriving-an-api-from-a-site.md`.

### Derived proxy entry
A proxy entry obtained by watching a site's own frontend, for a site with no usable public API. The user drives the site once in a visible browser while its calls are captured. The result is an `apis.json` entry plus an *endpoint catalog*. It replays the user's own authenticated session and never bypasses anything: a CAPTCHA or bot wall stops it. Any secret found during capture goes to the engine's credential store, never into either artifact.
See also: *endpoint catalog*, *credential*, `system-knowhow/deriving-an-api-from-a-site.md`.

### Engine
The process serving one *workspace*: it holds the threads, answers the app, runs triggers and scheduled tasks, and talks to its database. One per workspace, started by the *workspace gateway* (dev term) and addressed through it. "Cannot reach the dev engine" means this client got no answer from that process. The workspace is not gone, and the engine may be healthy: a client can lose the route to it. The picker and the Lucidos menu's Workspaces row reach the gateway instead, so listing and switching keep working. Settings → System → Overview shows its version and restarts it.
Contrast with *Lucidos Engine*, the actor chip on work the engine did without the LLM.

### Environment variable
A user-managed, **non-secret** `NAME=value` pair (Settings → System → Environment variables). Lucidos injects it into every subprocess it spawns: `run_bash`, `run_python`, background tasks, scheduled scripts, *triggers*, and *coding agent* sessions. Examples: `CLAUDE_CODE_USE_VERTEX`, `LUCIDOS_REPO`, build flags, default model names.

Stored in the `environment_variables` table. Edited in Settings or by the *Lucidos Agent* via the grouped `env_vars` tool (`list` / `set` / `delete`; `set_environment_variable` is a back-compat alias for `set`). Applied per spawn, so a change takes effect on the next tool call or agent turn.

Distinct from a *credential*: env vars are non-secret and appear in tool-call payloads, logs, and the *event* store. Credentials hold secrets and feed the proxy auth pipeline. Names are uppercase letters, digits and underscores, not starting with a digit. They may not clobber engine-owned names (`CRED_*`, `OAUTH_*`, `PG*`, `PATH`, internal `LUCIDOS_*`), which always win a collision.

A credential can take a custom env var name, so its secret also injects as, say, `GITHUB_TOKEN`. That is an extra alias **in addition to** `CRED_<NAME>`, which keeps working. An auth handshake script never gets the custom name, only `CRED_*` and `OAUTH_*` names.

The engine also copies every pair into its own process environment once at startup. So a variable the engine itself reads picks up a change only on the next engine start.

<!--gloss-event-start-->
### Event
A past-tense fact about something that happened in the workspace, named in the past tense even when transient. Persisted events are written to the `events` table, replay, drive projections and match triggers. Transient events go over SSE only and never reach projections or the trigger matcher. Subtypes: thread lifecycle events (`MessageReceived`, `ResponseGenerated`, …), system events (notifications, preferences, …), and *domain events*. There is no *command* concept: anything imperative becomes a request event (`AppUiRefreshRequested`, not `RefreshAppUI`), and a subscriber chooses whether to act.
<!--gloss-event-end-->

### File preview modal
A read-only view of one file, drawn over whatever the *content pane* shows, without navigating. An *app* opens it through `lucidos.ui.previewFile`, and a file link in an agent's reply opens it too. A reader following a citation glances at the file and keeps their place.

A link's `#L510-L520` suffix, or a `:510-520` ending its label, names the lines. It takes the same locators and `line` / `line_end` as the `file` navigation target (a workspace data path or `repo:<repoId>:file:<path>`). It shows the same highlight and line numbers as the content pane's preview, plus a link that opens that full preview. Esc, a click outside, or its close control dismisses it. The *content pane*'s file preview IS a navigation: it replaces the pane's view and lands in the Back history.
See also: `system-knowhow/js-sdk.md` § lucidos.ui.

### Find bar
The bar that finds text in what a pane shows, and steps a highlight through the matches, scrolling each into view. Mod+F opens it on the focused pane, and Mod+F again or Escape closes it. It searches what is shown, not files on disk (Search Everywhere's Text section does that).
- **An *app*:** the header's search button. The app must load the SDK, which runs the search inside the app (`system-knowhow/js-sdk.md` § Find in app).
- **A file preview:** the header's search button, on source, Markdown, CSV, slides, a diff and an HTML artifact. Not a PDF, an image or the editor.
- **The *transcript*:** "Find in thread" in the thread's title menu. It searches every message and reply in the thread, older history included, but not tool output. Typing counts the matches. Enter jumps to the first one below where you are reading, unfolding its turn.

Where nothing can be searched and no find bar is open, Mod+F keeps the browser's own find.

### Font catalog
Every font Lucidos offers, one entry each: its id, label, CSS stack, kind (`ui` for proportional, `both` for monospaced, `mono` for code only), group, source and license. The group is `sans`, `serif` or `mono`, read from the stack's final generic family, and Settings lists fonts by it. The source is `vendored` (served by the engine, works offline) or `device` (the device's own fonts). No font loads from the internet.

The `font-family` *preference* takes a font id or `theme`, which follows the *theme*'s suggestion. A theme may name any catalog font. The engine serves the catalog at `GET /api/v1/fonts`, followed by any *workspace font*. See `system-knowhow/themes.md` § Fonts.

### Form request
Something the agent put in front of the user to fill in or confirm: a credential form, a plugin install or uninstall panel, an email confirmation, or an OAuth authorization page. It is persisted, so it survives a reload or a dropped connection. It stays open until answered, cancelled, or replaced by something newer, and one `FormRequestResolved` records which. An open one shows in its thread with an Open button and leaves the thread's status alone.
See also: `system-knowhow/thread-events.md` § Form requests.

### Event address
How one event in the store is named to an agent, written `evt-<32 hex>`. The hex is the event's own id, so the address is stable forever and resolves to exactly one row. Every live tool result ends with the address of the `ToolCalled` behind it, and a resumed tool block carries the same string as its `tool_use_id`. Two readers take it: the `events` tool's `query` action with `event_id`, and a `[KEEP OPEN]` line in the *working understanding*. A bare uuid, hyphenated or simple, works too.

It lets the agent note where something lives before a sweep drops it, then read it back. So reading a tool call's address returns the pair, call then result, since the result is the half that went.

### Image handle
The stable address of an image already in a thread, written `img-<hex>`. Derived from the image's content, so it names the same picture for as long as it exists. Contrast `thread:N`, a *position* that renumbers whenever an earlier image turns up. Every tool taking an image reference (`view_image`, `save_thread_image`, `generate_image`'s `input_images`) accepts both. The conversation history's image note shows it beside `thread:N`, and the label on images attached to the current message shows it too. So the agent copies a handle rather than counting, and can note one for a later turn.
See also: `system-knowhow/best-practices.md` § Images posted in a thread.

### Image size hint
A trailing `#<width>x<height>` on a markdown image's source, in image pixels: `![mockup](artifacts/x.png#1600x1200)`. The thread uses it to hold the picture's box before the picture loads, so a card or reply does not jump. `lucidos data write` prints it on the picture line, and the engine adds it to every workspace picture on a question card. Keep it when you paste the line. Any other fragment is left as written.

### Imported
The `data/imported/` directory where imported external repositories land (via `RepositoryImported` events). Treated as *artifacts*: content is flattened into the workspace's git tree, not kept as nested git repositories. Distinct from a *repository*, a separately registered external git repo that an *external-repo coding-agent thread* runs against.

### In-app surface
The notification surface inside the Lucidos UI: the bell badge (unread count, top bar) and transient toasts. Driven by the `NotificationCreated` SSE message on a connected page, which decides locally from its own visibility, focused thread, and viewport. Independent of the *OS surface*: a notification can hit either, both, or neither (when auto-marked read on the *source event*).

The `notification_toasts` preference (workspace-wide, on by default) silences toasts. The bell badge cannot be silenced, so the notification still counts and waits in the Notifications panel. Toasts off does not hand it to the *OS surface*: the device is present, so the push stays withheld.
See also: `system-knowhow/notifications.md` §§1, 4, `system-knowhow/preferences.md`.

### Intent
What the user wants, in their words: stable, non-technical prose the LLM could read back to the user without sounding like a script. Lives in `data/apps/<app>/intents/<name>.md` or, for triggers, the `TriggerCreated` payload's `run.intent` field. Any length; never imperative *how* verbs (hit, parse, retry, fall back), which belong in *knowhow*.
See also: `docs/taxonomy.md` § Intent vs Knowhow, `system-knowhow/intent-registry.md`.

### Knowhow
How to achieve an *intent*, in technical terms: API details, data formats, quirks, workarounds, fallbacks. Evolves every time Lucidos learns something new: the *Lucidos Agent* writes and updates it as it learns. Lives in `data/knowhow/<id>.md` (shared) or scoped to an app / trigger. Discovered at runtime by the LLM via the `load_knowhow` tool, matched by the file's frontmatter `name` + `description`. Every knowhow file is either a *knowhow doc*, listed for routing, or a *knowhow reference*.
See also: `system-knowhow/building-knowhow.md`, `docs/taxonomy.md` § Intent vs Knowhow.

### Knowhow doc
A *knowhow* file the engine lists for routing. Every thread's Know-how list names it, with its id and description, so the LLM can `load_knowhow` it. Placement decides which files count: `data/knowhow/` and the shared `~/.lucidos/knowhow/` list `<name>.md` and `<group>/<name>.md`. An app's or a trigger's own `knowhow/` lists `<name>.md` only, since the app or the trigger is already the group. Anything deeper is a *knowhow reference*.
See also: `system-knowhow/building-knowhow.md` § "Where the file goes".

### Knowhow reference
A knowhow file that belongs to one *knowhow doc*: a long endpoint table, a payload dump, an error matrix. It sits in a folder named after the doc, below the listed depth, so it takes no row in the routing list. Placement is the whole distinction. A reference keeps its full id and `load_knowhow` still reads it. Nothing routes to one, so the owning doc must name its id, and the workspace audit flags a reference no doc names.
See also: `system-knowhow/building-knowhow.md` § "Where the file goes", `docs/taxonomy.md` § "Knowhow: Docs and References".

### Last used device
The *device* of your newest action in the current turn: the message that started it, a prompt sent while it runs, or an answer to a question card. Start a turn on your phone and answer from your laptop, and the laptop is the last used device from then on. `navigate_ui` sends a file, app or page there unless the agent names another device, and a sign-in page opens there. `capture_app` and `refresh_app` reach only that device too. A turn with no device, such as a trigger run, has none, so a navigate reaches every device showing the thread.

The agent's `[USER DEVICE & PREFERENCES]` block names it from the same lookup, so the two agree. The block is a snapshot from when it was built; a coding agent gets a fresh one with every message you send.

Not an *active device*, which is one showing Lucidos right now, asked fresh per notification. Several devices can be active, but a turn has one last used device.
See also: *device*, *active device*.

### Live child
A *child thread* that has not finished its turn, so its *parent thread* is still owed a result. It is running, waiting for your answer, or paused for a resume the engine promised. It may also hold an *event wait*, or wait in the *Thread Queue* for capacity. An idle or failed child is not live.

Live children are what the child cap counts: a thread may have `max_concurrent_children_per_thread` of them at the same time (*capacity policy*, default 10). A finished child frees its slot. A child *moved to top level* still counts while it is live, so a move cannot buy back a slot. A *child follow-up* is never refused at the cap. See ADR 0380.
See also: *child thread*, *event wait*, *capacity policy*.

<!--gloss-live-cocreation-start-->
### Live co-creation
The principle at the heart of Lucidos: you and the *Lucidos Agent* shape the whole living system, **data and presentation together**. You do it continuously and in place, with no build → deploy → observe gap. The *Conversation* and the *Canvas* are always live and a gesture apart: side by side on desktop, a swipe apart on mobile. The Canvas runs on the real workspace, since apps read live data through the SDK. So you research, build and iterate inside Lucidos in one motion, seeing real behavior at once. The Conversation↔Canvas back-and-forth is the surface of live co-creation; the depth is that one conversation reaches the entire stack.
<!--gloss-live-cocreation-end-->

### Lucidos Agent
The LLM driving a *thread* on the user's behalf: chat responses, trigger-thread runs, sub-thread callbacks, anything the LLM authored. UI actor-chip label: "Lucidos Agent". Returned by `mcp_client_name(ActorMode::Agent)` in `crates/lucidos-engine/src/mcp/client.rs`. Contrast with *Lucidos Engine*.

### Lucidos Engine
The engine itself acting without LLM mediation: recovery sweeps, *hardening*, scheduler ticks, system-initiated cancellations. UI actor-chip label: "Lucidos Engine". Returned by `mcp_client_name(ActorMode::Engine)`.

### Marketplace
A registered git repository (or GitHub tree URL) that the *Plugins panel* scans for installable *plugins*, shown when the **Installed only** filter is unchecked. Stored in `data/config/plugin-marketplaces.json`; added or removed under Settings → Marketplaces (or the `register_plugin_marketplace` tool). It holds one plugin at its root or several plugin directories; GitHub subdirectories become GitHub tree install URLs.

The engine scans marketplaces at startup, after registration changes, and every five minutes. Registering, renaming and removing one are announced, so an open *Plugins panel* and Settings → Marketplaces update in place. A newer version of an installed plugin raises one deduplicated "updates available" notification and is never applied automatically. The user applies it from the *Plugins panel* (with **Installed only** unchecked).

### Max tool calls
How many tool calls the *Lucidos Agent* may make in one turn before the engine ends it. Set under **Settings → Models → Chat & triggers**; the `preferences` knowhow states its default and its bounds (`max_tool_calls`). It counts calls, not replies, and applies to *trigger* runs exactly as to chat. The bound is far above any real turn, so the cost in time and tokens is the user's call.

Reaching it is not an error. The turn ends with a message prefixed `[ENGINE-LIMIT]` that names the limit and links to the setting; any message continues it. That prefix is the only trustworthy signal, since the agent cannot see its own call count and will otherwise invent one. Only the user can change the cap, the backstop over the agent's own work. Distinct from the *command guard*, which judges whether one command is safe.

### MCP server
An outside program that offers the *Lucidos Agent* extra tools over the Model Context Protocol: a Slack server, a Jira server, a company's internal catalog. Registered per workspace and managed under **Settings → MCP Servers**, which lists each with its cost.

**Nothing starts an MCP server when the engine starts.** A server runs for the current session only, and a restart switches them all off, hence the label "Running, this session". A **stopped** server's tool list is cached from its last successful connect. The page states that cost conditionally, stamped with when the tools were last seen. A server never connected says so instead of showing zero.

Every enabled tool's definition rides on every request, so a forty-tool server is a permanent per-turn tax. Two levers: switch the server off, or switch off a single *disabled tool*. The page also holds the allowlist behind the *MCP permission card*.

**The running servers share a quarter of what a request can carry.** Past that, each server keeps the tools that fit, in its own listed order. The rest are left out of requests, and the agent is told which. The page states the share of the request and warns before anything is cut.

**Starting or stopping one reaches a turn already running.** The agent can start a server mid-answer and use its tools in that answer. Switch a server off mid-turn and the agent's very next step is already without its tools.

A server whose stored id cannot be used on the wire shows as unusable and offers only Remove; nothing on it can be called. See *wire tool name* in the developer glossary.

### MCP permission card
The approval card the *Lucidos Agent* shows before calling a tool on an untrusted *MCP* server. Same UI as the *command permission card*: Deny, Allow once, Allow for this thread, Always allow this tool, or Always allow this server. "Always allow" choices go in an editable per-workspace list (`<workspace>/.lucidos/mcp-allowed-tools`). Entries are per tool (`Mcp(<server>:<tool>)`) or whole server (`Mcp(<server>:*)`). The thread waits until answered. A *trigger* never shows this card: its MCP calls are auto-approved silently, as is any call to a server with auto-approve set.

### Memory module
The workspace setting for how a turn gets its past, `memory_module` (ADR 0362). **Classic**, the default, is the conversation summariser, *memory recall* and *memory search*. **Tree** gives each turn two memory views: one over every thread and artifact write, and one of the thread itself. It also gives the agent the `recall` tool, which opens any line down to the exact message.

The user picks it under Settings → System → Memory. Choosing Tree there first shows what its one-time background summarisation would cost, and starts it only on Start Tree. Turns stay on Classic until the workspace and the last week's threads are summarised, and a progress bar shows how far it has got. Older threads fill in after. On Tree, the page also lets the user browse those summaries, line by line, down to the exact message. `preferences.md` § The memory module has the detail.

### Memory recall
Part of the Classic *memory module*. The engine reaching into long-term memory **for** the *Lucidos Agent*, automatically, before a turn starts. A classifier splits the user's message into sub-queries, the engine vector-searches memory with them, and the hits enter the turn's context. The transcript shows a step reading "Recalled 12 memories" (or "No memories recalled"). Tapping it lists each memory, linked to its source, above the sub-queries that found them.

Recorded as the `MemoryRecalled` *event*, which a *trigger* can subscribe to. Recall happens to a turn; a *memory search* is something the agent decides to do.

### Memory search
Part of the Classic *memory module*. The *Lucidos Agent* searching long-term memory itself, mid-turn, with its own query: the `memory` tool's `search` action, shown as "Searching memory for ...". It backs up a *memory recall* that missed, since recall runs once, from queries made before the agent read anything. Both rank the same corpus the same way, so a search never reorders facts already in context.

### Model registry
The database-backed list of chat models the user manages in **Settings → Models**. It drives the *Lucidos Agent* model picker and tells the engine which *provider* serves each model. The engine seeds known models. The user can add, enable, disable or delete models (builtins are disable-only). Adding one takes an id, a label and a provider, plus an unusual *context window* and the *vision flag*. Each row also carries its *default effort*, which the engine seeds and the user cannot edit.

A model listing several providers is served by whichever the workspace has credentials for: see *preferred provider*. Separate from the *Claude Code* model picker, which keeps its own list.

### Vision flag
Whether a model reads images: the image icon on each row in **Settings → Models**, the `vision` field of `manage_models` and `lucidos models`. It is off unless declared. Image description offers, defaults to and runs only models that have it, and a set `model_image_description` without it is refused rather than replaced. The engine seeds it on the Claude, Gemini and GPT-5/6 builtins. Any row, builtin included, can be switched, since a seed can be wrong. A model with no registry row has no flag.

### Motion
How much Lucidos moves on one device: **Settings → Appearance → Motion**, the device-scoped `motion` *preference*. **System** follows the device's reduce-motion switch. **Reduce** calms the app regardless: nothing slides, pulses or spins, and changes appear at once. **Full** keeps every animation even when the device asks for less. The page gets `data-motion="reduce"` or `data-motion="full"` before first paint, and apps read the same attribute. The Animation speed slider under **Settings → System → Debugging** is a diagnostic, and has no effect while Motion reduces.

### Move to top level
Cutting a *child thread* loose from its *parent thread*, so the parent stops waiting and the child becomes top-level. The thread menu item, the `threads` tool's `detach_child` action and `lucidos threads detach` all do it, recorded as `ChildThreadDetached` on the former parent. Nothing stops: the child finishes its turn and keeps its work.

The parent gets no further result and cannot follow up. The moved child keeps holding the parent's child slot while it is a *live child*, so a move frees nothing until the child finishes. An agent can move only its own direct children; the user can move any nested thread. It cannot be undone. Not the same as a *detached* event wait, a subscription holding no turn.
See also: *child thread*, *parent thread*.

### Ready signal
An app's call to `lucidos.ui.ready()`, saying its first content is drawn. The host covers an opening app and shows a progress bar until it can reveal it. An app whose *app manifest* declares `"reveal": "on-ready"` is revealed by its ready signal, with a 15 s fuse. Any other app is revealed on page load, and the call does nothing. See `system-knowhow/js-sdk.md` § Showing the app once its content is ready.

### Read marker
The "Sent" or "Read" label on a message sent to a thread. "Sent" means the engine handed it to the agent. "Read" means the agent took it in. A message stuck on "Sent" never reached the agent.

On a *coding-agent thread* the engine records each read as `CodingAgentInputRead` (ADR 0268). While the agent is busy, an unread message waits at the bottom as "Queued". Once read it moves into place, with what the agent does next below it.

On a *Lucidos Agent* thread a message is read once the agent starts its turn, or takes it into the running one. What a caller says on a call carries no marker.

Until read, a queued message offers **Edit** and a bin, on a Lucidos Agent or Claude Code thread. The bin takes it back. Edit takes it back into the compose box, text and images, so resending puts it at the back of the queue. Once read, both refuse and say so. Codex cannot take a message back, so its queued messages offer neither.

### Reasoning effort
How hard a *model* is told to think before it answers, chosen beside the model in the picker and in **Settings → Models**. Six levels: Off, Low, Med, High, X-High, Max.

Which levels a model offers depends on its *provider* as much as the model. Gemini stops at High, since every level above sends an identical request. OpenAI before GPT-5.6 stops at X-High. A non-OpenAI OpenAI-compatible server stops at High: X-High is OpenAI's own word, and a third party answers 400.

A level the model lacks snaps to the nearest one it offers, ties going up. So switching model never quietly spends less thought than you asked for.

Chat remembers a level per model, set in **Settings → Models** (the `chat_reasoning_efforts` *preference*). A model you never set one for runs at its *default effort*, and the picker's **Default** row returns a model to it. Switching a thread to another model never carries the old model's level along.

### Default effort
The *reasoning effort* a model runs at when nothing stored names one: the level its *provider* documents as the default, for example Medium for Opus 5.5 and High for Opus 5. Each row in the *model registry* carries it, and `manage_models` `list` shows it. A model whose provider documents none runs at "Provider default": Lucidos sends no level, and the provider picks. A background task keeps its own level on its recommended models, and runs any other model at that model's default effort.

### Response style
How an answer comes back, chosen in **Settings → Models → Response style**. Two independent parts. The **style** is the shape: how much comes back and what for, from outcome-only to explaining the why. The *technical literacy* is how technical the words are.

The style applies to chat and every *trigger* from the next message, and never to a *coding-agent thread*. **Standard**, the default, adds nothing. Whatever the style, Lucidos keeps every warning, every caveat that changes the answer, and every step the user must take. That rule sits outside the editable text and cannot be written away.

### Theme
A named set of design-token values that retunes how Lucidos looks: colours, the header bar, focus, radii and shadows. It can suggest a UI font and a code font from the *font catalog*; the user's own font pick wins. A theme styles one *theme mode* or both, picked by the device-scoped `theme` *preference* (**Settings → Appearance → Theme**). It holds only catalog tokens and *theme parts*, never CSS: it can recolour a control but never move or hide one. The *style overrides* still win over it, and it never repaints a *protected surface* past its clamped palette.

Built-in themes ship with the engine. A workspace theme is a file at `data/themes/<id>.json`, which a *plugin* can ship in its `themes/` folder. The engine validates it and derives what its three seed colours (background, text, accent) imply. Every surface, app frames included, paints the result. See `system-knowhow/themes.md`.

### Theme effects
Whether a *theme*'s part shadows, filters and scanlines show on one device: **Settings → Appearance → Theme → Effects**, the device-scoped `theme-effects` *preference*. **Reduce** drops every part `text-shadow`, `box-shadow` and `filter`, and the screen's scanlines. It keeps part colours, letter-spacing, the caret shape and borders.

**System**, the default, drops them when the device asks for more contrast or less transparency. **Full** always shows them. The page gets `data-theme-effects` before first paint, and apps read it too; battery state plays no part. See `system-knowhow/themes.md` § Theme parts.

### Theme family
The group a *theme* belongs to in the picker: `blue`, `violet`, `warm` or `neutral`. The picker labels `blue` "Cool". A theme names it in its own file, so similar themes sit together. A theme with no family shows last, under "Other". See `system-knowhow/themes.md`.

### Theme part
A named region of the UI that a *theme* may style with capped paint-only properties. Shell parts include chat text, the actor icons, header titles, the composer, step cards, floating surfaces and the screen. Three more paint inside app frames.

A theme names the part and the property, such as `"parts": {"chat-text": {"text-shadow": "0 0 0.3em var(--accent)"}}`, and never writes a selector: Lucidos owns them all. Each property has a grammar and caps, so no part can cover, hide or fake anything. Parts may target both modes or one, and none reaches a *protected surface*. `GET /api/v1/themes/parts` lists every part, and each part property compiles to a *part token*. See `system-knowhow/themes.md` § Theme parts.

### Theme token catalog
Every token a *theme* can tune, with its group, a description, its default in each *theme mode*, and the seeds it derives from. Served at `GET /api/v1/themes/tokens`, so a theme-building app can list every tunable element. A test pins it to the stylesheet, so no declared token is missing from it. See `system-knowhow/themes.md` § The theme token catalog.

### Theme mode
Light, dark, or system (follow the OS): the `theme-mode` *preference*, per device. The shell and every app frame paint the resolved value as `data-theme-mode` on `<html>`. Distinct from a *theme*, which picks the colours a mode paints.

### Technical literacy
How technical the user is, and so how technical the words are in every answer. Three levels, shown on every card as **Keep it plain** (non-technical), **Technical** and **I write software** (developer). It is the *response style*'s second part. The first chat asks for it during first-run setup. It can also be set in the *setup interview*, by telling the agent, or in Settings (row **How technical**). It reaches chat and *triggers* from the next message, and a *coding-agent thread* or a voice call from its next start.

Unset adds nothing. Lucidos stores only a level the user stated, never one guessed from their writing. The level changes the words, never the substance or the amount: warnings and steps stay.

At **Keep it plain**, the agent never asks a question the user cannot answer, such as what to do with a git branch. It decides, says what happened in their terms, and does the work itself rather than suggesting a *coding-agent thread*.

### Style library
The *response styles* a workspace can pick between. Lucidos ships four:

- **Standard**, the off switch, which cannot be edited or deleted;
- **Concise**, the answer first with no preamble;
- **Minimal**, the outcome and not the process;
- **Learning**, which explains the why as it goes.

The last three can be edited. An edited one keeps its shipped description, shows as edited in Settings, and offers *Reset* to restore the shipped wording. The user can add any number of their own, each a name plus an instruction in their own words. One style is selected at a time.

### Preferred provider
The *provider* a *model* was last picked on, remembered per model on its row. So Grok stays on xAI and Opus on Anthropic, neither overwriting the other.

The picker asks for a provider last, after the model and its *reasoning effort*, and only when two of the model's providers are set up. With one set up it never asks, but still names the provider. A pick sticks to the thread or draft it was made in, so a later pick elsewhere never moves a running conversation. A *trigger* can pin its own. Settings → Models → Routes shows and edits which providers serve each model, in the order tried.

A model with no stored choice runs on its first backend with credentials. If the stored choice is later removed or switched off, the turn is refused rather than quietly moved. The picker badges it "not set up", so one click fixes it.

### Provider
The backend that serves a *model*. Each *model registry* entry names its provider, whose credentials are configured once under Settings → Models → Providers.

**Vertex AI** takes no stored credential: it resolves project and token from gcloud. **Anthropic** is direct via `api.anthropic.com`: a Claude subscription OAuth token or an API key. The `ANTHROPIC_API_KEY` launch env var is a fallback below the stored credential. **OpenAI** is direct via `api.openai.com`: an API key, with the `OPENAI_API_KEY` launch env var as a fallback. Below that, a key is auto-detected from the Codex CLI's `${CODEX_HOME:-~/.codex}/auth.json` `apikey` login, as Vertex reads the gcloud ADC file.

**OpenRouter** is `openrouter.ai/api/v1`: a Bearer API key, with `LUCIDOS_OPENROUTER_API_KEY` as fallback, serving e.g. GLM 5.2. **xAI** is direct at `api.x.ai/v1`: a Bearer API key, with `LUCIDOS_XAI_API_KEY` as fallback, serving e.g. Grok 4.6. **Local** is any OpenAI-compatible server (Ollama / LM Studio / vLLM / llama.cpp) at a configurable base URL. The default is Ollama's `http://localhost:11434/v1`, and the API key is optional.

**OpenCode Free** is `opencode.ai/zen/v1`, the one provider with no credential. The relay serves free models anonymously, so no key and no account exist. It is off by default and switched on with a toggle. Requests leave the machine to a third party, and several free models may train on them, so the toggle says so.

OpenAI, OpenRouter, xAI, OpenCode Free and Local all speak the OpenAI Chat Completions wire format but are distinct backends.

A model naming several providers stays ONE picker entry, served by whichever has credentials. The first-party Claude models list Vertex and Anthropic, since their ids match on both. A backend that spells a model differently says so per provider: Grok is `grok-4.6` on xAI and `x-ai/grok-4.6` on OpenRouter.

### System One model
A model that answers typed questions with a probability for every answer, rather than writing text. It cannot hold a conversation, so it never appears in the chat model picker. Four are offered: TypeSafe's **Jev**, Cloudflare's **Clef** and **Clef-flash**, and a **custom** endpoint, such as a model hosted on the user's own machine. Each is set up on its own row in Settings → Models → Providers.

A System One model is picked in the model control of a decision Lucidos makes: the command guard's Judge model, and Query classification. Picking one sends what that decision judges to its vendor. By default every such decision runs on a chat model, and storing a key changes nothing on its own. If a System One call fails, the chat model answers instead.

### Builtin provider proxy
A *provider*'s API exposed through the engine's proxy **without** re-entering the credential in `data/config/apis.json`. Apps reach it with `lucidos.proxy(<name>).fetch(path, init)`, scripts with `lucidos proxy <name>`, and the Lucidos Agent with `proxy_request`. The builtin names are `anthropic`, `local`, `openai`, `openrouter`, `typesafe`, `vertex` and `xai`. `opencode-free` has none (ADR 0104).

When `<name>` matches one and no `apis.json` entry exists, the engine forwards to that provider's API root (every default root includes `/v1`). It injects the credential from Settings → Models → Providers server-side, so the secret never reaches the caller. A same-named `apis.json` entry is consulted first and overrides the builtin. The Lucidos Agent's context lists every builtin and whether it is configured. `request_credential` refuses a key a configured builtin already injects (ADR 0350).

`vertex` takes the publisher/model suffix only: the engine owns the `…/projects/<project>/locations/<region>` prefix and mints the access token. So the app never needs the project id or a token. See `system-knowhow/js-sdk.md` § `lucidos.proxy`.

### Rejected proxy entry
An entry in `data/config/apis.json` the engine will not serve, because it does not parse or uses a retired shape. Only that entry is rejected: every other entry works, and the workspace still starts (ADR 0135). A notification names the entry and the reason at boot, and a call to that proxy answers `502`. Fix it by editing the entry and restarting the workspace.
See also: *derived proxy entry*, *builtin provider proxy*, `system-knowhow/building-an-auth-handshake.md`.

### OAuth provider registry
The list of OAuth providers Lucidos knows the endpoints for, stored as
`system-knowhow/oauth-providers.json`. Each row carries a provider's
authorization, token and userinfo URLs, its userinfo method, its authorization
parameters, its base URL, and where to register an app with it: the console link,
which client type to pick, which permissions to enable.

With it, **Settings → Accounts** offers a quick button per provider and
prefills a whole app registration. So an OAuth Client *credential* needs only a
Client ID. Saving copies the endpoints into the credential, so it fully describes
its own flow. A registry that later moves an endpoint cannot silently break a
working one; the *Lucidos Agent* repairs a stale credential on request. A
provider absent from the registry still connects: the form asks for its
endpoints, or which known provider a *derived provider* name runs on.

`system-knowhow/oauth-providers.md` is the prose beside it (redirect URI forms,
confidential versus public clients, scope notes) and does not restate the rows.
Adding a provider is a JSON edit, never an engine change.
See also: *connected account*, *credential*, *derived provider*,
*OAuth client type*.

### OAuth redirect URI
The loopback URL the provider sends the user back to after they authorize. Lucidos must repeat it byte-for-byte when it redeems the authorization code. A temporary listener on a fixed port binds **both** loopback families, so three host forms work: `http://127.0.0.1:14981/oauth/callback` (the default), `http://localhost:14981/oauth/callback`, and `http://[::1]:14981/oauth/callback`. The engine owns port and path. Only the host form is configurable, via the optional `redirect_uri` key on the *credential*. Providers disagree: Spotify rejects the name form, and Microsoft's Entra portal rejects the IP form under its Web platform.

The user must register the resolved URI with the provider exactly. Which form a provider wants lives in `system-knowhow/oauth-providers.md`, never in engine code.

### OAuth client type
Whether Lucidos authenticates the token exchange as a **confidential client** (sends the `client_secret`) or a **public client** (no secret, PKCE instead, per RFC 8252). It follows only from whether the *credential* carries a `client_secret`, so the engine never needs to know the provider. It must match the app's registration: a web/confidential one rejects a secret-less redemption, and a desktop/native/public one rejects a secret. Lucidos runs on the user's own machine, so public fits best wherever offered. Leaving Client Secret blank in the credential modal selects it.

### OS surface
The notification surface outside the Lucidos UI: an OS-level banner. Two transports, chosen by client:
- **Web push** (browser / PWA): delivered by the device's push service (APNs on iOS, FCM on Chrome/Edge, Mozilla autopush on Firefox) and drawn by the service worker. The browser requires each push to show a visible `showNotification()` (`userVisibleOnly: true`); silent pushes are penalised and can revoke the subscription.
- **Native desktop** (Tauri app): a macOS notification driven by the *NativePushRequested* SSE. The app's `show_native_notification` command shows and tap-routes it via Apple's `UserNotifications` framework (`UNUserNotificationCenter`). WKWebView can't subscribe to Web Push, hence the SSE route. Requires a packaged `.app` build (inert in `tauri dev`).

Both ride the engine's single push-allowed decision (see PresenceCheck protocol). So a notification reaches a device through exactly one transport and never collides with the *in-app surface* toast.
See also: `system-knowhow/notifications.md` §§1, 3, 4.

### Orchestrator
The role a *parent thread* plays while it runs several *child threads* at once: it scopes their work, rules when they disagree, and is the only thread that can direct them. Not a separate kind of thread or relationship, just a name for what the parent does. Children may observe each other freely (events, artifacts, transcripts) and may never direct each other. So the immediate parent settles a disagreement from the shared event record; children never negotiate.

The rule recurses: a child that spawns children is their orchestrator. Beyond that, the engine models nothing; the rest is the orchestrator's judgement. Reasoning: `docs/adr/0083-sibling-threads-observe-never-direct.md`.
See also: `system-knowhow/orchestrating-sub-threads.md`.

### Device
One browser storage container that has met Lucidos. Listed in **Settings → Devices**, one row each, however it arrived. A device is what a push goes to, what a device-scoped *preference* applies to, and who an actor chip credits.

A device has one name everywhere: the name typed on its row, else its *pairing label*, else its browser and machine, as in "Chrome on Mac (109371a3)". Failing all three, it is `device-` and the start of its id. See `system-knowhow/remote-access.md` § What a device is called.

**Per browser means per browser, and on iOS that includes the home-screen app.** iOS gives it its own storage container, so it is a separate device from Safari on the same phone.

One row carries two facts with different reach. Its *pairing* (may it reach the machine at all) is machine-wide: **Revoke** cuts it off every *workspace* at once. Everything else is per workspace: where push goes, which preferences apply, the typed name. **Remove** forgets only that, here, and leaves the device paired.

Either half can be missing, and neither is an error. A device paired from another workspace holds nothing here yet. Its row says **Not set up in this workspace**, with push off and disabled until it opens this one. The row never claims the device was never here, since a missing half does not prove that. A browser reaching an engine directly never went through the gateway, so it has no pairing to revoke.

**One device is one row.** The id comes from the *workspace gateway* when there is one, and both halves key on it. A browser reaching an engine port directly mints and keeps its own, which is safe: no pairing list exists there to disagree.
See also: *paired device*, *pairing code*, *preference*, *active device*, *one-off device*.

### One-off device
A *device* used on one day and never again. Lucidos removes it once a week old, unless someone named it, paired it, or turned push on for it. Most come from automated browser runs, since a fresh browser profile mints a fresh device id. One that does come back registers again under the same id, without its old device-scoped *preferences* and pinned apps.
See also: *device*, `system-knowhow/remote-access.md` § The list of devices lives in Settings → Devices.

### Pairing label
The name a device got when it paired, such as "Safari on iPhone". The pairing screen suggests it from the browser, and the person at the device may type their own. It is fixed until you revoke the device and pair again. The *workspace gateway* passes it with each request, so every *workspace* knows it. A typed **Devices** row name wins over it (see *device*).
See also: *device*, *paired device*, `system-knowhow/remote-access.md` § What a device is called.

### Paired device
A *device* you have allowed to reach Lucidos over the network. The *workspace gateway* answers no unpaired network caller, so joining the tailnet is not enough. Pairing is per device and per browser, and survives restarts until revoked. A paired device reaches every *workspace* that gateway serves. Workspaces are no boundary against each other, since a *coding agent* in one can already read another through the shell.

**Pairing is per gateway, which shows only when a machine runs two** (see *cross-gateway link*). Each keeps its own device list, *pairing code* and cookie. So one refuses the other's code, **Devices** lists the gateway serving that page, and revoking there revokes there (ADR 0132). One browser can hold a pairing to both at once.

On iOS the home-screen app is its own *device*, so it pairs separately from Safari. That is why the pairing screen shows a phone browser the install steps first, and why a phone can appear twice in **Devices**.

**A pairing ends when you revoke it, and at no other time.** Lucidos runs no idle or absolute timeout, so a device unopened for a year still works. A stolen credential in use never goes stale, so an expiry would cut off only the devices you forgot. Revoking answers the one you know you lost.

The browser cookie carries a window, refreshed each day the device is seen, but the *workspace gateway* never reads it. So **Devices** shows when each device was last seen, to the day. That is what separates a phone in daily use from a laptop you sold, so read it before revoking. A device paired before Lucidos recorded this shows no last-seen until its next request.
See also: *device*, *pairing code*, `system-knowhow/remote-access.md` § Devices pair before Lucidos answers them.

### Pairing code
The one-time code that makes a device a *paired device*. It works once and expires in five minutes. Two places produce one to pass on: **Settings → Access → Add a device**, and `lucidos pair` on the machine Lucidos runs on. The desktop app mints a third silently for itself: it reads the local file that proves it is on the machine, and pairs its own window on launch.

Both also draw it as a **QR**: the same code wrapped in a reachable address, never a second credential. Typing the digits does the same, so a failed scan is never a dead end.

A desktop browser that scans it opens the pairing screen and spends the code at once. A phone browser gets the install steps instead, since the installed app is a different device from the tab. The code rides into that install, so the app pairs itself on first open. Install promptly: the code still lasts five minutes. An app already installed, or whose code ran out, takes one by **Paste** or **Scan QR** on its own pairing screen.

Only a process on that machine can mint one, which stops a stranger pairing in. An already-paired device can mint one too, so you can add a tablet from the sofa. A browser needs a code even on the same machine: proving you are local means reading a file, and a browser cannot. The desktop window is a browser by that test, so its Rust side mints for it.
See also: *paired device*, `system-knowhow/lucidos-cli.md` § `lucidos pair`, `system-knowhow/remote-access.md` § Settings → Access.

### Parent thread
The direct ancestor of a *child thread*, resolved via the child's `parent_thread_id` column. A thread has at most one parent; a parent can have many children. Each child reports its outcome upward when it terminates, and the parent can send a *child follow-up* down to any child it spawned. A child paused by a user Stop sends a note instead; see *stopped child*.

### Part token
The custom property one *theme part* property compiles to, named `--part-<part>-<property>`, such as `--part-chat-text-text-shadow`. The engine emits it in the *theme*'s resolved map in canonical form, never the author's string. A shipped Lucidos stylesheet reads it under the part's selector, falling back to the element's unthemed paint. An app that loads the SDK stylesheet may read one by name. `--part-` is reserved: a theme's token maps cannot set one, and one in the *style overrides* must pass the same grammar.

### Paused (thread status)
A *thread* whose turn the user's own *Switch to new version* interrupted, and which the engine has promised to resume. Its indicator is the one that is not a dot: the standard pause glyph, in a neutral tone, never the red *failed* dot. The row's Info card says "Paused", and the transcript labels the turn "Paused by restart". The engine resumes the turn itself, usually within seconds, so no **Continue** button appears and nothing is asked of you. A paused thread does not count toward *attention*.

Every OTHER interruption is *failed*, with the red dot, an *attention* count, and Continue. That covers a crash, an engine shutdown the user did not ask for, and a switch whose resume the next boot cannot deliver. In that last case the boot replaces the pause with the error. So the pause glyph is a promise, never shown for a turn nothing is coming back for.

Paused is a *verdict* about the interrupted turn, not a resting state, so events that merely close the turn cannot walk it back to idle. A follow-up message clears it like any new work. Distinct from `waiting` (a *change* is in review) and `waiting_for_user_answer` (the agent is parked on a question).

**Not a stopped child.** A *child thread* paused by a user Stop waits on the user, not the engine. That makes it a *stopped child*, and it counts toward attention.

**A pending change does not change the verdict.** A thread interrupted mid-turn reads Paused (or *failed*) whether or not it carries a *change*. Nothing writes `waiting` for that now: the thread's change state carries the change, not its status. The change stays in the thread and the review list.

### Pinned thread
A *thread* the user pinned to keep at hand, in the Pinned section of the *thread drawer*. A pinned thread is never archived: pinning an archived thread brings it back, and archiving a pinned thread unpins it. So a settled pinned thread offers Archive beside Unpin. Only the user's own Archive unpins. Automatic archiving (a finished, unwatched trigger run) and an agent's archive leave a pinned thread alone.

Internally the pin is still called "saved": the `saved` section key, `is_saved`, and the `ThreadSaved` / `ThreadUnsaved` events.
See also: *Current section*, *thread drawer*.

<!--gloss-plugin-start-->
### Plugin
A bundle of installable workspace content shipped as one unit. Contains any of `apps/`, `knowhow/`, `triggers/`, `scripts/`, `auth-modules/`, `themes/`, `fonts/`, mirroring the top-level `data/` directories. Defined by a *plugin manifest* at the root. Install merges the contents into the workspace's `data/`. Use a plugin when the pieces only make sense together (an app, its knowhow and its trigger); ship single files on their own.
See also: `system-knowhow/plugins.md`.
<!--gloss-plugin-end-->

### Plugin category
A topical tag on a *plugin* (e.g. `finance`, `health`, `developer-tools`) for browsing the *Plugins panel*'s catalog, as a per-category filter and chips on each card. A **controlled vocabulary**: the author tags it in the *plugin manifest* (`categories = [...]`) from a fixed set. A value outside the set is dropped and flagged in the catalog scan's `errors`, never blocking install. Distinct from a plugin's *content* kinds (`apps`/`knowhow`/`triggers`/`scripts`/`auth-modules`), which the engine derives from the files. Allowed set and rationale: `system-knowhow/plugins.md`.

### Plugin manifest
The `manifest.toml` file at the root of a *plugin*. Declares `id`, `version`, `name`, `description`, optional topical `categories` (see *plugin category*), an optional *engine requirement*, and optional install-time `setup` steps. Schema in `system-knowhow/plugins.md`.

### Engine requirement
The Lucidos releases a *plugin* works on, declared in its *plugin manifest* as a semver requirement: `engine = ">=0.46.1"`. Set it to the first release with every platform feature the plugin uses. Omit it and any release will do, but Lucidos shows the gap: "No version requirement" on the *Plugins panel* row, and a note in the confirmation panel.

The engine enforces it at install and update only, never on an installed plugin. A release that misses it is refused before anything is written: "Theme Studio 0.1.0 needs Lucidos 0.46.1 or later". A malformed value is refused too. The *Plugins panel* still lists such a plugin, with Install or Update disabled and the reason beside it. The Apps panel offers no Update, and no update notification is sent. See `system-knowhow/plugins.md` § "The `engine` requirement".

### Plugin local patch
The difference between a *plugin*'s content on disk and what it shipped at your installed version. The **Modified** badge counts it, as a diff rather than a yes or no. An update three-way merges it into the new version, so an edit you rely on survives. Where a merge is impossible, your version lands under `data/artifacts/plugin-local-changes/` with a `.patch` beside it. **Derived, not stored**: the engine diffs the working tree against the install commit, which records what the plugin shipped. The *Plugins panel* can offer the patch to the plugin's author in a thread; see `system-knowhow/plugins.md` § "Local modifications".

### Plugin modified state
Whether a *plugin*'s shipped content was edited locally since install: the user, the *Lucidos Agent* or a *coding-agent thread* changed an app, knowhow, script or trigger it owns. Shown as a **Modified** badge on its *Plugins panel* row, whose tooltip lists the changed paths. Updating merges those edits (see *plugin local patch*), and the install panel states the outcome per file. **Derived, not stored**: the engine diffs current `data/` against the install commit, so it self-heals on a revert and stays true after a merge. A file added in a plugin's app directory counts, build output aside; a new file in a shared root belongs to no plugin.

### Plugins panel
The top-level panel for discovering, installing and managing *plugins*, including the *apps* they ship. One list with an **Installed only** checkbox, **checked by default**. Checked, it shows every plugin on disk, app-bearing or not. Unchecked, it widens to the whole catalog of registered *marketplaces*, installed and available. A live search and a per-category filter compose with it.

Each plugin is a card whose primary button runs **Install** (or **Update**) → **Setup** → **Open**, plus **Uninstall** once on disk. "Setup" shows while the plugin's *setup thread* runs; "Open" launches its app once setup is done or if there was none. Installs and uninstalls go through the standard plugin confirmation panels. The catalog re-scans whenever shown (the panel opens or **Installed only** is unchecked). The engine never silently updates a marketplace plugin: it notifies, and the user updates from the card or the app's **Update** button (see *Marketplace*).

With no marketplaces registered, the catalog and Settings → Marketplaces empty states offer **Add the official Lucidos marketplace** (registering `github.com/lucidos-dev/plugins`). Marketplaces are added and removed under Settings → Marketplaces.

Distinct from the *Apps* panel: a plugin's app appears in Apps (open it) AND its plugin appears here (manage, update, uninstall). Plugins that ship no app (knowhow-, trigger-, script- or auth-module-only) live here too, with links to each shipped file. See ADR 0019.

<!--gloss-proxy-start-->
### Proxy
The engine's route to an outside service. You connect the service once, as an entry in `data/config/apis.json`: an API key, an OAuth sign-in, or a custom handshake or signer. The *Lucidos Agent* calls it with `proxy_request`, an *app* with `lucidos.proxy(<name>)`, and a *script* with `lucidos proxy <name>`. A *trigger* reaches it through its thread or its script. The engine adds the *credential* to each request server-side, so an app never sees the key.

A *builtin provider proxy* reaches a model provider with no `apis.json` entry.
See also: `system-knowhow/building-an-auth-handshake.md`, `system-knowhow/js-sdk.md` § `lucidos.proxy`.
<!--gloss-proxy-end-->

### Preference
A single key→value user setting in the `preferences` table: theme, language, timezone, push notifications, the welcome message, chat model, UI scale, font, and so on. Most of what the user calls **Settings**; the rest (*models*, *credentials*, MCP servers, *repositories*) have their own stores. A preference is **global** (workspace-wide) or **device-scoped** (a per-device override that wins on the device that set it). The *Lucidos Agent* reads and writes the agent-settable ones with `get_preferences` / `set_preference`; the human uses Settings. A write emits the persisted `PreferencesChanged` event (or `LanguageSet` / `TimezoneSet` for locale), which open pages live-apply.

Distinct from *config* (`data/config/` files like `apis.json`) and from a *credential*, a secret never stored as a preference. See `system-knowhow/preferences.md`.

### Pseudo-fullscreen
The *app UI* filling the whole viewport as a CSS overlay, where the native Fullscreen API is unavailable or refused. That is the iOS path, so it is usual on a phone. Both modes start from the same content-header control and look the same to the user.

A natively fullscreen element is painted alone, so the browser supplies Escape and draws nothing else. A pseudo-fullscreen panel sits in the normal layer, so the host draws its own chrome over the app iframe. That is an exit button top-right, plus a transparent guard strip down each screen edge on mobile. Those regions belong to the host, and an app must keep them clear (`system-knowhow/building-an-app.md` § Responsive by default). Source: `.app-ui-fullscreen` in `crates/lucidos-app/src/styles/panels/previews.css`, gated by `isPseudo` in `components/apps/AppUiInline.tsx`.

### Pull to refresh
Dragging down past the top of a *content pane* view on a touch screen to re-read what it shows. A scroll up that keeps going past the top counts too. An arrow drops with the finger, blending into the accent colour, fully there once letting go would refresh. It then stops dropping but keeps turning while the finger pulls. A refresh icon spins beside the header's menu button until the new data lands, then turns into a check. On desktop it is the header's Refresh button.

Each view decides what it re-reads. Disk Usage re-measures its worktrees, the Plugins panel rechecks every marketplace, and an *app* reloads as its header Refresh does. Live views, such as the thread list, have nothing to pull. Inside an app the SDK sees the pull, and an app can opt out (`system-knowhow/js-sdk.md` § Pull to refresh).

### Pane swipe
On a phone, a sideways drag that moves between the three panes: the *thread drawer*, the *thread pane* and the *content pane*. A short fast flick or a drag past a third of the screen moves one pane; anything less springs back. It works anywhere, over an open *app* or an HTML preview too, except while an app is fullscreen. Over a PDF or a web page preview it starts only at the screen edges. Inside an app or an HTML preview, a carousel or a slider keeps its own sideways drag (`system-knowhow/js-sdk.md` § Pane swipe).

### PresenceCheck
The transient SSE event the *Lucidos Engine* broadcasts on every `NotificationCreated` to ask each connected page for its live presence. A **pure pong trigger**. It carries `notification_id`, `event_id` (so the pong can report `event_in_viewport`), `sent_at_ms`, and a `deadline_ms`. The deadline comes from `scheduler::push::DEADLINE_MS`, currently 2 s. That covers an iOS PWA's first packet after Tailscale wakes from idle, which can take 1100–1800 ms. It carries NO toast content; *NotificationToastRequested* drives the toast, so it cannot race the push decision.

Each page answers with a *PresencePong*. The engine collects pongs up to the deadline and decides whether to send an *OS surface* push. The check is skipped only when nobody is reachable. That means no open SSE connection AND no device pinged visible within `PRESENCE_STALE_AFTER` (120 s, `core::device_presence`). The live SSE-connection count is the primary gate (`engine.sse_connections`); heartbeat candidates are secondary (`expected_pong_count` in `scheduler::push`).

The SSE count makes this robust. iOS suspends the 30 s heartbeat while a PWA is foregrounded, so the heartbeat row goes stale on a connected page. Gating on the open connection still lets that page suppress the push. See `system-knowhow/notifications.md` §3.

### PresencePong
The page's response to a *PresenceCheck*, POSTed to `/api/v1/presence-pong` with `notification_id`, `device_id`, `is_active`, `focused_thread_id`, `event_in_viewport`. An OS push goes out iff NO pong reports `is_active`; multi-tab pongs are ORed per device. Late pongs (after the deadline) ack 200 and are dropped, since the race is normal. One pong is owed per open SSE connection, so documents sharing one through the *shared SSE holder* are ORed into a single POST. See `system-knowhow/notifications.md` §3.

### Shared SSE holder
The one `SharedWorker` per *workspace* per browser profile that owns that workspace's `GET /api/v1/events` connection. It relays every frame to its attached documents: the Lucidos shell, each app iframe, and each app in its own tab. So open apps no longer multiply connections. Its script URL (`/<slug>/api/v1/sse-worker.js`) carries the workspace prefix, so no document can receive another workspace's frames. It ORs its documents' *PresencePong* answers into the one pong its connection owes. A browser without `SharedWorker` (Chromium on Android, Android WebView) falls back to one private `EventSource` per document.

### NotificationToastRequested
The transient SSE event the *Lucidos Engine* emits to drive the *in-app surface* toast. The `NotificationCreated` fan-out emits it **only on the push-suppressed branch**, when the *PresenceCheck* pongs show an *active device*. It carries the toast content (`title`, `body`, `thread_id`, `event_id`, `app_id`, `tap`, `sent_at_ms`), so the page needs no re-fetch. Active pages show the toast, or auto-read when looking at the *source event*; hidden pages ignore it. It and the OS push hang off opposite branches of one decision, so a device never gets both for one notification. See `system-knowhow/notifications.md` §4.

### NativePushRequested
The transient SSE event the *Lucidos Engine* emits to drive the native-desktop *OS surface*. The complement of *NotificationToastRequested*: emitted **only on the push-ALLOWED branch** (no *active device*), with the same payload. A connected Tauri desktop app shows a macOS notification from it via its `show_native_notification` command. That uses Apple's `UserNotifications` framework (`UNUserNotificationCenter`), whose delegate captures the click to route the tap. WKWebView can't receive Web Push, so this is the desktop's web push, over the open SSE stream.

Needs a packaged `.app` build (inert in `tauri dev`). Browser / PWA pages ignore it (the handler gates on Tauri) and get the real web push on the same branch. See `system-knowhow/notifications.md` §§1, 4.

### Scratch
Ephemeral working files under `.lucidos/tmp/`, at the *workspace* root and so **outside** `data/`: gitignored, not indexed, not *artifact*s, safe to delete any time. `http_request(temp_path)` saves raw responses there, `git_clone` puts inspect-only checkouts there, and plugin archives are staged there during install. The file tools **read** it (`read_file`, and `copy_file` as a source to promote a file into `artifacts/imported/<name>/`).

They never **write** it, since they git-commit everything they write. So `write_file` / `edit_file` / `delete_file` refuse a scratch path and point at `run_python`, whose cwd is the workspace root. Only `.lucidos/tmp/` is addressable; the rest of `.lucidos/` (coding-agent *worktree*s, `exhaust/`, `engine.pid`) is refused both ways.
See also: `system-knowhow/best-practices.md` rules 8 and 10; ADR 0051.

### Script
Code (Python, shell, JS) invoked by an *intent* or *knowhow*. Lives with its primary consumer when scoped (`data/apps/<id>/scripts/`, `data/triggers/<slug>/scripts/`, `data/knowhow/<domain>/scripts/`), or at top level (`data/scripts/`) when shared.

### Setup interview
A guided interview the *Lucidos Agent* runs to work out what this person should use Lucidos for. It ends with *app*s, *trigger*s and *knowhow* actually built in their *workspace* in that session.

Its first card asks how technical the person is, unless first-run setup already did, and stores the answer as their *technical literacy*. Every later card and thread uses that level. It is not work-only. The next card asks which parts of life to cover (work, home and personal admin, health and training, learning and side projects). It takes several answers, so a kit can center on training or a household as readily as a job.

Three entry points start it:

- the "Help me get the most out of Lucidos" button on the first-run welcome;
- the "Setup guide" row in the *Lucidos menu*, which the Lucidos mark opens on every viewport;
- the help button beside New thread on the desktop header.

**It runs in the *home thread*,** wherever it starts, so it creates no thread of its own (ADR 0411).

The menu row says "guide", since "interview" reads as an interrogation to a newcomer. The two later entry points confirm first, since they send. Mobile has no header *button*: its header has no slot to spare for a rare action. There the menu row is the durable route, beside the welcome and simply asking. Every entry point sends the same ordinary message, so typing always reaches it.

`system-knowhow/setup-interview` drives it. That file owns the areas to ask about, the question ladder, which cards take several answers, and the answers-to-kit mapping. It also owns the confirm-before-building rule and what gets persisted. The record lives at `artifacts/setup-interview.md`, appended per run and never overwritten, plus a `SetupInterviewCompleted` *domain event*. Only facts the user stated reach memory or `user_profile.md`; the agent's conclusions stay in the artifact.

It is a sibling to the two other workspace-wide recipes:

- *workspace audit* asks whether the workspace matches current conventions;
- *workspace learning* asks whether the conventions match this user;
- the setup interview asks whether the workspace matches the person.

It is the only one of the three that needs the user present, since it asks and then builds rather than sweeping and proposing. Distinct too from a *setup thread*, which finishes installing one *plugin*.

### Setup thread
A *Lucidos Agent* *thread* the engine spawns when a *plugin* with a `setup` field is installed, from the *Plugins panel* or the `install_plugin` tool. On an **update** it spawns only when the new `setup` differs from the installed one. The user lands straight in it on install. Only a user-confirmed install spawns one; the background marketplace update check only notifies.

Its first message names the occasion. A first install seeds `Set up the newly installed <name> plugin.` An update seeds `Set up <name> again: its setup instructions changed since version <prior>.` under the title `Update <name> setup`, so the thread list never shows one name twice. A legacy record naming no version drops the `since version` clause, and says `this update changed its setup instructions` instead.

The engine wrote that line, so it is attributed to the *Lucidos Engine* with the reason `plugin_setup`, never to the user. It records the plugin, both versions and the occasion. The message route popover shows "Plugin install" or "Plugin update", with the confirming device as "Confirmed on". See `system-knowhow/thread-events.md` § Engine origins.

The agent loads `system-knowhow/plugin-setup` to plan the steps as a todo list. The author's setup instructions come by reference from the `PluginInstalled` event, not embedded in the thread. The agent walks the user through them, asking for credentials and choices and doing the wiring it can.

On an update it starts from the last run. It diffs the two `setup` texts and reads the previous run's `PluginSetupCompleted` *domain event*, which holds the user's choices and what was skipped. Without one, the earlier setup thread is the fallback. A record is never enough alone: the agent checks each trigger, webhook, credential or config entry is in place, since a record cannot see a deletion.

Its id is in the `PluginInstalled` event, so the card's *Setup→Open* button can reopen it. The card treats setup as done once the thread is no longer `running` or `waiting_for_user_answer`. The same holds once the thread is gone entirely, with no summary row and no live *Thread Queue* entry. So a lost or stale setup thread degrades to *Open*, never a *Setup* button that errors.

### Side-effect grant
The irreversible side-effect categories a *trigger* may perform unattended, set per trigger under "Allowed side-effects". No human is present to ask, so the *command guard* consults the grant instead. An irreversible command (email, a mutating HTTP request, a cloud-CLI change, out-of-workspace destruction, anything else irreversible) runs only if its category is granted. Otherwise it is blocked and the run fails, with a notification naming the missing grant.

Categories: **email**, **external API**, **cloud CLI**, **out-of-workspace destruction**, **other**. The default is empty. Consulted only while the *command guard* is on; chat turns ignore it, since they ask every time. Only the user sets it: the `create_trigger` / `update_trigger` LLM tools cannot, so an agent can't widen its own unattended authority.

The same grant **also governs *coding-agent thread*s a trigger spawns** (Claude Code / Codex), inherited down the spawn tree. The engine resolves their permission cards from the root trigger's grant instead of hanging. Benign in-workspace work and granted categories are auto-allowed; an ungranted category or a catastrophic command is auto-denied. Unlike the chat guard, this denies the single request rather than failing the run, whatever the command-guard toggle. See `coding-agent-events.md` § "Unattended auto-resolution".

The spawn tree means *child* spawns. A *top-thread* is outside its spawning thread's tree, so a coding agent a trigger starts as a top spawn asks a human. A human in the tree also ends the inheritance: once you send a message or answer a question in that thread or any above it, its cards ask you.

Some denials sit outside the grant. A command the *command guard* cannot read is denied whatever the grant holds. Those are shapes whose head is not what runs: a substitution, a code-loading `VAR=` preamble, a path-qualified head, a write outside the workspace. An unrecognised command (`cargo build`) is not one of them, and still runs.

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

### Source event
The specific *event* a notification points to, stored as `notifications.event_id`. The *in-app surface* uses it to tell whether the user is looking at that very thing. If the page is on its thread AND it is in viewport, the notification is auto-marked read with no toast and no badge increment. A notification with `tap = { kind: 'navigate', to: { target: 'thread', id: '...', event_id: '...' } }` also lands on it (scroll + pulse).

Both uses resolve the event the same way, whether it starts a turn or is folded into one as a step. A step is addressed by its own card (a failed response by its failure card), not by the surrounding turn. A source event that never renders is reported, and the transcript stays where it was.
See also: `system-knowhow/notifications.md` §§2, 4.

### Spawning thread
The *thread* that issued the `run_thread` / `run_coding_agent` / `lucidos spawn-thread` call. For `relation: "child"`, it IS the parent. For `relation: "top"`, there is no parent and no callback wiring. Either way the spawn is *attributed*: the spawned thread's first message records its launcher, so its route popover links back here. Attribution is not linkage: a top-thread never reports back, counts as a child, or inherits the spawning thread's permissions.

### Stopped child
A *child thread* whose turn a user Stop ended, not yet continued, archived or discarded. Cancel on its question card counts as a Stop. The child is alive, and one message continues it. Its *parent thread* gets a quiet `ChildThreadStopped` note and no turn, and is still owed a result.

The child's next finished turn sends the usual completion card. Archiving or deleting it, or discarding its change, sends one with status `canceled`. Meanwhile it counts toward *attention*, and a notice at the end of its transcript names the waiting parent. It blocks nothing, so the parent can still be archived.

Distinct from *Paused*, which the engine resumes by itself. See ADR 0252.
See also: *child thread*, *parent thread*.

### Status filter
The `--status` flag on `lucidos threads list` / `count`, and the matching `status` parameter on `GET /api/v1/threads/{list,count}`, the `threads` tool and `lucidos.threads`. It names exactly the *thread* statuses to keep, out of `idle`, `running`, `waiting`, `waiting_for_user_answer`, `paused`, `failed`. These are the values each returned *thread summary* carries in `status`, so a caller filters on what it reads. It is the precise form of the *Active (thread state)* union: `status=running` asks whether the workspace is busy, `status=waiting_for_user_answer` whether anything waits on you, and `active=true` both. Passing `status` with `active` is refused rather than intersected, as is an unrecognized or empty value.
See also: `system-knowhow/lucidos-cli.md` § `lucidos threads list`.

### Superseded question
A *question card* the engine resolved because a follow-up arrived that could not be its answer. Unlike a canceled one, the user did reply, just not to this question, and that reply drives the next turn. The card reads "Replaced by your next message" with its buttons spent; on the wire it is `UserQuestionAnswered { answer: { kind: "Superseded" } }`.

Coding-agent lane only, to break a deadlock only that lane has. The agent is parked inside the call that asked, so only an answer releases it, and the follow-up's prompt event has already killed the card. A chat thread keeps its question live, queuing the follow-up as an injection until the user answers.
See also: `system-knowhow/coding-agent-events.md` § `UserQuestionAsked`.

### Protected surface
A surface where the user grants, denies, answers or confirms: permission and question cards, the credential and email forms, the Apply controls, plugin install panels, and the confirm, prompt and progress dialogs. For text, fills and confirm or deny colours it reads only the `--protected-*` palette. So no *theme*, *theme part* or *style override* can make it unreadable or misleading. The engine derives that palette from the active theme and clamps it. Text and button labels reach WCAG AA, confirm stays green, deny stays red, and a blocking dialog's scrim always dims. See `system-knowhow/themes.md` § Protected surfaces.

### Style override
One entry in the `style_overrides` *preference*: a CSS custom property name and its value, applied on the app's root element. Writing one repaints every connected client live, over the `PreferencesChanged` fan-out, so a design value can be retuned with no rebuild. Device-scoped like other appearance preferences, so tuning on a phone leaves a desktop alone. Values only: it can retune a colour, a size or a duration, never move a control or change behavior.

An override never sets a `--protected-*` token, a `--z-*` stacking token, the UI font tokens, `--user-ui-scale` or the scanlines. It cannot reach inside a *protected surface*. A shadow override stays within 2rem of its box, as a theme's must.

Two ways out if a value makes the UI unusable: **Settings → Appearance → Style overrides → Clear all**, or `?style-reset` on the URL. The URL flag clears them before first paint, so it works when nothing is readable. It also resets the *theme* to the default.

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

A watching thread reads as **Waiting**, like one waiting on its *sub-threads*, even after a reload; the *waiting indicator* says what for. Its *todo list* agrees: unfinished items at park time are marked `waiting`, not `abandoned`.

Two things separate it from a *trigger*. **Where the answer goes:** a trigger runs in its own thread and reaches you as a *notification*, while an event wait resumes the conversation you are reading. **How long it lasts:** a trigger starts a NEW thread every time, indefinitely. An event wait resolves on the first match; the agent re-arms it per event, up to a cap on re-arms without you.

So "tell me **here** when a change is proposed" is an event wait, though it sounds like a standing rule. "Notify me whenever a change is proposed, from now on" is a trigger. Often both are right: watch here now, and add a trigger to keep going after this conversation.

**Neither a message nor Stop ends it.** A message runs an ordinary turn and leaves every subscription as it was, and **Stop** ends only the running turn. A wait that *fires* is used up, and the agent must subscribe again for the next one.

**Four things end one, and each says so:** **Stop waiting** in the *waiting indicator*, archiving the thread, discarding it, and the agent standing it down. Archiving asks first, naming every subscription it would stop, *sub-threads* included. Each leaves a line in the conversation saying what stopped and how, so a watch never ends in silence. None re-opens the thread, so ending the last one on an idle thread settles its *todo list* too.
See also: *event subscription*, *trigger*, *waiting indicator*, `system-knowhow/thread-events.md`.

### Waiting indicator
The prompt-bar control showing what the open *thread* is waiting for. It appears whenever the thread is parked: on a live *thread subscription* (an *event wait*), or on unfinished *sub-threads* (working, or asleep on their own event wait). A subscription lists the agent's reason, the event watched, a countdown to its deadline, and a **Stop waiting** button. A sub-thread is listed by title, and tapping it opens that thread. Without scrolling back, it tells a stuck thread from one asleep on purpose. The **Waiting** status says *that* it waits, on every list; this says *what for*, on the open thread.

A sub-thread row links rather than stops; you end a sub-thread on the sub-thread itself. A thread with a proposed change that waits only on sub-threads reads **Changes to review** instead, since its change can be applied.

Each subscription reads **watching for** and the event in plain words ("background job finished"), with its meaning on the tooltip. An event watched under a `condition` says **(with a condition)**, and tapping it opens the condition, exact event type included. The transcript's record of the wait opens the same thing.

The subscriptions section is headed **EVENTS**, since people wait for things to happen, not for subscriptions.

### Grouping button
The *thread drawer* header button that swaps between the *Folders grouping* and the *Ongoing grouping*. It shows where a tap goes: a folder while you are in Ongoing, an inbox while you are in Folders. Under Folders it carries the *Blocked* count. A tap into Ongoing selects Blocked when the button carried that count. Otherwise it keeps your last pick while that group has threads, or moves to the first group that has some.
See also: *thread drawer*.

### Folders grouping
The *thread drawer*'s default grouping: the Pinned, *Current* and Archive sections. Each section collapses on its own, and the thread filter shapes what they list. Reached with the drawer header's *grouping button*, which shows a folder while the Ongoing grouping is on screen.
See also: *Ongoing grouping*, *thread drawer*.

### Ongoing grouping
The *thread drawer* grouping by what is still going on with each *thread*: *Blocked*, *Review*, **Drafts** and *In flight*. Each group is a tile at the top showing its count, always all four and always in that order. One tile is selected, and only its threads are listed below the tiles. Tap a tile to select it and open the group's first thread, which on a phone happens only when the group holds just one. An empty tile is dimmed but can still be selected, and then the list says it is empty. A thread that matches no group is not listed here: find it under the *Folders grouping*.

Opening a thread from Blocked or Review lands on what needs you, rather than where you last read. Which target depends on the tile you opened it from.

A thread shows in every group it matches, so a failed thread with a ready *change* is in Blocked and in Review. Only one group is listed at a time, so no row shows twice. Every thread type is listed: the thread filter shapes the *Folders grouping* only, and its button hides here. Reached with the drawer header's *grouping button*, which shows an inbox and carries the Blocked count while the Folders grouping is on screen. **Show in Folders** always goes to the Folders grouping, where the thread has its place.
See also: *Folders grouping*, *thread drawer*.

### In flight
The *Ongoing grouping* group for work that is not finished and does not need you. It lists every Current and Saved *thread* whose status reads **Running** or **Waiting**: a turn running now, a thread watching for an *event wait* of its own, or a parent waiting on its *sub-threads*. A sub-thread sits under its parent when both are in flight. A thread waiting for your answer or one that failed is in *Blocked* instead. A finished one with a ready *change* or a *read request* is in *Review*. A thread in flight is never in either of those.

### Blocked
The *Ongoing grouping* group for threads that cannot go on without you. It lists every Current and Saved *thread* waiting for your answer or permission, one that failed, and a stopped *sub-thread*. Questions come first, then the newest. Its count rides the *grouping button* under Folders, and the thread-drawer toggle while the list is hidden. Opening a thread from Blocked lands on the open question or permission card, or on the failure. It was called Needs attention until ADR 0409.

### Review
The *Ongoing grouping* group for finished work that is your turn to look at. It lists every Current and Saved *thread* whose turn has ended and that holds a ready *change* or a *read request*. Opening one from Review lands on the change's turn when a change is ready, and otherwise on the start of the newest turn. A read request adds nothing to the Blocked count.

### Read request
A *thread*'s flag that its latest reply is worth reading, so *Review* lists it. The thread's agent sets it with the `request_read` tool, or a coding agent with `lucidos request-read`. It does so only when the reply holds what you asked for, such as a report or findings, never for a plain "done". A *trigger* run that found something may ask too.

Its row wears a filled dot, while a ready *change* wears a diff glyph.

It clears once the end of the latest reply has been on screen for a second, the same dwell a *seen target* uses. Your next message or an archive also clears it. Reading on one device clears it on all of them. A *trigger* run that asks lands in the *Current section* rather than Archive. It is not a notification: it neither pushes nor toasts. Not to be confused with a *read marker*, which says the agent took in your message.

### Thread link
The address that opens a *thread*, written as a markdown link target: `[Plan dinner](thread:myws/<thread_id>)`. Every row the `threads` tool and `lucidos threads` return carries one as `link`, as does a spawn result. Its workspace is always the one that served the row. A *draft* or a *held message* has no address of its own, so its link is its thread's.

### Settings link
The address that opens one Settings page, written as a markdown link target: `[Settings → System → Backup](settings:backup)`. The part after `settings:` is the same view id `navigate_ui` takes as `settings_view`. The label is the breadcrumb route, so a surface that shows plain text still reads the route. Engine notifications that name a Settings page link it this way, and the link opens the same page as the notification's tap.

### Reader fields
Four fields on each row that `lucidos threads list`, `lucidos threads search` and the `threads` tool return, saying what each *thread* holds and how to open it. `has_draft` says whether it holds a *draft*. `draft_preview` and `draft_length` give the draft's first 200 characters and its length, only when there is one. `link` is its *thread link*. Other thread reads leave them out.

### Thread drawer
The pane listing your *threads*, in one of two groupings: the *Folders grouping* (Pinned, *Current*, Archive) or the *Ongoing grouping*. Its header's title names the one on screen, or reads **Filters** while the thread filter is open. It hides when the header is too narrow to show it whole.

The **grouping button** in that header swaps between them. It shows where a tap goes: a folder while you are in Ongoing, an inbox while you are in Folders. Under Folders it carries the *Blocked* count; under Ongoing the Blocked tile shows it. While the list is hidden, the same count rides the **thread-drawer toggle**, so a thread waiting on you stays visible. Hidden means the drawer is closed on desktop, or another pane shows on mobile.

On mobile the toggle leads the *thread pane* header and opens the threads pane. The hamburger **menu drawer** sits at that header's trailing edge and slides out from the right. The thread drawer is the first of the three panes, and with the *thread pane* makes up the *Conversation* side. CSS container `.thread-drawer` (`FocusedPane = 'drawer'`; on mobile the leftmost swipe pane, `MobileView = 'threads'`). Always say *thread drawer*, never a bare "drawer". The hamburger **menu drawer** (Files / Apps / Plugins / Triggers plus pinned *apps*, `Drawer.tsx` / `drawerOpen`) is a different surface.

On desktop the toggle rests top-left in the header, drawer open or not. In the Mac app it sits right after the window buttons. Its icon is a window with a left column; on a phone, a bulleted list. With the thread drawer open, Filter follows the toggle, and the grouping button and Search sit at the far end of its header. On a phone, Filter and the grouping button lead the header.

Filter shows under the *Folders grouping* only, and pressed only while the thread filter is open. Its icon is an outline funnel, or a filled one while thread types narrow the list. Under the *Ongoing grouping* it fades out where it stands, so the grouping button beside it never moves.

Switching grouping, or between the list and the thread filter, dips through the background: one view fades out, then the other fades in. The header title switches at once, and the button icons fade quickly. With motion reduced, every change is instant.
See also: *Conversation*, *thread pane*, *content pane*, *Current section*.

### Thread pane
The pane showing the open *thread*'s transcript plus the prompt input, where you read what the *Lucidos Agent* did and type the next message. Second of the three panes, the other half of the *Conversation* side beside the *thread drawer*. CSS container `.pane-thread` (`FocusedPane = 'thread'`; on mobile the middle swipe pane, `MobileView = 'thread'`).
See also: *Conversation*, *thread drawer*, *content pane*.

### Thread Queue
System-wide admission control for the shared thread pool. Every path that creates running work shares one pool. Background spawns are an event *trigger* fire, a scheduled (cron) fire, or an agent-driven *sub-thread* or *coding-agent thread* spawn. Those come via `run_thread` / `run_coding_agent`, agent-mode `lucidos spawn-thread`, or cross-workspace task POSTs. User-initiated work is a person's chat or typed coding-agent thread. Within the *capacity policy* work runs at once; over capacity it waits.

User-initiated work is **prioritized, not exempt** (ADR 0008). It counts against the ceiling, drains ahead of background, and ignores the per-kind and per-trigger caps. It queues only at true pool-max, where a person briefly sees "requesting". `reserved_background` keeps that priority from starving triggers and cron.

Background ordering is FIFO, strict per trigger and best-effort across triggers. Cron fires **coalesce** to at most one entry per trigger: a cron fire carries no distinct payload, so a redundant one is dropped. A restart's duplicate cron rows collapse to one on recovery; event triggers keep strict FIFO.

Background entries persist in the `thread_queue` projection, event-sourced from `ThreadQueued` / `ThreadQueueAdmitted` / `ThreadQueueDropped` / `ThreadQueueCompleted`. So a restart re-queues work that never ran and drains it as capacity frees. User-initiated slots are in-memory only: a dead response is gone on restart, never re-fired. The **Thread Queue panel** shows it all (Running counts background + user; run now, drop, edit the capacity policy). A badly delayed trigger or a full pool raises notifications that tap through to the panel.
See also: `system-knowhow/thread-queue.md`, ADR 0008.

### Thread triage
The Lucidos Agent sorting the user's inbox threads into proposed actions, each with a reason drawn from facts, never from a title. It is the `threads` tool's `triage` and `apply_triage` actions. A *triage action* is one of `archive`, `pin`, `delete`, `follow_up` (needs the user), `dismiss_question` or `keep`. Trigger runs group per trigger: the newest is kept, older ones are proposed for archive, and an older run's lone question for dismissal.

`triage` records a `ThreadTriageProposed` event, and `apply_triage` runs only after the user replied to it in the same thread, re-checking every thread first. It never archives a thread that needs the user, and never deletes: a delete is the user's own, from the drawer (ADR 0192). See ADR 0349.

### Thread summary
A projected snapshot of a *thread*'s metadata: title, source, status, last activity, parent / trigger / repo links, coding-agent flags. The engine maintains it from the event stream in the `thread_summaries` table. **Same name everywhere**: DB table `thread_summaries`, Rust struct `ThreadSummary`, TS / JS SDK type `ThreadSummary`, wire JSON, this entry. Returned by `GET /api/v1/threads/list`, `lucidos threads list`, the `list_threads` LLM tool, and `lucidos.threads.list()`, the one canonical source of thread metadata. The *thread* itself, its event sequence, is the source of truth.
See also: `system-knowhow/lucidos-cli.md` § `lucidos threads list`, `system-knowhow/js-sdk.md` § `lucidos.threads`.

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

- **`waiting`** when the thread still holds a live *event wait*: it stopped on purpose and something will re-open it. Waiting rows keep full-strength text, a clock marker and a `waiting` tag.
- **`abandoned`** otherwise. Abandoned rows show a dashed strike-through and an `abandoned` tag.

A wait that resolves without the agent picking the list back up settles those items to `abandoned`. Usually that happens at the next terminator, since a delivery or a timeout re-opens the thread. **Stop waiting** re-opens nothing, so cancelling a thread's last subscription settles the list itself. Without that, an idle thread whose watch the user called off would read `waiting` forever.

The engine leaves the list alone on a cancel while a turn is live or promised (the agent standing its own watch down mid-turn). The agent can still finish the list there, and `abandoned` is final. A later subscription never un-abandons an item. To avoid the settle, the agent finishes the list or drops it with `todo_write` and `[]`.

**The agent gets asked before it walks away.** A turn about to end with open items and nothing to re-open the thread is stopped once, before its answer is sent. The agent then subscribes, finishes the list, or says it is not watching for anything. So an `abandoned` row usually means the agent chose to leave it. This stops a reply that promises to keep watching while nothing is.

**A settle is corrected by writing, never inferred.** The panel shows the agent's last list plus the engine's settling. So an item finished in a LATER turn reads `abandoned` until the agent calls `todo_write` again. The engine cannot see from the work that a plan item is done. So the tool tells the agent to re-write a list it picked back up.

**The agent can read the list.** Every `todo_write` call returns the resulting list, and `todo_write(action="read")` reads without writing. Both give each item's `content`, `active_form` and current status, engine-written ones included. A turn whose thread holds any non-`completed` item opens with the list in front of the agent. A thread with no list, a cleared one, or a finished one carries nothing. None of this applies under *self-curated context mode*, which has no `todo_write` and renders the checklist in the *working understanding*.

### Top-thread
A spawn with `relation: "top"` (the CLI default for `lucidos spawn-thread`). It has no parent and no callback wiring, so it appears in the main thread list as an independent thread. The *spawning thread* is **not** resumed when it finishes. It still records WHO launched it: the route popover on its first message names and links the spawning thread. "No parent" is about the callback, not provenance.

**No parent means the workspace, and two top-threads are siblings.** Every top-thread sits directly under the workspace root (ADR 0168 clause 1). The workspace is a container, never a place work runs, so nothing holds a turn there or can be delegated to it. Neither sibling has standing over the other: a top-thread reaches its own descendants on its own authority, and anything wider is the *workspace owner*'s button.

### Standing instruction
What lets a *thread* press one of the *workspace owner*'s buttons while the owner is not watching. A thread acts inside its own subtree on its own authority; anything wider needs this. Two shapes qualify and no third.

**A turn the owner opened.** They spoke into the thread from one of their own devices, and their words in that turn are the press. Lucidos may resume the turn after a restart, an API error or a hang. That is the same turn carrying on, so it keeps the instruction.

**A *trigger* firing the owner authorized.** The same decision, made in advance: they wrote the trigger, or switched it on. A trigger an agent wrote and fired carries nothing, or any thread could hand itself the owner's authority.

Nothing is inherited. A thread you spawn opens its own turn, so it carries none of yours. A thread without one can still ask for a single act with an *owner approval*. See `system-knowhow/orchestrating-sub-threads.md` for what this permits and refuses.

### Owner approval
Your **Allow once** on an *owner approval card*: a *question card* Lucidos writes itself, asking whether a *thread* may do one thing outside its own subtree. Examples are creating a top-thread, or applying another thread's change. The agent asks for it with `lucidos ask-owner-approval`, and its reason shows below as the agent's.

Your Allow lets that thread do that one thing, to that one thread, once. It expires when the thread's next turn starts. **Don't allow**, Cancel, or an answer from anyone but you grants nothing. A card never asks to answer another card for you.

<!--gloss-trigger-start-->
### Trigger
A workspace configuration that fires on a schedule (`run.cron`) or on one of its *event subscriptions* (`on`). The `run` takes one of two shapes:
- `run.type: "intent"`: spawns a *trigger thread* whose LLM gets `run.intent` (the user's voice, non-technical prose) as a user message. It finds the knowhow it needs via `load_knowhow` at fire time; there is no per-trigger knowhow allowlist.
- `run.type: "script"`: runs the *script* at `run.path` directly, no LLM. On event fires the engine sets `TRIGGER_EVENT_TYPE` / `TRIGGER_EVENT_PAYLOAD` / `TRIGGER_EVENT_ID` / `TRIGGER_EVENT_THREAD_ID`. So the script can branch deterministically and deep-link a notification to the originating event. Right for a deterministic transformation that needs no LLM judgement.

Lifecycle (both shapes): defined by `TriggerCreated`; each firing emits `TriggerStarted` then `TriggerCompleted`. The panel row shows its **last-run status** (OK / failed) beside the last-run time; there is no built-in run-history view. For more, ask the *Lucidos Agent* ("what has this trigger been finding?") or build an *app* on its *event* stream.

The row also shows the **next runs**: the next few fire times, merged across every cron expression. A cron that can *never* fire (`0 0 9 31 2 *`, Feb 31) is rejected at create and update, naming the offending fields. A trigger stored before that guard still loads, but wears a **schedule error** rather than the "No more runs" of a spent one-shot. Neither has a next run, but one never worked and the other finished its job.
See also: `system-knowhow/triggers.md`, `docs/taxonomy.md` § Triggers.
<!--gloss-trigger-end-->

### Off-schedule run
A firing of an existing *trigger* asked for by a person rather than by its schedule or an *event subscription*: `triggers(action="run")`, `lucidos triggers run --id`, `lucidos.triggers.run(id)`, or the **Run once** button on the trigger's panel row. It is **indistinguishable downstream** from a scheduled fire. Same `TriggerExecuted` / `TriggerCompleted`, `last_run` and last-run status, *trigger thread*, `go_to_review` routing and *side-effect grant*, and no actor stamp. A manual run also suppresses a redundant catch-up of the slot it covered.

Distinct from the *Thread Queue* panel's **Run now**, which force-admits an *already queued* entry and cannot create a fire. Refused when the trigger is paused (resuming restores the schedule but runs nothing). Refused too when it has no cron schedule; emit its subscribed event instead to reproduce an event-driven fire. A fire of the same trigger already active or queued is reported, not started, since cron fires coalesce to one pending run.
See also: `system-knowhow/triggers.md` § "Running an existing trigger once, off-schedule".

### Trigger definition
The on-disk `trigger.toml` at `data/triggers/<slug>/trigger.toml`: a **derived read-model** of a *trigger*'s durable config. The engine writes it on create and update, removes it on delete, and rebuilds it from events on boot. It is NOT the source of truth (events are), and **not version-controlled** (it sits in the repo's local `.git/info/exclude`). The scheduler never reads it, so a hand-edit changes nothing that fires and is overwritten. It makes a trigger inspectable, and lets a *plugin* SHIP a trigger by declaring one (install parses it into a `TriggerCreated`). See ADR 0019, `system-knowhow/triggers.md` § "On-disk trigger definition".

### Event subscription
A standing request to be told when a matching *event* happens: an `event_type` plus an optional payload `condition` scoped to that event. One shape, one matcher, two species. Without a condition it matches every event of its type; with one, only events whose payload satisfies it.

A **trigger subscription** is one entry in a *trigger*'s `on` list. On a match it **spawns a new thread** and **stays armed**. That makes a trigger a standing rule: it outlives every thread it starts and fires until paused or deleted. A trigger may carry several, and each filter constrains only its own event, so payload shapes never interfere.

A **thread subscription** is one an existing *thread* armed for itself, with `await_event` (the *Lucidos Agent*) or `lucidos await-event` (a *coding agent*). On a match it **resumes that thread** and is **spent**, so the next one needs a new arming. The code and event log call it an *event wait*.

The same matcher decides both, so a `condition` that fires for one fires for the other. A thread screen shows only the thread species.

**What can be subscribed to: any persisted event.** A thread event, a *domain event* your workspace emits, or a persisted system fact such as `BackupCompleted`. A transient engine frame writes no row and reaches no matcher, so both species refuse it at the tool boundary. A transient *domain event* is the exception, since your workspace named it. It matches live only, with no row for a catch-up to replay.

**The name is checked when you subscribe, not when nothing arrives.** Matching is exact, so a typo would arm clean and wait forever. Both species refuse a nonexistent engine name, naming the near match, and a retired one, naming its replacement. Any other name is accepted, with a warning when unseen here, since a domain event not yet emitted is legitimate. Look names up with the `events` tool's `event_types` action.

**The condition's paths are checked too.** A *field path* that no payload of that type carries fails as silently as a misspelled name. So each is checked against the most recent stored payloads, and a path in none of them draws a warning naming the real path. Not a refusal: an optional field can be absent from a sample.
See also: *trigger*, *event wait*, *waiting indicator*, `system-knowhow/triggers.md` § "One trigger, multiple events", `system-knowhow/thread-events.md` § "Check the name before you subscribe".

### Field path
A key in an *event subscription*'s `condition`, naming one value inside the event payload. A bare name reads a top-level field, and dots read downwards: `workflow_run.event`. Both subscription species use the one matcher, so a path means the same to a *trigger* and an *event wait*.

Three rules. Resolution prefers a key that exists verbatim at every level, so a payload field literally named `a.b` stays nameable. A path that resolves to nothing is JSON null, like a missing top-level field. A numeric segment is an object key, never an array index, so an array anywhere on the path ends resolution.

See also: *event subscription*, *trigger*, `system-knowhow/triggers.md` § "What a condition can say".

### Trigger thread
A *thread* spawned by a *trigger* firing, marked `source = 'trigger'`. Its LLM has the same knowhow access as a chat thread: the system prompt advertises the intent registry, and the LLM calls `load_knowhow` when relevant. No per-trigger knowhow allowlist. Terminal event: `TriggerCompleted`.

### Trigger group
A user-visible folder that organizes *triggers* in the triggers panel. Pure label: no agent, no schedule, no code. A trigger belongs to at most one group via `group_id`; ungrouped triggers show under an implicit "Ungrouped" section. It can gather an emergent workflow (triggers chained by `emit_event` → `on_event`) into one group without changing how they fire. Lifecycle: `TriggerGroupCreated`, `TriggerGroupRenamed`, `TriggerGroupReordered`, `TriggerGroupDeleted`. A group with members cannot be deleted until its triggers are reassigned or deleted; panel order follows `order: i32`, ascending.

### Typed answer
What you type into the composer while a *question card* waits. It is that card's answer, so it shows on the card under **Your answer**, never as a message below. While sending, the card's header reads **Sending**. If Lucidos does not answer, it reads **Not sent**, with a retry icon under your answer and the options live again. Tapping an option or typing a new answer replaces it.

A typed answer need not answer the question. It may ask back or say something else, and the agent then responds and asks again.

A tapped option behaves the same way: with no answer, it stays marked **Not sent** with a retry icon. Tapping **Not sent** shows what went wrong.
See also: *unsent message*.

### Unsent message
A message you sent that got no answer from Lucidos, so it may never have arrived. Lucidos first retries it quietly, up to three times over about 13 seconds, while it still shows as sending.

Only then does it stay in the *thread* under a **Not sent** card that quotes it, with a **Retry** button. A *typed answer* to a waiting question card stays on that card instead. It never vanishes or reads as a failed reply. Retry sends the same message again. If the first did arrive and only the answer was lost, Lucidos recognizes the repeat and runs it once.

It is kept on the sending device, so a page reload brings it back, Retry included. That covers a send the reload cut off, whose card says so. If the message did arrive after all, it shows as sent. **Discard** removes the card without sending. Other devices never see it, and your *draft* never holds a copy.
See also: *draft*.

### Urgent follow-up
A *child follow-up* marked `urgent: true`, which stops the child's current turn so it reads the message now, not at its next natural break. An ordinary follow-up queues, because a steer should never throw away an in-flight build. Urgency loses whatever the interrupted turn was mid-way through. So use it for messages that cannot wait (a cancellation, "stop, you are working from a wrong assumption"), not for hurry.

Without it the wait is unbounded: a child in a long tool call reads a queued message only when the call returns. A *coding-agent thread* in a ten-minute blocking wait really sits on a STOP for ten minutes. The interrupted turn ends as "Superseded", not "Canceled", since the work is steered, not abandoned. So the parent gets no false completion card for a child that carries on.

Two caveats. A child parked on a question waits on a *human*, so urgency cannot unblock it. On a **Codex** child it changes nothing: a Codex turn reads no queued message until it ends, so every follow-up already stops the turn. Ask for what you mean, whatever the child's backend.
See also: *child follow-up*, *coding agent*.

### Call toggle
The handset button in the prompt input, beside the follow toggle, that starts and ends a *voice session*: press to call, press again to ring off. It is green while a call is up and grey while hanging up. Point at it and it turns red, which is what the next press does. A slow connect says it is waiting for microphone access, which the browser asks for once per launch.

It is a toggle, not a hold-to-talk microphone: the microphone stays open for the whole call. It sits in the *home thread*'s prompt input, the one place a call runs.

**Holding it picks which microphone a call opens**, from those this browser can see. Otherwise a call takes the system default, which on a Mac can be a nearby iPhone. The choice is per device and applies from your next call.

**Two things hide it, and a live call overrides them both**, so you never lose the ring-off button. Voice ships off; Settings → Models → Voice is the switch. And every thread but the home thread draws no handset.
See also: *voice session*, *home thread*.

### Voice session
Talking to Lucidos out loud in your *home thread*. Voice is a **mode of a thread**, not a kind: the same thread, history and agent. The composer stays live, so you can speak a sentence and type the next, both in the same conversation.

It starts and ends at the *call toggle*. It also ends when you open another thread, when the connection drops, or when the engine goes away.

**A call runs on the home thread, and nowhere else.** Every other thread offers no call: the handset is absent, and the engine refuses one placed any other way. From Home, Lucidos can still reach the rest of your work: it reads any thread and follows one up when you ask.

During a call the handset stays green; there is no separate panel. What each of you says lands in the thread as it is said. Your bubble appears as you start speaking and pulses until your words arrive. Speaking over the assistant stops it mid-word, as on the phone.

**Experimental, and off until you turn it on** at **Settings → Models → Voice**. The voice page also holds the call's two models, its voice, and what it loads before starting. One model talks; the other turns your speech into text. A call with no talker says so, with a button straight there.

**Speaking wakes the thread**, unless Lucidos answers you itself. A sentence it hands on lands as a message and starts an ordinary turn, as typing would. One it answers from what it knows starts nothing, but is still written down.

**You can answer out loud what Lucidos is waiting on.** A pending question or permission is read to you, and saying your choice settles it, writing what the on-screen buttons write. The two widest "Always allow" choices stay on screen only. So a voice permission is allow once, allow for this conversation, or no. While something waits, no new request starts: Lucidos is stuck inside the question, and says so.

**How much of that works depends on the talker.** A Realtime one settles everything out loud. GPT Live holds no tools, so it settles a question by sending your words verbatim, as typing a reply would. There a permission is a tap, and so is ringing off. The talker row in Settings says which you are on.

**Saying you are done ends the call.** "That's all, thanks" rings off after the goodbye, and running work keeps running.

**The audio is thrown away, the words are not.** Nothing is recorded. The thread keeps what you said, what the assistant answered, and what was said out loud. It also keeps when the session opened and closed, and what the call spent.

**The transcript says which of it was spoken.** A spoken message carries a small handset above it, so a half-spoken, half-typed conversation reads back correctly. The bubble is the one a typed message gets, and the mark appears either way, whether the assistant answered at once or went to look. A spoken answer to a question it asked is written on the card as the answer itself.

**What the assistant said out loud is a bubble inside the turn**, under the same handset, marked as cut off if you talked over it. It sits beside the written answer, not instead of it.

**What you hear is not what you read.** The written answer is for reading, and can carry tables, code and links. The spoken reply is what it *means*, in a sentence or two. A long answer is spoken as the short version plus an offer of the detail. The full text waits in the thread.

**One entity, honest about what it knows.** You hear one assistant, in the first person. What it answers instantly is bounded by what loaded when the session opened. For anything else it says it is checking, and comes back. It never claims to have done something it did not do.
See also: *chat thread*, *thread*.

### Wake question
An `ask_user_question` called with exactly one option. The *Lucidos Agent* uses it to offer one tap-to-send follow-up, like "Show results" or "Stop sweep", instead of a typed wake message. The agent's context lives in the `question` text; the option's label is the user-voice prompt the tap sends. The thread shows the "?" attention status (`WaitingForUserAnswer`) until the user taps. A free-form message also auto-resolves the pending question.
See also: `system-knowhow/running-python.md` § The drain pattern.

### Release notice
Something a release needs you to KNOW or DO, shown once on the first open after you upgrade. "Run a workspace audit" was the first. The release notes in *What's New* say what changed; a release notice says what to do about it. Most releases carry none, which keeps the ones that do worth reading.

It arrives as a card over the workspace with the instruction and, where there is something to start, one button. That button SENDS an ordinary sentence as a new message, like the first-run welcome's setup-interview button. So you see what was asked and can reword it later. Anything else it points at is a link in the text.

**One at a time, oldest first.** Skip several releases and they come in order, "1 of 3", each replacing the last as you answer it. A later notice waits until the one before is answered. **Got it** answers the one you are reading. Escape and the X close the card and answer nothing, so it returns next time you open the workspace.

**A shared button comes once.** When several owed notices offer the same button, such as "Audit my workspace", only the last draws it. The earlier ones say it comes later, so one audit covers them all.

Every notice this workspace has met lives on its own page, **Settings > System > Release Notices**. Go there if you closed the card mid-task, or tapped one action and want the others. It sits apart from *What's New*, which says what CHANGED.

Anything still owed leads the page. Answered notices fold away behind a shut **Already answered** row, each ticked and struck through: a record of what the release asked. Their buttons stay live, since **Got it** means you READ the notice, not that you carried it out. So after acknowledging the card, the audit is still one tap away on the answered row.

Answering is remembered per *workspace*, not per *device*: settle it on your laptop and your phone will not ask. The workspace's place in the sequence is the *release notice cursor* (`docs/glossary.md`). A brand new workspace starts level and hears from its next upgrade, so nothing lands over the first-run welcome.

### What's New
The release notes panel, at **Settings > System > What's New**. It lists every published Lucidos release newest first, with the running one open and marked **Running**, and the rest as expandable rows. The notes are the project's changelog prose.

Opening the panel reads the published changelog, so releases newer than your copy are listed too. When it cannot be reached, the panel falls back to the copy **inside the engine binary**. That keeps it working offline and on an install with no source checkout. The fetch happens on panel open, never on a schedule.

A release the updater is **offering** sits above that list, with the control to get it. With no offer, the control sits on the newest release ahead of you, since the changelog arrives before the update check. On the desktop app the control is **Update & Restart**, and the row wears no chip. A browser or phone has no updater, so the row keeps its **Available** or **Newer** chip and offers **How to Update**. That opens the page for your kind of install.

One exception: with the desktop app open on the Mac that runs the workspace, a browser or phone gets **Update Desktop App** instead. The desktop app installs the release and restarts Lucidos, while your phone shows progress. It works only where nobody needs to be at the Mac. So it is not offered from the disk image, or from a folder that needs an administrator password.

An offered version is newer than the running binary, so its notes are not in that binary's history. They arrive with the update check. So the offer's notes appear only where a real update is pending, and never fall back to the installed list. That would show your CURRENT version's contents under the heading of the one you were about to install.

Three ways in. The Lucidos menu's version row opens it (tap the Lucidos mark in the header). That row carries a dot until this device reads the notes for the release it runs; the dot is per device. The update notice on Settings > System > Overview links here too, as does the update toast, next to Update & restart.

The two update links open the release they announced, since they answer "what is in it". They open the panel at the top, so an offer never lands you mid-list. Every other way in opens the release you are running.

### System attention badge
A small blue dot on the way into **Settings > System**, saying something there awaits you. It appears at every step in: the hamburger that opens the menu drawer, that drawer's **Settings** row, the **System** row, and the page's own row in the System list.

Two things raise it, on two pages. A Lucidos update you can take is *What's New*. An unanswered *release notice* is **Release Notices**. Both are work, which separates the dot from news you might simply read. Hovering the hamburger says which.

Upstream of those pages one dot stands for both. On a System row it means work on that row's page, so an update never dots Release Notices.

**It clears by being acted on, never by being seen.** Install the update or answer the notice and it goes. Opening and closing the panel changes nothing, nor does closing a release notice card with Escape.

Distinct from the dot on the Lucidos menu's version row, which means you have not read the notes for the release you run. That one is per device and clears when you open the panel. This one is about the workspace, and about work.
See also: *What's New*, *release notice*.

<!--gloss-webhook-start-->
### Webhook
An endpoint you point a third party at, so their service can tell Lucidos something happened. Each webhook emits exactly one *domain event*, which a *trigger* can then react to. Manage them at **Settings > Webhooks**, or with `lucidos webhooks`.

**The event is pinned when you create the webhook**, and no caller can change it. So an endpoint you gave GitHub only ever fires the event you chose, whatever GitHub posts. For several kinds of message from one sender, fire one event and discriminate in the trigger's `condition` on the sender's own fields.

**Only you create one.** No agent tool or app can, for the same reason an agent cannot grant itself a *side-effect grant*: a webhook opens a door, and an agent must not widen its own authority.

A delivery has to prove itself. An unsigned webhook carries a **token**, shown once at creation, that the sender returns as `Authorization: Bearer <token>`. Or it carries a **signature** configuration, as GitHub, Stripe and Slack sign the request body with a shared secret. Those senders attach no token, so a signed webhook gets none. The secret is a *credential* you save once and name here, never a copy beside the webhook.

**The sender decides which side invents that secret**, and the form follows. GitHub takes whatever secret you put in its webhook form, so Lucidos offers to generate one and shows it once for pasting. Slack and Stripe issue their own in their console, so you paste those in. Either way you may name an already saved credential instead.

**A signature can be changed or removed without changing the URL**, so a rotated secret or wrong signature header is fixed in place. A webhook carries exactly one of the two verifiers. Adding a signature drops the token; removing one mints a fresh token, shown once.

A delivery becomes an event shaped `{summary, headers, payload}`. The sender's body is under `payload`, so a trigger condition reads `payload.action`. Allow-listed request headers are under `headers`, read as `headers.X-GitHub-Event`.

**A sender will resend the same delivery**, and by default that emits the event again. GitHub retries a slow response and offers a Redeliver button; Stripe retries for days. Every arrival stays on the event log, so you can see how often a sender repeats. Switch on *delivery deduping* to count each delivery once.

Deliveries arrive on their own port, the *hook socket*, which answers webhook URLs and nothing else. That makes it the one surface safe to expose to the public internet with `tailscale funnel`.
See also: *trigger*, *domain event*, *credential*, *delivery deduping*, `system-knowhow/lucidos-cli.md` § `lucidos webhooks`.
<!--gloss-webhook-end-->

### Delivery deduping
A per-*webhook* setting that makes a resent delivery emit nothing instead of firing the event a second time. **Off by default**, so every arrival is an event unless you ask otherwise. Set it with `lucidos webhooks`, which is also where a signature is configured.

You name the header carrying the sender's delivery id (`X-GitHub-Delivery` for GitHub) and a window. A repeat of that id inside the window emits nothing. It gets a success and the first delivery's event id, so the sender stops retrying. A repeat arriving while the first is still being handled is told to retry, since that one may yet fail. Name no header and the body itself is the key, which also collapses two different but identical-looking deliveries.

The window defaults to an hour and can go up to seven days. Setting it to `0` turns deduping off again.

Only *this* endpoint's deliveries are compared, so two webhooks fed by the same sender both fire, as two subscriptions should.
See also: *webhook*, *domain event*.

### Ingress probe
Lucidos knocking on its own public webhook address from outside, every 15 minutes, to check a sender could still reach it. It sends one unsigned POST to a real *webhook* and expects to be turned away. A refusal is the good answer: it proves the whole path is alive, from the public relay through the *hook socket* to the verifier.

**It leaves the machine, and probes every address separately.** A loopback check would pass while the outside world got nothing. The public name usually has several addresses and a sender reaches one, so each is probed and judged per address family. An IPv4 outage is an outage even while IPv6 answers, which is the failure this catches.

It runs only with something to protect: at least one enabled webhook, and a funnel serving the hook port. Two failed rounds in a row declare an outage, and one good round ends it. A round that reached no address declares nothing and changes nothing, since it measured nothing. You see a bar across the app and a line on every enabled row in **Settings > Webhooks**. The bar's **Discuss** button hands the agent the whole diagnosis, address by address.

Lucidos only reports. It emits `WebhookIngressDegraded` and `WebhookIngressRecovered` once each per outage, not per round, and a *trigger* decides what to do. It never re-arms the funnel: that is your tailnet, and the fix depends on who should be delivering.

**A round your computer slept through proves nothing.** It cannot declare an outage, and after a wake the count starts over, so a sleeping laptop never reads as a dead funnel. Senders like GitHub do not resend a delivery that failed, so after a sleep of 30 minutes or more Lucidos emits `WebhookDeliveriesSleptThrough` once. It sends no notification unless a *trigger* asks for one.
See also: *webhook*, *refusal check*, *trigger*, `system-knowhow/remote-access.md`.

### Refusal check
The other half of the *ingress probe*: what a *webhook* does with deliveries that DO arrive. Also every 15 minutes, but it sends nothing. It reads each webhook's own record of what it accepted and turned away.

**The probe cannot see this fault, by design.** It expects to be turned away, so a webhook that refuses every real delivery passes it perfectly. A hook switched off by hand once threw away eighteen days of deliveries with every check green.

A webhook is reported after at least three refusals spanning at least half an hour. Both are needed: three refusals in four seconds is one bad payload. **A switched-off webhook is reported after one**, since nothing was read and the delivery is certainly gone.

The two cases always use different words. A switched-off webhook verified nothing, so its signature and secret are fine and the fix is the Enable button. Calling it a signature problem would send you looking where there is nothing to find.

You see a bar across the app and a line on the webhook's row in **Settings > Webhooks**. The bar's **Discuss** button hands the agent the declaration: which webhook, which case, how many deliveries and for how long. Use it for the verification case, to check the secret still matches the sender's. It clears when a delivery verifies, the webhook is switched back on or deleted, or nothing arrives for a fortnight. It emits `WebhookDeliveriesRefused` and `WebhookDeliveriesRecovered` for a *trigger* to act on, and fixes nothing itself.
See also: *webhook*, *ingress probe*, *trigger*.

### Widget
A small interactive answer that lives in a thread: an *app* whose *app manifest* says `"kind": "widget"`, made by `create_app` with `kind: "widget"`. It shows inline at the turn that made it, as a *widget card*. It has a chip on the thread's *widget shelf* only once pinned. It fits one phone screen with no scroll area inside, and its name says what it does ("Flight picker"). It runs in the same frame as an app and reaches exactly what an app reaches. It is never in the apps list, so the list stays the user's persistent interfaces.

The thread filter lists each widget a coding-agent thread worked in under its own Widgets heading. A deleted widget stays there, marked deleted, because `AppDeleted` records its kind (ADR 0404).

Its manifest records the thread that made it (`origin_thread_id`). *Delete (a thread)* removes that thread's own widgets, and nothing else removes one. "Make app" builds a separate, full-size app from it and leaves the widget as it is. Not an OS home-screen widget, which is a presence surface outside Lucidos.
See also: *reusable widget*, *widget card*, `system-knowhow/building-an-app.md` § Widgets, ADRs 0402 and 0407.

### Reusable widget
A *widget* whose manifest says `"reusable": true`, so any thread may show it (`widgets(action="show")`). The apps panel lists them in a Widgets group, and the agent sees them in its prompt. Its origin thread stays recorded: "Stop reusing" clears the flag, and the widget then belongs to that thread again, so it is refused once that thread is deleted. Deleting the origin thread never removes a reusable widget.

### Widget shelf
The row of chips in a thread's title row, one per **pinned widget**. Showing a *widget* adds no chip: the user or the agent pins the ones worth coming back to, such as a picker. A chip never squeezes the title: on a phone the shelf takes its own line under it, and on desktop it sits beside the title when the row has room. A chip tap drops the widget open under the title on every device; it never scrolls to the turn. A right-click, a long press or the ⋯ in the widget's bar opens its menu: Open in Canvas, Make reusable, Make app, Show in thread, and Pin to shelf or Unpin from shelf.

Unpinning takes the chip off the shelf. The *widget card* and the files stay. The shelf is the pinned part of the *thread widgets*, so it reads the same on every device and after a reload. The agent pins and unpins through the `widgets` tool, and the CLI through `lucidos widgets pin` and `unpin` (ADR 0407).

### Thread widgets
Every *widget* shown in a thread, pinned or not: each one's name, `reusable` flag, pin state and the turn that last showed it. The *widget cards* read it, and the *widget shelf* keeps its pinned ones. It derives from the thread's `WidgetShown`, `WidgetPinned` and `WidgetUnpinned` events. Read it with `widgets(action="thread")`, `lucidos widgets thread` or `GET /api/v1/widgets/thread`.

### Widget card
The inline card a `WidgetShown` draws at its turn: the *widget*'s bar (icon, name and menu) with the widget under it. The frame is as tall as the content the widget reports, so it never scrolls inside the thread's own scroller. Past about one phone screen, the card clips with a fade and an Expand button that opens the widget full size in Canvas. "Show in thread" lands on it. Unpinning leaves it in place (ADR 0407).

### Workspace
A user's complete Lucidos instance: one PostgreSQL database inside the shared
Lucidos Postgres cluster, plus a `data/` directory (artifacts, apps, knowhow,
triggers, intents, config, auth-modules, scripts, themes). Multiple workspaces run
concurrently, each its own isolated engine and database. A single *workspace
gateway* (dev term) fronts them and addresses each by its *workspace address*,
the path prefix `/<slug>/`.

The workspace picker is at `/~/`, or just `/` when there is more than one. There
you switch, create, rename and delete workspaces, and toggle each one's
*auto-start*. You can also **restore one from a backup**: drop in an encrypted
`.enc` backup file and its backup key. The name comes from the backup, and you
must change it if its address is taken. Every workspace you have launched stays
**listed** after it stops, and opening a stopped one starts it.

You can also **switch without the picker**. The Lucidos menu's Workspaces row
(tap the Lucidos mark in the header) unfolds the same list, marking the current
one. Each row shows whether that workspace is running. Unread counts sit in the
menu's notifications rows above the list, not on these rows.

The installed desktop app gives each workspace its own window, and brings it
forward if already open. A browser tab or installed web app switches in place;
in a browser, cmd-click or middle-click opens a tab. A right-click always offers
the other one.

Only switching lives in the menu. Creating, renaming, deleting, restoring and
starting or stopping go through the picker, linked from the list's last row. On
first run the picker offers both ways in side by side: name your first workspace
(suggesting "personal" or "work"), or restore one from a backup. Nothing is
auto-created.

### Workspace address
The path a *workspace* is served at (`/personal/`), also its folder and database name. It derives from the name at creation (lower-cased, anything not a letter or digit turned into `-`). Then it is **fixed forever**: a rename changes the label, never the address. The picker shows the address only when it would surprise you: after a rename, or when two workspaces share a label.

**No two workspaces can share an address, and no two can share a name either.** Creating or renaming to a taken name is refused, naming the holder. Names match ignoring case and surrounding spaces, since "Work" and "work" look the same in a list. A name a running restore is about to use counts as taken, and you can wait for it.

The address alone may be taken, by a workspace since renamed. A create then gets the next free address, `/personal-2/`, and the picker says so first. A restore is refused instead, since a suffixed address would quietly make a second copy. Workspaces that shared a name before this rule keep working, and the picker shows their addresses.

### Workspace font
A font installed into the *workspace*, by you, the agent or a *plugin*, rather than shipped in the *font catalog*. It lives in `data/fonts/<slug>/`: a `font.json` naming its label, group and font files, beside the files. Its id is `ws-<slug>`.

The group decides where it fits: a `sans` or `serif` font can be the UI font, and a `mono` font can be the UI font or the code font. The engine serves it from the workspace, never from the internet, and lists it after the catalog fonts in `GET /api/v1/fonts`. The `font-family` *preference* and a *theme* name it like any catalog font. See `system-knowhow/workspace-fonts.md`.

### Workspace audit
A read-only sweep of what your *workspace* holds against how Lucidos expects it to look today: *app*s, *trigger*s, *knowhow*, intents, scripts and *artifact*s. It writes a report and changes nothing. Ask for it by name, or take the button on the *release notice* that offers it. Drift is mostly silent: a trigger on a renamed event stops firing, and an app that lost a browser capability quietly forgets your settings.

Your words pick one of two shapes. An unqualified "audit my workspace" is the **full pass**: every check, every time, compared with the last report but never inheriting its coverage. Naming one break ("migrate my apps off `localStorage`") is a **targeted run**: the same check and fix guidance over one surface, straight through to the fixes. A targeted run reports only what it examined, so it never reads as a clean bill of health. Fixes happen only if you ask: the sweep proposes, and a *coding-agent thread* per app does the work.

Driven by `system-knowhow/workspace-audit`. Siblings: *workspace learning* asks whether the conventions match you, and the *setup interview* asks whether the workspace matches you.

### Auto-start
A per-workspace toggle in the workspace picker: whether the *workspace gateway* starts that workspace's engine when the gateway starts. **On** means always-on: it spawns on every gateway (re)start, including a packaged install's login-launched gateway. So its triggers, scheduled tasks and notifications work with no window open. On is the default both for creating a workspace and for restoring one, which likely has triggers and scheduled tasks already. **Off** (the picker row says "Only runs while open") keeps it listed, but its engine starts only when you open it. An already-running workspace is re-adopted across a gateway restart either way.

## Advanced (coding agents)

The surface for users running coding-agent workflows: Claude Code and Codex, the Apply / Discard flow on Lucidos's own repo, the hardening gate, and the external-repo variant for user-added repositories. Most users never meet these.

### Apply
The user-clicked action that merges a *coding-agent thread*'s worktree branch into `main`. **Always non-disruptive**: it never restarts the engine on click. The button reads **Apply** for a change that needs no restart. It reads **Apply\*** when the change is engine-affecting, a compact asterisk marker the tooltip explains.

**Asking the agent to apply asks the same question the button does.** A *thread* that has not settled is refused either way (ADR 0233). For a *settling* thread the agent is pointed at a *standing apply*. For one parked on a question or a failed turn, the agent comes back to you.

There is no "Apply & Restart" label: even for an **Apply\***, the restart is the separate *Switch to new version*, never Apply itself. A merged *change* touching engine-affecting source also starts a background engine rebuild (dev only). That surfaces later as *New version available / Switch to new version*. If the session didn't run *hardening* first, Apply runs it synchronously and the user waits. Source: `crates/lucidos-engine/src/engine/git_ops/restart_detection.rs` (`files_require_restart`).

### New version available / Switch to new version
The affordance for moving the *engine* onto a newer version. Lucidos shows **"New version available"** (a dismissible toast plus a persistent control-panel badge). It offers **"Switch to new version"**: a deliberate, user-triggered restart behind a brief blocking overlay and a "Starting new version…" notice. Two sources feed it. In a **dev** build, *Apply*-ing a *change* to Lucidos's own source rebuilds the engine in the background, and the old engine serves until you switch. In a **packaged** build, the gateway finds a newer published release.

The packaged path **narrates itself**, since it downloads and swaps a whole signed bundle before restarting the stack. The toast and **Settings → System** show the same named *update phase* throughout. The phases: *Checking for updates* → *Downloading* → *Verifying* → *Installing* → *Restarting background services* → *Relaunching*. Downloading shows bytes transferred, and a progress bar when the server declares a size. The download can be **cancelled**, since nothing is written yet and the update stays on offer. From *Installing* on there is no half-installed state to return to, so cancel is withheld; a failure names its reason.

After a user-initiated switch, in-flight threads auto-resume with no manual *Continue*. *Coding-agent threads* pick up where they left off. Chat and trigger threads re-enter with a note summarising what the interrupted run did. A thread parked on an *AskUserQuestion* answer is preserved; answering resumes it. A restart that was **not** a deliberate switch (a crash) never auto-resumes: those threads keep **Continue**, so work that may have crashed the engine can't loop.

The engine toast **and its control-panel badge are ONE signal**, keyed on the on-disk binary build id. Both appear only once the rebuild produced **a newer binary to switch onto**. Never at *Apply* time, and never for a build that produced nothing newer. "Newer" is literal: co-located dev workspaces share one build output, so a provably **older** binary is never offered. Dismissing the toast only **defers** it: the badge stays lit, and a genuinely newer build re-surfaces the toast.

Distinct from the client-bundle **"New version available"** toast with its Refresh action, which reloads the frontend page, not the engine. Its badge and toast are **likewise one signal** that defers on dismiss, and the badge persists while the loaded bundle is stale. In dev the engine serves only a client compatible with itself, a boot-pinned `dist/` snapshot. So a reload never loads a newer client against an older engine. A **frontend-only** *Apply* leaves the binary unchanged, so the engine advances the served client **in-process** and offers Refresh. A **mixed** change advances the client only with the engine, on a switch.

Source: `crates/lucidos-engine/src/engine/engine_version.rs`, `crates/lucidos-engine/src/api/frontend_snapshot.rs`, `crates/lucidos-engine/src/engine/frontend_refresh.rs`, `crates/lucidos-app/src/store/actions/engine-update.ts`, `client-update.ts`.

**The gateway polls once per machine** (ADR 0108, `crates/lucidos-gateway/src/release_check.rs`), for every install shape, not just the macOS app. The client still installs, so narration, cancelling and failure work as above. A headless install has no client, so it gets the `install.sh` command to re-run. Settings > System > Overview carries the off switch, and `PRIVACY.md` says what the poll sends.

**Taking the offer opens a confirm that says what the switch brings**, rather than restarting at once (ADR 0229). It names the version and lists only the commits the switch brings, under *New*, *Fixed* and *Improved*. **Switch** commits; **Later** just closes it and is not a dismissal, so the toast stays. The same confirm opens from Settings → System, and from the Lucidos menu's **Restart** row while it wears the *New version* pill. A packaged build reports no commits, so its confirm lists the applied *changes* the restart activates, grouped by proposing *thread*. A plain restart shows that too, under its shorter question.

The button for *Switch to new version* reads **Switch**, on the toast and in the confirm.

**A version not built yet is its own, quieter signal** (dev only). With new engine code in source and nothing built, there is nothing to switch onto. So the brand mark carries a small dot instead of the switch badge, and the toast offers *Rebuild*. It defers on dismiss like the one above, keyed on the checkout's commit, since no on-disk build exists yet. Tapping the dot brings it back. If a rebuild for that commit already produced nothing to switch onto, the button is withheld and the toast names the real fix.

One packaged failure is reported differently: if the swap leaves no runnable app on disk, Lucidos does **not** restart anything. It says so and tells you to reinstall from the `.dmg`, since a retry has nothing left to install over. The background service keeps running the loaded version, so your workspaces stay up until you reboot.

### Apply All
The user-clicked action that runs an *Apply* on every pending *change* in one batch. UI label: **Apply All**, beside each row's *Apply* on the changes panel, whose caret holds Set aside and Discard.

The batch skips exactly what a per-row *Apply* refuses: changes whose thread has not settled, and changes with no file changes left. Discard All skips neither.

Changes apply one at a time, with one exception. A change that hits a merge conflict hands it to its thread to resolve, and the batch moves on meanwhile. The parked change lands when its resolution finishes. If a change applied meanwhile collides with that resolution, the parked change re-queues once for a fresh try.

While it runs, progress shows only in the Lucidos menu, on one line: "Applying 5 changes" and "2 of 5". Tapping unfolds "Change 2 of 5", a progress bar, the in-flight thread's activity (Merging, Resolving merge conflict, or Hardening) and title, and **Cancel**. The thread links to the event where the hardening or conflict began, or to the change. No notice pops up meanwhile except a member's failure. A summary notice reports the end.

A single *Apply* shows the same way, on its own line, and ends with an Applied or Failed notice. Each parked change also gets its own line, "Resolving merge conflict" and its title.

The unfolded line also shows times from the *apply estimate*: how long a hardening or conflict has run, how long one usually takes here, and roughly how long until all are applied. Its bar fills per finished thread and pulses on the ones in flight, parked ones included.

### Apply estimate
How long a hardening and a merge conflict resolution usually take in this workspace, from its own past applies. Recent applies count most, since the process gets faster. With fewer than five past runs there is no estimate, and the Lucidos menu shows only the time so far. It is a typical time, never a countdown: the slowest runs take two to three times as long.

Each pending change also carries a conflict prediction: whether merging it into `main` now would conflict. If so, its Changes panel row says "Likely merge conflict", and the batch estimate counts a resolution.

The engine emits `ApplyAllBatchStarted` with the full change-id list and the actor. It advances the batch as each member's `ChangeApplied` / `ChangeApplyFailed` lands, and parks a member on its `MergeConflictDetected`. Once all members resolve, it emits `ApplyAllBatchCompleted` with `applied: Vec<Uuid>` and `failed: Vec<ApplyFailure>`. Member status is first-write-wins, so one failure does not abandon the batch. Each member follows the same *hardening* and restart-derivation rules as a single *Apply*. Persisted under aggregate `apply_all_batch`, `aggregate_id` = `batch_id` (UUID).

While the batch runs, its Lucidos menu line offers **Cancel** (`POST /api/v1/changes/apply-all/cancel`). The engine stops advancing and interrupts the in-flight *hardening* or merge session and every parked resolution. It marks the remaining members `failed` with "Apply All canceled", so `ApplyAllBatchCompleted` still fires. Applied members stay applied; the rest return to pending (best-effort for a merge that already landed). A single *Apply* that woke a *hardening* or merge session can be canceled from its *coding-agent thread*'s Cancel button.

**The sweep is the prompt's form.** Ask Lucidos to apply everything as it settles, and it applies what is ready now. It also adds a *standing apply* to every *settling* thread whose change Lucidos applies, even one with no change yet. An *external-repo coding-agent thread* is passed over, having no change to apply. The batch's **Cancel** in the Lucidos menu takes back the whole sweep. The Changes panel's **Apply all on settle** arms only the changes it lists (*standing apply*).

### Standing apply
The owner's instruction to *Apply* a *change* once its thread finishes: pressed while the thread settles, carried out by the engine later (ADR 0168 clause 5). It replaces the Apply button on an unsettled thread, so nothing on either surface renders disabled.

Two forms. **Apply on settle** arms one change, from the thread's prompt row or its Changes panel row. **Apply all on settle**, atop the Changes panel's **Not finished** section, arms each *settling* change listed there and touches nothing in **Ready**. Both follow the same rule and are one-shot. The *sweep* is the prompt's form (*Apply All*).

It goes wherever *Apply* goes, and nowhere else. An *external-repo coding-agent thread* never gets one: Lucidos does not merge into that repo, and the thread proposes no *change*. So its prompt row draws no flag, the sweep passes it over, and the engine refuses any other arm.

Nor does a change whose *Apply* hit merge conflicts. Its thread works only to resolve that merge, and the resolver lands the change itself. Its Changes panel row reads **Applying...** with "Resolving merge conflicts", and its prompt row draws no flag.

It always ends. The change applies once the thread finishes and the agent has saved its last edits. A thread on an *event wait* keeps the instruction, since the wait ends by itself. A thread parked on a question, with a failed turn, or whose session stopped mid-settle never settles by itself. There the instruction is dropped and reported. It acts only on the change it was armed for, never a later one.

Cancel it from the control that armed it; every control is a toggle showing one shared state. A **Changes panel** row reads **✓ Applying on settle** once armed. The bulk control atop Not finished reads **✓ Applying all on settle** once all its changes are armed, and a press cancels them. On the **thread's own prompt row** it is a flag icon, filled once armed, with a matching tooltip.

Cancelling stops what has not started. A change already merging or hardening finishes, and nothing applied is undone. A running *Apply All* has its own Cancel, on its Lucidos menu line.

### Settling thread
A *coding-agent thread* that has not finished but will by itself: running, paused, or on an *event wait*. A *standing apply* waits through these, and through the moment after a turn while the agent saves its last edits. A thread parked on a question is not settling, since only you can end that. While it runs or watches an event, its *Apply* is withheld and the standing apply takes its place.

### Cancel (Stop)
The user-clicked **Stop** action on a working *coding-agent thread*. Like **Esc** in the *Claude Code* CLI, it *interrupts* the current turn but keeps the session resumable. The same `cc_session_id` and branch are kept, so the next message continues the *same* conversation (a `--resume`) with full context. It is NOT a kill and NOT a fresh start.

It emits `ResponseCanceled` (the visible "Canceled" chip) and `CodingAgentIdled` (the resume anchor). *Apply*, Discard and Archive are distinct: each ends the turn with its own lifecycle event. A stopped turn proposes nothing. Its work stays on the branch as *unproposed work* with reason `turn_incomplete`, and the *Not ready* strip offers **Continue** (ADR 0400). Stopped work does not block Archive: archiving sets it aside.

It routes through `interrupt_agent` (`POST /api/v1/claude-code/stop`, default `StopReason::UserStop`). A bounded fallback hard-stops only if the agent fails to honor the interrupt. Source: `crates/lucidos-engine/src/engine/claude_code/control.rs`, `agent_session/lifecycle.rs` (`idle_change_write`, `SessionEndAction`).

### Change
A *coding-agent*-proposed set of file edits shown as a pending branch in the UI. It is resolved by *Apply* or Discard, or kept for later with **Set aside**. Apply is a non-disruptive merge into main; an engine-affecting change then surfaces *New version available / Switch to new version*. Lifecycle events: `ChangeProposed`, `ChangeApplied`, `ChangeDiscarded`, `ChangeSetAside`, `ChangeBroughtBack`, `ChangeWithdrawn`. Stored as a row in the `changes` table. Internal (Lucidos-repo) coding-agent threads produce changes; *external-repo coding-agent threads* skip this flow.

The Changes panel lists pending changes in two sections. **Ready** comes first: changes whose thread has finished, with **Discard All** and **Apply All**. **Not finished** comes second: changes whose thread is still unsettled (*settling* or parked on a question), with **Apply all on settle**. Discard All never reaches Not finished. Below them sit **Set aside** and **Recently applied**.

A change's `status` is one of six values: `pending` (awaiting Apply or Discard), `set_aside` (a *set-aside change*), `applied`, `discarded`, `reverted` (applied, then undone), or `withdrawn` (a *withdrawn change*). No other value exists.

A user **Stop** proposes nothing: its work stays *unproposed work* on the branch (see *Change state*).

A change's file list tracks git. When later commits cancel the diff out (a commit plus its revert), the engine re-syncs the row to **zero files**. The card then reads "No file changes" rather than claiming edits its Diff can't show. Such a change stays pending, since the engine never resolves a change for the user. But *Apply* is refused, having nothing to merge; **Discard** resolves it. The re-sync runs when the coding agent next idles, when its session ends, and in an engine-startup sweep for stale rows.

### Set-aside change
A *change* you keep for later, out of the way. **Set aside** sits in the Apply button's menu, the thread's ⋯ menu and each Changes panel row. A set-aside change leaves the Review list, the attention badge and *Apply All*, and its thread can be archived. The branch stays, and the Changes panel lists the change under **Set aside**.

**Bring back** returns it to pending, from that list or its thread's banner; apply it from there, never directly. It also returns by itself when its thread's agent proposes new work on the same branch.

Lucidos sets work aside for you in one case: a thread archived with *unproposed work* on its branch. That work becomes a set-aside change instead of being lost. Its row reads " · Incomplete", since nobody saw its turn finish. **Bring back** on such a row makes it a *withdrawn change* and decides again: a new change if the last turn finished, otherwise unproposed work with its reason.

### Change state
What a *coding-agent thread*'s branch holds, as one of three values (ADR 0400):

| State | Means |
|---|---|
| `none` | No work on the branch. |
| `unproposed` | *Unproposed work*: commits on the branch that no pending *change* carries. |
| `proposed` | A pending *change*, which *Apply* takes. It also says whether applying it needs a restart. |

*Apply* acts only on `proposed`. Filter threads by it with `lucidos threads list --change-state`.

### Unproposed work
Work on a *coding-agent thread*'s branch that no pending *change* carries. Its reason says which turn end withheld it:

| Reason | Why |
|---|---|
| `plan_missing` | The branch has no implementation plan. |
| `plan_awaiting_approval` | The plan is not approved yet. |
| `outside_bound` | A security fix reaches past the files it named. |
| `turn_incomplete` | The turn did not finish: a Stop, a failure or an engine restart. |

No reason means no turn end withheld it. The thread is still running, waits on an event, works in an external repo, or its work was set aside. A turn end that withholds work emits `ProposalWithheld`. Unfinished work is never a proposal: Continue resumes the turn, and a turn that finishes proposes it.

### Not ready
The amber strip above the prompt box on a resting *coding-agent thread* that holds *unproposed work* with a reason. It shows where *Apply* would be, and says why there is none: "Not ready: needs a plan", "Not ready: plan awaits approval", "Not ready: outside its fix bound" or "Not ready: the turn did not finish". A short hint follows, and a tap shows the full reason. For an unfinished turn it carries **Continue**.

### Withdrawn change
A *change* that went back to *unproposed work* (`ChangeWithdrawn`). Its row keeps `status = withdrawn`, and the branch keeps every commit. Four things withdraw one:

- a turn that did not finish and moved the branch past a pending or set-aside change;
- a session that was running when Lucidos stopped, for its pending change;
- **Bring back** on an incomplete *set-aside change*;
- the first start after ADR 0400, for each change an older Stop proposed.

A withdrawn change never blocks Archive. The thread returns to the inbox, and its next proposal gets a new change.

### Incomplete change
Retired as a kind of pending *change* (ADR 0400). Work from a turn that did not finish is now *unproposed work* with reason `turn_incomplete`. The word survives only as the " · Incomplete" label on a *set-aside change* the archive filed.

### Change summary
One line saying what a multi-commit *change* does, written by a background model once the change is proposed. It heads the change card in the thread, the Applied / Discarded / Reverted toasts, and the Changes panel row. The card and the row unfold to the change's commits, oldest first. A one-commit change gets no summary, since its commit subject is the line. Until a summary lands, the oldest commit subject stands in, never the newest (usually a small fix). New commits clear the summary and a fresh one is written.

The model is set under **Settings → Models → Background tasks → Change summary**, and follows the title model until you set it.

### Claude Code
Anthropic's coding-agent CLI; the default *coding agent* Lucidos integrates (the other is *Codex*). Often abbreviated **CC**. Modeled in code as `CodingAgent::ClaudeCode` (enum, wire value `"claude-code"`). The thread channel value `"claude_code"` is historical and shared by every coding-agent thread: it means "coding-agent channel", not "runs Claude Code". The per-thread backend lives in the `coding_agent` column / event field.

### Codex
OpenAI's coding-agent CLI; the second *coding agent* Lucidos integrates. Modeled in code as `CodingAgent::Codex` (enum, wire value `"codex"`). Picked per thread via the coding-agent chip on the *compose destination* picker. The default is *Claude Code*, remembered per workspace via the `coding_agent_default` preference. The choice locks at the thread's first message, so a thread never switches backends.

Codex sessions run in an OS sandbox scoped to the thread's *worktree*, with two deliberate extras. The workspace's `data/` tree is writable, so `lucidos data write` works, and so is the worktree's shared git dir, so `git commit` works. Nothing else in the *workspace* is writable: not `.lucidos/`, not a sibling worktree.

User questions work as for Claude Code: Codex asks via the `ask_user_question` tool, and the answer renders as the usual question card. Under the default protocol, a permission card appears when a command or file change must escalate past the sandbox. The `exec` escape-hatch protocol runs non-interactively, with the sandbox as the only guard. The Apply / Discard flow, *changes*, and *hardening* work as for Claude Code. Lucidos classifies an escalated command first, and a plain read it recognises raises no card (see *coding-agent permission card*).

### Coding agent
Role: a subprocess driving a *thread* to make code changes inside an isolated git *worktree* (dev). Lucidos integrates two: *Claude Code* (default) and *Codex*. Modeled in code as `CodingAgent` (enum). The thread it drives is a *coding-agent thread*; the agent is chosen at the thread's first message and locked thereafter.

### Coding-agent branch
The git branch a *coding-agent thread* works on, named after the thread so `git branch -a` reads as a list of work: `lucidos-<coding-agent>-<app|repo>-<name>-<slug>-<id>`. Examples: `lucidos-claude-code-repo-lucidos-fix-auth-timeout-401a2d19`, `lucidos-claude-code-app-habit-tracker-add-streaks-401a2d19`, `lucidos-codex-repo-example-repo-fix-auth-401a2d19`. The `lucidos-` prefix marks it as Lucidos-made, which matters most in an *external repo* among your own branches. The short id at the end is the thread's own, as in its *worktree* folder name. It keeps threads started together apart, even when their prompts open alike.

The name is fixed when the branch is created. Renaming the thread does not move it, and a continued thread keeps its branch. *Apply* merges this branch into `main`. Older branches (`claude-code/…`) keep their names and keep working.

### Coding-agent permission card
The approval card a *coding-agent thread* shows when its agent wants something the engine won't wave through: Deny, Allow once, Allow for this thread, or Always allow. The thread waits until answered. Anything the agent writes **inside its own worktree** is allowed automatically, since the worktree is disposable and you review every diff before Apply. A card appears for a shell command the agent's own gate escalates, or a write **outside** the worktree. It also appears for a write into the worktree's hidden `.git` folder, the one in-worktree place the diff does not show.

**"Allow for this thread" is remembered for the life of that thread**, even across an Apply that restarts Lucidos. "Always allow" applies to every future thread, in an editable list under **Settings → Permissions**. A *trigger* fires unattended, so it never shows this card; see *side-effect grant*.

One kind of escalated command is answered for you. When Codex asks to step outside its sandbox, Lucidos reads the command first, and a plain read it recognises runs with no card. So a Codex thread watching a process no longer asks on every `ps`. Anything not plainly safe still asks, as does anything reaching for another user's rights.

**Working directories**, plural: "outside" above means outside these, not just the worktree. Two folders join it: this workspace's `data` folder (artifacts, apps, knowhow, triggers) and the OS temp dir. Reading or writing a file in either raises no card. A shell command still can, and four `cd` shapes always do. The `auto` *coding-agent permission mode* goes further: Claude Code's own classifier approves routine actions, so most of the rest never reaches you.

Precisely: a trigger-rooted thread is never ASKED. When the engine refuses one of its requests, it records the card already answered, so you can see afterwards what was refused and why. Nothing waits on you, and the thread is not flagged as needing attention.

### Coding-agent permission mode
Which of Claude Code's own permission modes its threads run in, set under **Settings → Coding Agents → Permissions**. **Accept edits**, the default, lets writes inside the *working directories* through and raises a *coding-agent permission card* for anything else. **Auto** hands those decisions to Claude Code's safety classifier instead. It covers shapes no allowlist can, such as a command that changes directory and redirects output in one line.

Auto has costs. It ignores a blanket `Bash` entry in your Claude Code permissions, so more commands reach the classifier, each paying a round-trip. An unreachable classifier denies rather than asking you, and a run of denials falls back to asking anyway. Codex has no equivalent setting. Changes apply to new sessions.

### Background task
Work Lucidos runs for a *thread* so it can outlive the agent's turn: a test suite, an e2e run, a long build. A *coding agent* starts one with `lucidos background-task run --description "<what it is>" -- <command>`; the *Lucidos Agent* with `run_bash_background`. The description names the task on the thread's waiting row. Lucidos runs it in the thread's worktree and arms an *event wait* on its completion, so the agent ends its turn instead of waiting. When it finishes, the thread re-opens with the exit status and the output's tail. Stopping your own task does not re-open the thread, and `lucidos hardened mark` stops any task the thread still has running, since the hardening supersedes it.

A command the agent backgrounds on its own dies when its turn ends, which is why this exists. Discarding or archiving the thread stops its running tasks.

### Coding-agent thread
A *thread* driven by a *coding agent* (Claude Code or Codex) inside an isolated git worktree. Marked by `is_coding_agent = true` on `thread_summaries`. A `SessionStarted` opening an agent session sets it, as does another event on the `claude_code` channel. A *resume boundary* alone never does: `ContinuationStarted` fires on chat and trigger threads too.

The persisted `source` is `"claude_code"` for every coding-agent thread (the historical, backend-agnostic channel name). Public source filters use `coding-agent`, accepting `claude_code` only as a legacy alias. The `coding_agent` column names the product (`'claude-code' | 'codex'`, NULL = legacy Claude Code row). The `coding_agent_kind` column names the worktree flavor (`'lucidos' | 'app' | 'external'`). It emits `CodingAgent*` events instead of chat `Response*` events. Three flavors:

- **Lucidos-internal coding-agent thread**: works on the Lucidos workspace repo itself. Produces *changes* surfaced via the Apply / Discard UI on completion.
- **App coding-agent thread**: works on one app folder (`data/apps/<id>/`) via a sparse-checkout *worktree* of the workspace git. Produces *changes* with the same Apply / Discard UI; Apply does **not** restart the engine, and Lucidos's `/harden` does **not** run.
- **External-repo coding-agent thread**: works on a user-registered external git *repository*. It uses a different worktree-creation path and a minimal system prompt, and **skips** the Lucidos change-proposal flow on session end.

See also: `system-knowhow/coding-agent-events.md`.

### Side question
A quick question put to a thread from *side-question mode*: type it, then press the round Ask button or Enter. A typed `/btw` is ordinary text. It may carry images.

The thread's agent answers from the thread's full context, beside any running turn, with no tools. A Claude Code thread asks a copy of its session; a Lucidos Agent thread asks its own model once. The answer shows on a card at the moment it was asked, with later output below it.

It is kept as events **no agent ever sees** (ADR 0320), so the card survives reloads and shows on every device. A tap on its head folds it to one line; a second tap unfolds it. Codex threads refuse side questions.
See also: `system-knowhow/coding-agent-events.md` § Side questions are recorded, and hidden from every agent.

### Side-question mode
The composer state where the round Ask button asks the box as a *side question* instead of sending a message. A hold on the composer row's end button turns it on: Send, Stop, Submit, or a waiting card's Cancel. So does ⌥↵ (Alt+Enter), in any thread state, and any draft stays in the box.

A "Side question" pill above the box shows the mode. The pill's ×, Escape, a second ⌥↵, or asking turns it off. It wins over a waiting card, so the × is the way back to answering. It is kept with the draft, so it survives a reload and shows on every device.

### External-repo coding-agent thread
A *coding-agent thread* running against a user-registered external git *repository*, not the Lucidos workspace. It has no Apply / Discard surface: the user reviews diffs in the external-repo diff viewer. Its worktree creation and system prompt differ from the Lucidos-internal variant, as `docs/plans/2026-03-17-external-repos-plan.md` documents.

### App coding-agent thread
A *coding-agent thread* whose isolated *worktree* sparse-checks out the workspace git on one `data/apps/<id>/` folder. Same machinery as a Lucidos-internal one (worktree, branch, *change*, *Apply* ff-merge), but on the workspace git, not the Lucidos source repo. *Apply* restarts no engine, and Lucidos's `/harden` does not run (apps own their hardening). If *Apply* changed an iframe-bundled file, the engine emits a transient `AppUiRefreshRequested { app_id }` so open iframes reload. The *WIP app preview* shows the in-flight app from the worktree while the thread is open.

Branch name shape: `lucidos-<coding-agent>-app-<app_id>-<slug>-<id>`, e.g. `lucidos-claude-code-app-habit-tracker-add-streaks-401a2d19` (ADR 0041, ADR 0076).
See also: `docs/plans/2026-05-27-app-coding-agent-threads-design.md`.

### WIP app preview
The in-progress rendering of an *app* served from an open *app coding-agent thread*'s *worktree* instead of the workspace's main copy. Add `?thread_id=<id>` to the app UI URL. The panel-overlay slot then swaps from the live app (`<workspace>/data/apps/<id>/`) to the WIP (`<worktree>/data/apps/<id>/`). The WIP iframe loads HTML/CSS/JS from the worktree, but its SDK calls (`lucidos.data.*`, `lucidos.events.*`) still hit the live workspace. So data-coupled UI edits show their full effect only after *Apply*. It reverts to live when the user leaves the thread or *Apply* removes the worktree.

### Hardening
The quality gate every *coding-agent thread* must run via `/harden` before handing back to the user. It reviews the diff against project rules, runs the relevant test suites (Rust + TS + e2e, skipping irrelevant layers), and checks system-knowhow drift. If the hardening marker is missing when the user clicks *Apply*, Apply runs `/harden` synchronously and the user waits.
See also: `.claude/commands/harden.md`, the playbook the agent runs. The engine system prompt owns the requirement to run it and states it to every session.

### Repository
A user-registered external git repository (a row in the `repositories` table) that an *external-repo coding-agent thread* can run against. Distinct from `data/imported/` *imported* repos, flattened to plain files as *artifacts*.

## When to add a term

Add a term here when a new concept appears in the user-facing surface: UI strings, chat prose, app/trigger intents, knowhow frontmatter, or `system-knowhow/*.md` content. A dev-internal concept (engine plumbing, DB schema, event-bus mechanics) goes in `docs/glossary.md` instead. Coding-agent-only concepts go in the **Advanced (coding agents)** section.

## When a term changes

If a term is renamed, retired, or shifts meaning, update this file in the same commit. Per `.claude/rules/system-knowhow.md`, every `system-knowhow/*.md` file using the term is updated alongside. `/harden` treats drift between code/UI and the glossary as a hardening failure.
