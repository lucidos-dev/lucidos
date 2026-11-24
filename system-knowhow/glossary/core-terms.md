---
name: Lucidos Glossary, Core terms
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

<!-- domain-event: BuildSlotAcquired: The CLI emits this domain event when a contended host build acquires a slot. -->
<!-- domain-event: BuildSlotWaiting: The CLI emits this domain event when a host build must wait for a slot. -->
<!-- domain-event: BuildSlotReleased: The CLI emits this domain event when a host build slot is released. -->

One part of `system-knowhow/glossary`, which lists every part.

## Core terms

### Active device
A device currently reporting itself visible to the engine. On desktop: `document.visibilityState === 'visible'` AND `document.hasFocus()`. On iOS PWA standalone: `visibilityState === 'visible'` only (Safari leaves `hasFocus()` false even in the foreground).

The Tauri desktop client also requires the *native window* to be active (focused AND on-screen). WKWebView can't see macOS `orderOut:`, so a trayed window keeps `visibilityState='visible'` / `hasFocus()=true`. The AppKit state reaches the page via a `native-window-active` event instead.

When an active device has the notification in front of the reader, no OS push fires anywhere. It acts on the *PresenceCheck* (toast, auto-read, or the badge with toasts off) and reports that *presence outcome*, and only that report suppresses the push. The protocol decides this per notification, not a stale heartbeat window.

A device with an app in *fullscreen* (native or pseudo) is NOT active, since only the Lucidos shell can show a toast. It then matches an app in its own window, which always took the push.

See also: `system-knowhow/notifications/1-conceptual-model.md` §1, `system-knowhow/notifications/3-the-presencecheck-protocol.md` §3.

### Active (thread state)
A *thread* currently doing work (the system's turn), shown by the row's status icon, **not** by a separate section. It stays in the *Current section* and shows or drops the Active indicator in place. Contrast *attention* (the user's turn: a pending *change*, an awaiting answer, or a failure), shown by the *Blocked* and *Review* groups of the *Ongoing grouping*.

As a **query filter**, "active" is the **union** of the `running` and `waiting_for_user_answer` statuses. It is `--active` on `lucidos threads list` / `count`, and `active` on `GET /api/v1/threads/{list,count}`, the `threads` tool and `lucidos.threads`. That union answers "is anything busy?" wrongly, since a thread parked on a question waits on the human. Ask that with the *status filter* instead.

### Affected trigger
A *trigger* a *plugin* does not own that relies on a file the plugin's update or uninstall removes. Usually one an agent made during the plugin's setup. Lucidos only warns: in the plugin panel, on its receipt, in one notification, and in each later fire. It never edits, pauses or deletes the trigger. The warning ends when the user edits the trigger or the file returns. See `system-knowhow/plugins/three-distribution-shapes.md` § "Affected triggers".

### Agent archive
An agent archiving a *thread* itself, through the `threads` tool's `archive` action or `lucidos threads archive`. It reaches only its own thread and its own direct *child threads*. It runs the same *cascading archive* as the Archive button, so the same states refuse it. A *pinned thread* is the one difference: an agent is refused one (`thread_pinned`), and a pinned sub-thread stays open while the rest goes (ADR 0312). A child is archived at once.

An agent's own thread is archived once its turn ends and it has settled, and a new message before then keeps it open. Lucidos never archives a thread by itself, not even after its change is applied (ADR 0310).

*Thread triage* is the one wider path: under a triage the user replied to, an agent may archive any inbox thread that needs nothing from the user (ADR 0349).

### Agent browser
The Chrome the Lucidos Agent drives through its `browser_*` tools, headless or in a visible window, with its own profile in the workspace. Lucidos uses the Chrome installed on the machine and bundles none.

**It always runs inside Chrome's sandbox**, so a page that exploits Chrome stays confined. There is no setting to turn the sandbox off. A machine where Chrome cannot start one gets no agent browser and an error naming the fix: run Lucidos as a non-root user, or give Chrome a usable sandbox (ADR 0457).

### Agent sandbox
The OS sandbox every agent-steered process runs in: the Lucidos Agent's commands and scripts, triggers, coding agents, background tasks and MCP servers. It hides Lucidos's own secrets and keeps each workspace's permission lists read-only. Everything else stays as reachable as before, except that a trigger's subprocesses can write only inside its write scope. macOS uses a seatbelt profile, Linux uses Landlock. For a trigger, Linux also mounts everything outside the scope read-only, where the kernel allows unprivileged user namespaces.

Where the machine cannot apply it, those processes do not start.
The owner gets one notification, and a banner names the reason.
Turning **Sandbox agents** off (`agents_unsandboxed=true`, Settings → Permissions) lets them start without it in this workspace.
The banner then warns instead (ADR 0645).

### Private path
An absolute file or folder path outside workspaces and registered repositories that the owner adds in Settings → Permissions to keep agents from opening it. The machine-wide list applies across workspaces on the Lucidos home. Each workspace can add paths. The section reports the local sandbox's enforcement level, and running agent processes pick up changes when they next start.

### Archive all
The **Archive all** action on the drawer's *Current section* header, under the *Folders grouping*. It archives only what is safe, so a thread that needs the user stays, and so does a running one. Needing the user means an unanswered question, a pending *change*, unproposed branch work, an unsent *draft* or a failed run, here or in a sub-thread. A *pinned thread* sits in Pinned and is never touched, and neither is any thread of its family, even below an archived one.

A confirm counts what goes and what stays, and the engine archives only the threads it listed, if they are still safe. Each kept family is counted with its reason, and its other threads beside it. Every count is threads not archived, sub-threads included, as the section badge counts them, so the two totals add up to the Current badge. The result toast offers **Undo**, which *unarchives* that batch (`ThreadUnarchived`). It is the owner's button only: an agent uses *thread triage* instead, through the same classifier (ADR 0349).

A thread in the Archive section comes back through **Move to Current**, in its thread menu. It unarchives the thread and the sub-threads under it, as Archive took them. A thread stored archived but still shown in Current offers Archive instead.

### Action blocker
Why Archive or Delete cannot run on a thread, as one reason. The reasons are the home thread, a running turn, a question waiting for the user, a pending *change*, and a *held idle proposal*. Each of the last four also counts when a *sub-thread* holds it. When several hold, the strongest is named.

The thread menu shows a blocked Archive or Delete dimmed with that reason, never hidden. When sub-threads hold it back, the menu lists each one with its state, strongest first, and a tap opens it. A refusal toast offers **Show sub-thread**, which opens the first. The user resolves the blocker: answer, Stop, Apply, Discard, or Stop waiting. Nothing in the menu cancels it. The engine's refusal names the same blocker (`thread_lifecycle::action_blocker`, ADR 0378).

<!--gloss-app-start-->
### App
A user-installed mini-application with its own UI (HTML/CSS/JS) at `data/apps/<id>/`, plus optional *knowhow* / *intents* / *scripts* / *triggers*. Chat is not per-app: every conversation is a regular *chat thread*. While an app is open in the panel-overlay slot, its `manifest.json` and discovered context flow into the *Lucidos Agent*'s prompt. Quick edits use the agent's file tools; heavier edits spawn an *app coding-agent thread*. The *app manifest* is user-facing metadata; `knowhow/` and `intents/` are engine-facing context loaded while the app is active. Its interactive surface is the *app UI*.

An app folder has a kind: an ordinary app, listed in the apps panel, or a *widget*, shown in a thread and never listed.
See also: `system-knowhow/building-an-app.md`, `docs/taxonomy.md` § Apps.
<!--gloss-app-end-->

### App icon
The picture that stands for an *app* or *widget* across Lucidos (apps panel, menu drawer, a dashboard's Add, search). An image file inside the app folder, named by the *app manifest*'s `icon` (`assets/icon.svg`). Without a valid one, a *monogram tile* stands in. Distinct from the *app-icon badge*, which counts on Lucidos's own home-screen or dock icon.
See also: `system-knowhow/building-an-app/scaffolding-defaults.md` § App icon.

### App-icon badge
The unread-notification *count* painted on the installed app's **icon**: a PWA's home-screen icon via the web Badging API, or the Tauri macOS **dock** tile. The macOS client also shows the count in its **menu-bar tray-icon title**, always, even when menu-bar-only. Distinct from the *in-app surface*'s bell badge inside the Lucidos UI.

The install origin decides how many workspaces it counts. Two installs show the total across running workspaces: the **Tauri desktop app**, and a PWA from the **gateway**. The gateway serves the picker at `/~/` and each workspace at `/<slug>/`, both with an origin-wide manifest. One icon covers them all, so one workspace's count would hide the rest. A PWA from a **direct engine** origin shows that workspace's count only. The gateway follows each running engine's row stream and re-reads that count on a change, so a stopped workspace contributes nothing.

The icon and the bell must never disagree *for the workspace on screen*. The OS also writes the icon from a push payload's `app_badge`, so the page **re-asserts** the count rather than writing only on change. It takes this workspace's share from the same live unread set the bell projects. While the app is closed, the icon can lag a read made elsewhere until the next push or open. See `system-knowhow/notifications/4-the-in-app-surface-page-local-3.md` § App-icon badge.

The **Lucidos mark**'s count badge totals the notification rows atop the **Lucidos menu**, including reachable paired remote gateways. Each row opens its workspace's notifications view. The OS icon counts only its own origin, so the mark can show a larger total. The mark's bottom-right badge carries this count onto the thread pane and the threads drawer, where the bell never appears.

### App lint
The engine's check on an HTML, CSS or JS file the Lucidos Agent writes under `data/apps/<id>/`, with `create_app`, `write_file`, `edit_file` or `run_python`. It finds three mistakes: a `:hover` outside `@media (hover: hover)`, `.action-btn` variants swapped to show a choice, and a variant without its base class. Each finding comes back in the tool result as an `[APP LINT]` note with the line and the fix. It never refuses or changes the write.
See also: `system-knowhow/building-an-app/scaffolding-defaults.md` § Scaffolding defaults.

### App manifest
The metadata file for an app at `data/apps/<id>/manifest.json`. Holds name, description, `icon` (the path to its *app icon*) and `reveal` (see *ready signal*): what the UI shows. A *widget*'s manifest also holds `kind`, `reusable`, and its owner: `origin_thread_id`, or `origin_plugin_id` for a widget a *plugin* ships. **Not** loaded into the LLM context; operational knowledge belongs in `knowhow/`.

### App storage
Per-app, per-device key-value state an app keeps through `lucidos.storage.local` and `.session`, which stand in for `localStorage` and `sessionStorage` inside an app. The values live in the Lucidos shell's own browser storage, so they stay on one device. Each app sees only its own keys. An app awaits `lucidos.storage.ready` before its first read. State every device should share belongs in `lucidos.data` instead.
See also: `system-knowhow/js-sdk/lucidos-preferences-user-settings.md` § `lucidos.storage`.

### App UI
The iframe that renders an app's HTML/CSS/JS (from `data/apps/<id>/ui/`) in Lucidos's panel-overlay slot. Distinct from the *app*, the whole installed unit. "Open the app" means open its UI inline and make its chat the active conversation. "Refresh the app UI" means reload the iframe without changing chat context. The `navigate_ui` tool's `app` target and the `AppUiRefreshRequested` event both mean this iframe.

### API caller
An external HTTP caller of `/api/v1/...` that did NOT identify as a known actor (`You`, `Lucidos Agent`, `Lucidos Engine`, `System`). It sends no `x-lucidos-device-id` (no browser session) and no `x-lucidos-agent-origin-token` (not a Lucidos-spawned subprocess). UI actor-chip label: "API caller", with a plug; the origin popover shows the User-Agent. The reserved label stops an anonymous mutating POST from impersonating the user as "You".

The venv agent-origin shim (`crates/lucidos-engine/src/runtime/python.rs`) auto-forwards the agent-origin token on Python calls to the engine port. So a `run_python` subprocess that hand-rolls `urllib.request` lands as `Lucidos Agent`, not here. The shim is a `.pth`-loaded `_lucidos_agent_origin` module that survives a host `sitecustomize.py` such as Homebrew's.

<!--gloss-artifact-start-->
### Artifact
A user-owned file under `data/artifacts/`. Git-tracked, never auto-deleted. Includes notes, imported API data, project folders, screenshots, generated images. The durable counterpart to ephemeral runtime state under `.lucidos/`.
See also: `system-knowhow/best-practices/workspace-layout.md` § `artifacts/`.
<!--gloss-artifact-end-->

### Auth module
A WASM signer (plus optional `<name>.manifest.json` *signer manifest*) installed under `data/auth-modules/` to sign outbound proxy requests. Plugins can ship auth modules in their `auth-modules/` directory; the install-time LLM walks the user through wiring the matching `apis.json` snippet. Engine-side mechanics (host imports, capabilities, body modes) live under *signer* in `docs/glossary.md`.
See also: `system-knowhow/building-an-auth-handshake.md`.

### Blocking descendant
A *sub-thread* whose state currently prevents its ancestor from being cascade-archived: Running, paused on a user question (WaitingForUserAnswer), or a *coding-agent thread* with pending *changes*. A *held idle proposal* also blocks the cascade, but this count leaves it out. The menu then offers Archive, and the engine's refusal names the sub-thread. Counted in `blocking_descendant_count` on the thread aggregate; while it is non-zero, the thread menu shows Archive blocked with the *action blocker*'s reason.

### Attention-needing descendant
A *sub-thread* that needs user action to progress: paused on a user question (WaitingForUserAnswer), or a *coding-agent thread* with pending *changes*. A strict subset of *blocking descendants* that drops `Running`, since running work is delegated. Counted in `attention_descendant_count` on the thread aggregate. It bubbles the ancestor chain to the **Current** section, so the user sees the attention card while siblings still run.

### Canvas
The right-hand side of the side-by-side desktop layout, where the live system appears and you act on it: an open *app*'s *app UI*, a file or *artifact* preview, a *change*'s diff, settings, a URL. You direct the *Lucidos Agent* in the *Conversation*, and its output appears, live and usable, in the Canvas. Apps there read real workspace data through the SDK, so changes made from the Conversation show at once (see *Live co-creation*). On mobile it is one swipe away rather than alongside.

Canvas names a **side**, not a pane; the single pane filling it is the *content pane*. Say "Canvas" for the side or the Conversation↔Canvas back-and-forth, and "content pane" for the pane itself (where a view lands, a shortcut, a resize).
See also: *Conversation*, *content pane*, *Live co-creation*.

### Build slot
One of N permits to run a heavy build on the host. A slot is an OS file lock in a machine-wide pool under `$HOME/.lucidos/build-slots/`. The kernel releases the lock when its holder dies. N resolves from `LUCIDOS_MAX_CONCURRENT_BUILDS`, then a capacity file beside the pool (`lucidos build-slot --set-capacity <n>`), then host RAM.

Over the limit a build waits rather than failing: `lucidos build-slot -- <command>` blocks until a slot frees, then runs the command as its child. It is **not** a queue: whoever samples a freed slot first takes it. Taken by `make lint`, `make test`, the e2e build phase, and the engine's own rebuilds; frontend commands are too cheap to gate. Without a `lucidos` binary on PATH the build just runs. Every release is announced as `BuildSlotReleased`, so a session that gave up on `--max-wait` and subscribed is always woken. `BuildSlotWaiting` and `BuildSlotAcquired` fire only under contention.

A slot also sets the build's CPU share. The holder runs at `nice +10`, inherited by the compile tree, with `CARGO_BUILD_JOBS` set to cores divided by slots held. The share never falls below `cores / capacity`, so a solo build keeps the machine and full contention divides it evenly. A caller's explicit `CARGO_BUILD_JOBS` wins, and `LUCIDOS_BUILD_SLOT_NICE` overrides the increment (`0` opts out).

The engine's own Apply rebuild waits as a **priority waiter**: ordinary builds leave a freed slot to it, and it runs un-niced. It never exceeds the count and never stops a running build. While it waits, the Lucidos menu calls it a **queued build** ("New version queued", not "Building new version"), with a still hourglass for the spinner.

A source launch build takes a **build-slot lease** before compiling and keeps
it through preflight, publish, codegen and the after hook. Its slot stays held
between steps, so a competing build cannot take it mid-build. A broker helper
releases the slot when the build finishes or its shell dies. A broker that
predates leases leaves the individual steps to acquire slots.

It exists because each *coding-agent thread* has a *worktree* with its own `target/`, so parallel sessions mean N full compiles at once. Distinct from the engine build lock, which serialises engine builds inside one checkout's shared `target/`, and from the e2e lock, which hard-fails a second run.
See also: ADR 0070, ADR 0210, ADR 0304, `system-knowhow/lucidos-cli/subcommands-6.md` § `lucidos build-slot`.

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

