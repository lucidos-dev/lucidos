---
name: Lucidos Glossary
description: Canonical user-facing terms used across Lucidos. Core terms (app, artifact, intent, knowhow, plugin, script, thread, trigger, workspace, event, …) plus advanced coding-agent terms (Apply, change, Claude Code, coding agent, coding-agent thread, external-repo coding-agent thread, hardening). The single place these words are defined; every other system-knowhow file, the engine system prompt, and the UI use these definitions verbatim. Load when the user (or the LLM) needs to disambiguate one of these words, when the same concept seems to have two names, or before writing user-facing prose that mentions any of them. Dev-only terms (actor, ActorMode, aggregate, BusEvent, EventBus, ThreadEvent, projection, signer, worktree, …) live in `docs/glossary.md`, which extends this file.
---

# Lucidos Glossary

Canonical definitions for the words Lucidos uses with the user. One term, one place, one definition. When prose in another file (`system-knowhow/*.md`, the engine system prompt, app/trigger intents, UI strings) names a term below, it uses *this* meaning — never a synonym.

If you find yourself reaching for a near-synonym (*child thread* for the transitive descendant concept *sub-thread*, *task* for *intent*, *recipe* for *knowhow*, *attachment* for *artifact*), use the canonical word from this file instead. If a needed concept genuinely isn't here, add it in the same change that introduces it.

Terms are split into two sections: **Core** terms most users encounter, and **Advanced — coding agents**, the surface only users running coding-agent workflows hit (everything around Claude Code, worktree-based code changes, the Apply / Discard flow, hardening).

This file is the **base layer**. `docs/glossary.md` extends it with dev-only internal terms (aggregate, actor, ActorMode, EventBus, BusEvent, ThreadEvent, projection, worktree, …) that the workspace LLM doesn't need.

## Core terms

### Active device
A device currently reporting itself visible to the engine. On desktop: `document.visibilityState === 'visible'` AND `document.hasFocus()` (the tab is in the foreground stack AND the browser window has OS focus). On iOS PWA standalone: `visibilityState === 'visible'` only (Safari leaves `hasFocus()` false even when the PWA is fully foregrounded). When at least one device is active at notification time, no OS push fires anywhere — the active device gets the *in-app surface* via the `NotificationCreated` SSE channel instead. Determined per-notification by the PresenceCheck protocol, not by a stale heartbeat window.
See also: `system-knowhow/notifications.md` §§1, 3.

### Active (thread state)
A *thread* that is currently doing work — the system's turn — surfaced by the row's status icon, **not** by a separate section. A thread no longer changes section when it starts or stops running: it stays in the *Current section* and just shows (or drops) the Active indicator in place. Contrast the *Current section* (where it lives) and *attention* (the user's turn — a pending *change*, an awaiting answer, or a failure — surfaced by a count badge and the attention filter rather than by reordering Current).

### App
A user-installed mini-application with its own UI (HTML/CSS/JS) at `data/apps/<id>/`, plus optional *knowhow* / *intents* / *scripts* / *triggers*. Chat is not per-app: every conversation is a regular *chat thread*. When an app is open in the panel-overlay slot, its `manifest.json` and discovered context flow into the *Lucidos Agent*'s prompt, so the agent can answer in-context. Quick edits to an app happen through the agent's file tools on the chat path; heavier edits spawn an *app coding-agent thread*. The *app manifest* is user-facing metadata shown in the UI; `knowhow/` and `intents/` are engine-facing context loaded into the LLM when the app is active. The app's interactive surface is the *app UI* — see entry.
See also: `system-knowhow/building-an-app.md`, `docs/taxonomy.md` § Apps.

### App manifest
The metadata file for an app at `data/apps/<id>/manifest.json`. Holds name, description, icon — what the UI shows. **Not** loaded into the LLM context; operational knowledge belongs in `knowhow/`, not the manifest.

### App UI
The iframe that renders an app's HTML/CSS/JS (from `data/apps/<id>/ui/`) inside Lucidos's panel-overlay slot. Distinct from the *app* (the whole installed unit — UI plus scoped chat, knowhow, intents, scripts, triggers): "open the app" means open its UI inline and make its chat the active conversation; "refresh the app UI" means reload the iframe without changing chat context. The `navigate_ui` tool's `app-ui` target and the `AppUiRefreshRequested` event name both refer to this iframe surface specifically.

### App Store
The **Store** tab of the *Apps* section, for discovering and installing *plugins* from registered *marketplaces*. (It was previously a separate top-level panel; it was folded into Apps alongside the **Installed** tab — the same view that lists the workspace's installed *apps*.) The Store lists marketplace plugins; each card's primary button progresses **Install** (or **Update**) → **Setup** → **Open**, with an **Uninstall** button once the plugin is on disk. "Setup" appears while the plugin's *setup thread* is still running; "Open" launches the plugin's app once setup is done (or there was none). New installs/uninstalls route through the standard plugin confirmation panels. The catalog re-scans whenever the Store tab is opened. The engine no longer silently updates installed marketplace plugins — when it finds a newer version it notifies the user, who applies the update from the Store card or the installed app's **Update** button (see *Marketplace*). Marketplaces are added/removed under Settings → Marketplaces. When no marketplaces are registered yet, both the Store empty state and the Settings → Marketplaces empty state offer a one-click **Add the official Lucidos marketplace** button (registering `github.com/lucidos-dev/plugins`), alongside the link to register your own.

### API caller
An external HTTP caller of the engine's `/api/v1/...` surface that did NOT self-identify as one of the known actors (`You`, `Lucidos Agent`, `Lucidos Engine`, `System`). Reaches the engine without an `x-lucidos-device-id` (no browser session) and without an `x-lucidos-agent-origin-token` (not a Lucidos-spawned subprocess). UI actor-chip label: "API caller" with the 🔌 icon; the origin popover discloses the User-Agent string for forensics. Reserved label so an anonymous mutating POST can never impersonate the user as "You". A Lucidos-spawned `run_python` / `run_bash` subprocess that hand-rolls `urllib.request` / `curl` instead of using the `lucidos` CLI used to fall in here — the venv agent-origin shim (`crates/lucidos-engine/src/runtime/python.rs`), a `.pth`-loaded `_lucidos_agent_origin` module that survives a host `sitecustomize.py` such as Homebrew's, now auto-forwards the agent-origin token on Python calls to the engine port so those land as `Lucidos Agent` instead.

### Artifact
A user-owned file under `data/artifacts/`. Git-tracked, never auto-deleted. Includes notes, imported API data, project folders, screenshots, generated images. The durable counterpart to ephemeral runtime state under `.lucidos/`.
See also: `system-knowhow/best-practices.md` § `artifacts/`.

### Auth module
A WASM signer (plus optional `<name>.manifest.json` *signer manifest*) installed under `data/auth-modules/` to sign outbound proxy requests. Plugins can ship auth modules in their `auth-modules/` directory; the install-time LLM walks the user through wiring the matching `apis.json` snippet. Engine-side mechanics (host imports, capabilities, body modes) live under *signer* in `docs/glossary.md`.
See also: `system-knowhow/building-an-auth-handshake.md`.

### Blocking descendant
A *sub-thread* whose state currently prevents its ancestor from being cascade-archived: Running, paused on a user question (WaitingForUserAnswer), or a *coding-agent thread* with pending *changes*. Counted in `blocking_descendant_count` on the thread aggregate; surfaced in the frontend to hide the Archive button when non-zero.

### Attention-needing descendant
A *sub-thread* whose state requires user action to progress — paused on a user question (WaitingForUserAnswer), or a *coding-agent thread* with pending *changes*. Strict subset of *blocking descendants* — drops the `Running` case (running work is delegated, not pending attention). Counted in `attention_descendant_count` on the thread aggregate; bubbles the ancestor chain to the **Current** section so the user notices the attention card even when sibling descendants are still running.

### Canvas
The right-hand side of the side-by-side desktop layout — the **content pane** — where the live system materializes and you act on it: an open *app*'s *app UI*, a file or *artifact* preview, a *change*'s diff, settings, a URL. The counterpart to the *Conversation*: you direct the *Lucidos Agent* in the Conversation, and its output appears — live and usable — in the Canvas. Not a static preview — apps in the Canvas read real workspace data through the SDK, so reshaping things from the Conversation updates the Canvas immediately (see *Live co-creation*). The same surface on every device: on desktop it sits beside the *Conversation* (the side-by-side layout); on mobile it's one swipe away rather than shown alongside.

### Capacity policy
The configurable caps governing the *Thread Queue*: `max_concurrent_total` (the shared ceiling for ALL threads — background spawns and user-initiated work alike), per-kind caps (event trigger / scheduled / sub-thread / coding agent — background only), per-*trigger* concurrency, a per-trigger queue ceiling with a choice of overflow behavior (drop the oldest waiting fire + notify, or pause the trigger + notify), and `reserved_background` — slots background can reclaim ahead of user-initiated work so user priority can't starve triggers/cron (0 = pure user priority). Concurrency caps of 0 mean "hold" — admission pauses and the queue accumulates until the cap is raised. Edited in the Thread Queue panel (`PUT /api/v1/thread-queue/policy`); stored event-sourced — the latest `CapacityPolicyChanged` event IS the policy. User-initiated work is prioritized but still subject to the ceiling (ADR 0008).

### Cascading archive
Archiving a parent *thread* also archives every *sub-thread* under it, in one atomic operation. Disabled when any descendant is a *blocking descendant*.

### Chat thread
A *thread* whose `source = 'chat'` — the user typed the opening message in the Lucidos chat UI. Answered by the *Lucidos Agent*. Contrast with *trigger thread* and *coding-agent thread*.

### Child thread
A direct descendant *thread* created by a `relation: "child"` spawn (`run_thread` / `run_coding_agent` / `lucidos spawn-thread --relation child`). The engine wires a callback so the *parent thread* resumes with the child's result when it terminates. Identifiers: DB column `parent_thread_id` on the child row points up to the parent; Rust struct field and event payload field `child_thread_id` (on the `Callback` struct and the `ChildThreadCompleted` event) name the child. A child is a *sub-thread*; the reverse isn't true.

### Command guard
An opt-in safety gate over the *Lucidos Agent*'s shell/Python tools. Toggled under **Settings → Permissions → Command Safety** (off by default). When enabled, a command that's clearly catastrophic (`rm -rf /`, a fork bomb, formatting a disk) is refused without running; a command that looks like an irreversible real-world side-effect (sending mail, a mutating HTTP request, a cloud-CLI change, spending money) or destruction outside the workspace pauses and shows the user a *command permission card* to approve; an in-workspace deletion/overwrite (recoverable) runs after a *command checkpoint* is taken, leaving a one-click Undo. A fast static check settles the obvious cases and a cheap LLM **judge** decides the ambiguous middle, erring toward asking. Under the master toggle, two sub-settings (only active while the guard is on): an **LLM judge** on/off switch (off falls back to a static classifier: the dangerous-command list plus a destruction scan — out-of-workspace deletes/overwrites still ask, in-workspace ones still checkpoint) and the **judge model** (defaults to Haiku). Commands the user has chosen to always allow are kept in an editable list under **Settings → Permissions → Lucidos Agent permissions** (`~/.lucidos/agent-allowed-commands`). A *trigger* fires unattended, so it can't be asked — it instead runs irreversible commands only within its declared *side-effect grant*; an ungranted one is blocked and fails the run. The safe majority of commands — including reads anywhere and writes inside the workspace — run untouched. See `system-knowhow/running-python.md` § The command guard.

### Command checkpoint
A snapshot of the workspace's tracked content the *command guard* takes right before running an in-workspace destructive command (a delete or overwrite under the workspace) — the recoverable, "reversible" lane. Instead of asking, the guard saves the current state, runs the command, and shows a one-click **Undo** on the command's card; Undo restores the workspace to the snapshot (re-creating deleted files, reverting overwritten ones). Undo does not remove files the command newly *created* — it targets destruction. Out-of-workspace destruction can't be checkpointed, so it goes to the *command permission card* (ask) lane instead. Only taken when the *command guard* is on.

### Command permission card
The approval card the *command guard* shows when a shell/Python command needs the user's go-ahead. Same UI as the coding agent's permission card: Deny, Allow once, Allow for this thread, or Always allow (remembered for similar commands). Until answered, the thread waits on the user; answering lets the command run or refuses it.

### Compose destination
The compose view's single "who/where" pick for a new *thread*, shown as a single destination picker: either the *Lucidos Agent* (default — a *chat thread*) or a coding target — the Lucidos source, an *app*, or a registered *repository* — which spawns a *coding-agent thread* of the matching flavor. A one-line caption under the picker states the consequence of the current pick (the Lucidos Agent can hand off to a *coding agent*; a coding target produces a reviewable *change*, except *external-repo coding-agent threads* which review the diff from the thread). The Claude Code vs Codex pick is a separate coding-agent chip shown only for coding targets, remembered per workspace (`coding_agent_default` preference) and locked at the thread's first message. Replaces the former mode toggle + scope chain. The Lucidos source target is offered only on a dev build (a source checkout exists); a packaged install has no source tree to edit, so the picker hides it there (gated on the `/health` `packaged` flag).

### Config
Workspace configuration files under `data/config/`, principally `apis.json` (proxy entries, signer wiring, OAuth flows). Users edit these directly or via the engine's auth-handshake flow.
See also: `system-knowhow/building-an-auth-handshake.md`.

### Connected-but-hidden
A device whose Lucidos page is alive (SSE EventSource still streaming) but not currently *active*: a different browser tab is selected, the window is behind another app, or the iOS PWA is in the app switcher. Receives the `NotificationCreated` SSE message and updates its bell badge silently, but does NOT show a toast (the user can't see it). Eligible for an *OS surface* push (subject to global suppression in §2 of `system-knowhow/notifications.md`). Distinct from *Offline*, where there's no SSE at all.

### Conversation
The left-hand side of the side-by-side desktop layout — the *thread* drawer plus the open thread — where you converse with the *Lucidos Agent* to direct the work. The counterpart to the *Canvas*: the Conversation is where intent is expressed; the Canvas is where the result lives and runs. Holds the list of your threads (the drawer) and the active one. The same surface on every device: on desktop it sits beside the *Canvas*; on mobile you swipe between them rather than seeing both at once.
See also: *Canvas*, *Live co-creation*.

### Current section
The thread-drawer section holding the live working set: every *thread* that isn't pinned or archived — whether running (the system's turn, shown via the *Active* row indicator), awaiting the user (their turn), or recently idle. Replaces the former Active + Review sections, merged so a thread no longer jumps sections every turn. Current is ordered by *thread* creation time (newest first) — a stable order that doesn't reshuffle as agents work or a thread gains a call-to-action. Attention-needing threads (awaiting answer/permission, pending *change*, failed) are NOT bubbled to the top; they're surfaced by a count badge and the attention filter icon instead. That count, summed with the Pinned section's, drives the thread-drawer toggle badge. The drawer's other sections are Pinned and Archive. (The Pinned section is user-facing terminology; the underlying section key, `is_saved`, and the `ThreadSaved`/`ThreadUnsaved` events still use "saved".)

### Domain event
An *event* the workspace itself emits via the `emit_event` LLM tool or `lucidos events emit` CLI — anything observable about the user's world (`MorningRoutineCompleted`, `JobListingFound`, `PanasonicHeatpumpAdjusted`). Persisted with the inner event type (not the literal string `"DomainEvent"`). Flows through the trigger matcher unconditionally, so a *trigger*'s `on_event:` can subscribe to any domain event name. Persisted `ThreadEvent` variants are also subscribable except per-token streaming ones — see *scheduler blocklist* (dev).
See also: `system-knowhow/thread-events.md` § "Today the scheduler uses a blocklist", `.claude/rules/rust.md` § "Apps — Event APIs".

### Environment variable
A user-managed, **non-secret** `NAME=value` pair (Settings → System → Environment variables) that Lucidos injects as a real environment variable into every subprocess it spawns — `run_bash`, `run_python`, background tasks, scheduled scripts, *triggers*, and *coding agent* sessions — e.g. `CLAUDE_CODE_USE_VERTEX`, `LUCIDOS_REPO`, build flags, default model names. Stored DB-backed (the `environment_variables` table), editable in Settings or by the *Lucidos Agent* via the `set_environment_variable` tool, and applied per-spawn so a change takes effect on the next tool call / agent turn with no engine restart. Deliberately distinct from a *credential*: env vars are non-secret (they appear in tool-call payloads, logs, and the *event* store — that's the point), whereas credentials hold secrets and feed the proxy auth pipeline. Names must be uppercase letters/digits/underscores (not starting with a digit) and may not clobber engine-owned names (`CRED_*`, `OAUTH_*`, `PG*`, `PATH`, internal `LUCIDOS_*`); engine-owned vars always win a collision. A credential can also be given a custom env var name so its secret injects as e.g. `GITHUB_TOKEN` **in addition to** the default `CRED_<NAME>` (an extra alias, so existing `CRED_<NAME>` references keep working).

### Event
A past-tense fact about something that happened in the workspace. Always past-tense, including transient ones. Two persistence flavors live side by side: persisted events (written to the `events` table, replayable, drive projections, match triggers) and transient events (broadcast over SSE only, never persisted, never reach projections or the trigger matcher). Concrete subtypes: thread lifecycle events (`MessageReceived`, `ResponseGenerated`, …), system events (notifications, preferences, …), and *domain events*. There is no *command* concept — anything that would look imperative is reframed as a request event (e.g. `AppUiRefreshRequested`, not `RefreshAppUI`); a subscriber chooses whether to act.

### Imported
The `data/imported/` directory where imported external repositories land (via `RepositoryImported` events). Treated as *artifacts* — content is flattened into the workspace's git tree, not kept as nested git repositories. Distinct from the *external-repo coding-agent thread* surface, which runs against a *repository* (a separately registered external git repo path).

### In-app surface
The notification surface inside the Lucidos UI: the bell badge (unread count, top bar) and transient toast popups. Driven by the `NotificationCreated` SSE message landing on a connected page; decided locally by the page based on its own visibility, focused thread, and viewport state. Independent from the *OS surface* — a single notification can hit either, both, or (when auto-marked-read on the *source event*) neither.
See also: `system-knowhow/notifications.md` §§1, 4.

### Intent
What the user wants, in their words — stable, non-technical prose the LLM can read aloud back to the user without sounding like a script. Lives in `data/apps/<app>/intents/<name>.md` or, for triggers, in the `TriggerCreated` event payload's `run.intent` field. Length is whatever fits; never contains imperative *how* verbs (hit, parse, retry, fall back) — those belong in *knowhow*.
See also: `docs/taxonomy.md` § Intent vs Knowhow, `system-knowhow/intent-registry.md`.

### Knowhow
How to achieve an *intent*, in technical terms — API details, data formats, quirks, workarounds, fallbacks. Evolves every time Lucidos learns something new. Lives in `data/knowhow/<id>.md` (shared) or scoped to an app / trigger. Discovered at runtime by the LLM via the `load_knowhow` tool, matched by the file's frontmatter `name` + `description`.
See also: `system-knowhow/building-knowhow.md`, `docs/taxonomy.md` § Intent vs Knowhow.

### Live co-creation
The principle at the heart of Lucidos: you and the *Lucidos Agent* shape the whole living system — **data and presentation together** — continuously and in place, with no build → deploy → observe gap. Because the *Conversation* and the *Canvas* are both always live and a gesture apart — side by side on desktop, a swipe apart on mobile — and the Canvas is backed by the real workspace (apps read live data through the SDK), you research, build, and iterate the whole thing inside Lucidos in one continuous motion, instead of building something, deploying it, and only then seeing how it behaves against real data. The back-and-forth between Conversation and Canvas — whichever way it's rendered — is the surface of live co-creation; the depth is that a single conversation reaches the entire stack.

### Lucidos Agent
The LLM driving a *thread* on the user's behalf — chat responses, trigger-thread runs, sub-thread callbacks, anything the LLM authored. UI actor-chip label: "Lucidos Agent". Returned by `mcp_client_name(ActorMode::Agent)` in `crates/lucidos-engine/src/mcp/client.rs`. Contrast with *Lucidos Engine*.

### Lucidos Engine
The engine itself acting without LLM mediation — recovery sweeps, *hardening*, scheduler ticks, system-initiated cancellations. UI actor-chip label: "Lucidos Engine". Returned by `mcp_client_name(ActorMode::Engine)`.

### Marketplace
A registered git repository (or GitHub tree URL) that the *App Store* (the *Apps* section's Store tab) scans for installable *plugins*. Stored in `data/config/plugin-marketplaces.json`; added/removed under Settings → Marketplaces (or the `register_plugin_marketplace` tool). A marketplace can contain a single plugin at its root or multiple plugin directories; GitHub marketplace subdirectories are converted into GitHub tree install URLs. The engine scans registered marketplaces at startup, after registration changes, and every five minutes; when an installed plugin has a newer version it notifies the user (a single deduplicated "updates available" notification) rather than applying the update automatically — the user reviews and applies it from the Apps section.

### MCP permission card
The approval card the *Lucidos Agent* shows when it wants to call a tool on an *MCP* server that isn't already trusted. Same UI as the *command permission card*: Deny, Allow once, Allow for this thread, Always allow this tool, or Always allow this server. "Always allow" choices are remembered in an editable list (`~/.lucidos/mcp-allowed-tools`) — per-tool (`Mcp(<server>:<tool>)`) or whole-server (`Mcp(<server>:*)`). Until answered, the thread waits on the user. A *trigger* fires unattended, so it never shows this card — MCP tool calls in a trigger thread are auto-approved silently (as is any call to a server with auto-approve set).

### Model registry
The database-backed list of chat models the user manages in **Settings → Models**. It drives the *Lucidos Agent* model picker and tells the engine which *provider* serves each model. Known models are seeded by the engine; the user can add their own (id + label + provider) and enable/disable or delete them (builtins are disable-only). Separate from the *Claude Code* model picker, which keeps its own list.

### Provider
The backend that serves a *model*: **Vertex AI**; **Anthropic** (direct, via `api.anthropic.com` — supports a Claude subscription OAuth token or an API key); **OpenAI** (direct, via `api.openai.com` — an API key, with the `OPENAI_API_KEY` launch env var as a fallback); **OpenRouter** (via `openrouter.ai/api/v1` — a Bearer API key, with the `LUCIDOS_OPENROUTER_API_KEY` env var as a fallback; serves e.g. GLM 5.2); or **Local** (any OpenAI-compatible server — Ollama / LM Studio / vLLM / llama.cpp — at a configurable base URL, default Ollama `http://localhost:11434/v1`, API key optional). OpenAI, OpenRouter, and Local all speak the OpenAI Chat Completions wire format but are distinct backends. Each entry in the *model registry* names its provider; the provider's credentials are configured once under Settings → Models → Providers. The same Claude model can be offered through more than one provider (e.g. Fable 5 via direct Anthropic, other Claude models via Vertex).

### OS surface
The notification surface outside the Lucidos UI: an OS-level notification banner. Two transports, chosen by client:
- **Web push** (browser / PWA): delivered by the device's push service (APNs on iOS, FCM on Chrome/Edge, Mozilla autopush on Firefox) and rendered by the registered service worker. Each push is required by the browser to result in a visible `showNotification()` call (`userVisibleOnly: true`) — silent pushes are penalised and can revoke the subscription.
- **Native desktop** (Tauri app): a native macOS notification driven by the *NativePushRequested* SSE, rendered + tap-routed by the app's `show_native_notification` command via Apple's `UserNotifications` framework (`UNUserNotificationCenter`). The embedded WKWebView can't subscribe to Web Push, so the engine reaches the desktop app over the open SSE stream instead. Requires a packaged `.app` build (inert in `tauri dev`).

Both ride the engine's single push-allowed decision (see PresenceCheck protocol), so a given notification reaches a device through exactly one transport and never collides with the *in-app surface* toast. Independent from the *in-app surface*.
See also: `system-knowhow/notifications.md` §§1, 3, 4.

### Parent thread
The direct ancestor of a *child thread*. Resolved via the child's `parent_thread_id` column. A thread can have at most one parent; a parent can have many children.

### Plugin
A bundle of installable workspace content shipped as a single unit. Contains any of `apps/`, `knowhow/`, `triggers/`, `scripts/`, `auth-modules/` — mirroring the top-level `data/` directories. Defined by a *plugin manifest* at the root. At install time the contents merge into the target workspace's `data/`. Use a plugin when the pieces only make sense together (e.g. an app + its knowhow + its trigger); ship single files individually otherwise.
See also: `system-knowhow/building-a-plugin.md`.

### Plugin manifest
The `manifest.toml` file at the root of a *plugin*. Declares `id`, `name`, content categories, and install-time setup steps. Schema in `system-knowhow/building-a-plugin.md`.

### PresenceCheck
The transient SSE event the *Lucidos Engine* broadcasts on every `NotificationCreated` to ask every connected page for its live presence. A **pure pong trigger** — it carries `notification_id`, `event_id` (so the pong can report `event_in_viewport`), a `deadline_ms` the page reads off the payload (set by `scheduler::push::DEADLINE_MS`, currently 2 s — sized to cover an iOS PWA's first packet after Tailscale wake-from-idle, where Tailscale's userspace WireGuard renegotiation pushes the round-trip into the 1100–1800 ms band), and `sent_at_ms`. It carries NO toast content: the in-app toast is driven separately by *NotificationToastRequested*, so it can no longer race the push decision. Each page answers with a *PresencePong*. The engine collects pongs up to the deadline and uses them to decide whether to send an *OS surface* push. Skipped entirely only when nobody is reachable — no page holds an open SSE connection AND no device has pinged visible within `PRESENCE_STALE_AFTER` (120 s, `core::device_presence`). The live SSE-connection count is the primary gate (`engine.sse_connections`); the heartbeat candidates are secondary (`expected_pong_count` in `scheduler::push`). The SSE count is what makes this robust — iOS suspends the 30 s heartbeat while a PWA is foregrounded, so the heartbeat row goes stale even though the page is connected and would pong; gating on the open connection lets the active page still suppress the push. See `system-knowhow/notifications.md` §3.

### PresencePong
The page's response to a *PresenceCheck*. POSTed to `/api/v1/presence-pong` with `notification_id`, `device_id`, `is_active`, `focused_thread_id`, `event_in_viewport`. The engine's decision: an OS push goes out iff NO pong reports `is_active`; multi-tab pongs on the same device OR within the device. Late pongs (after the deadline) ack 200 and are dropped — the race is normal. See `system-knowhow/notifications.md` §3.

### NotificationToastRequested
The transient SSE event the *Lucidos Engine* emits to drive the *in-app surface* toast. Emitted from the `NotificationCreated` fan-out **only on the push-suppressed branch** — i.e. when the *PresenceCheck* pongs say an *active device* exists, so the *OS surface* push is withheld. Carries the toast content (`title`, `body`, `thread_id`, `event_id`, `app_id`, `tap`, `sent_at_ms`) so the page renders without a re-fetch; active pages render the toast (or auto-read when looking at the *source event*), hidden pages ignore it. Because it and the OS push hang off opposite branches of one decision, a device never receives both for one notification — the in-app toast and the OS push are mutually exclusive by construction, not by a page-side timing race. See `system-knowhow/notifications.md` §4.

### NativePushRequested
The transient SSE event the *Lucidos Engine* emits to drive the native-desktop *OS surface*. The mutually-exclusive complement of *NotificationToastRequested*: emitted from the `NotificationCreated` fan-out **only on the push-ALLOWED branch** (no *active device*). Same payload shape as the toast event. A connected Tauri desktop app renders a native macOS notification from it via its `show_native_notification` command, using Apple's `UserNotifications` framework (`UNUserNotificationCenter`) with a delegate that captures the click to route the tap — the embedded WKWebView can't receive Web Push, so this is the desktop counterpart of the web-push fan-out, delivered over the open SSE stream. Native delivery needs a packaged `.app` build (inert in `tauri dev`). Browser / PWA pages ignore it (the handler gates on Tauri) and receive the real web push on the same branch. See `system-knowhow/notifications.md` §§1, 4.

### Script
Code (Python, shell, JS) invoked by an *intent* or *knowhow*. Lives with its primary consumer when scoped (`data/apps/<id>/scripts/`, `data/triggers/<slug>/scripts/`, `data/knowhow/<domain>/scripts/`) — or at top level (`data/scripts/`) when shared across multiple consumers (mirroring how knowhow can be standalone or app-scoped).

### Setup thread
A *Lucidos Agent* *thread* the engine spawns when a *plugin* that shipped a `setup` field is installed (from the *App Store* button or the `install_plugin` tool). On an **update** it spawns only when the new version's `setup` differs from the installed version's — an unchanged version bump re-runs nothing. It's seeded with the plugin's setup instructions and walks the user through completing them — asking for credentials/choices and doing the wiring it can. The user is navigated straight into it on install. Its id is recorded in the `PluginInstalled` event so the App Store card's *Setup→Open* button can reopen it; the card treats setup as done once the thread is no longer `running` or `waiting_for_user_answer`, and also once the thread is gone entirely (no summary row and no live *Thread Queue* entry), so a lost or stale setup thread degrades to *Open* rather than a *Setup* button that errors. The engine's background marketplace update check never installs (it only notifies), so it never spawns a setup thread — those come only from a user-confirmed install.

### Side-effect grant
The set of irreversible side-effect categories a *trigger* is pre-authorized to perform unattended (set per-trigger in the trigger's settings under "Allowed side-effects"). A trigger fires with no human present, so the *command guard* can't pause to ask it — it consults the grant instead: an irreversible command (sending email, a mutating HTTP request, a cloud-CLI change, out-of-workspace destruction, or anything else irreversible) runs only if its category is in the grant; otherwise the command is blocked and the trigger run fails (a failure notification surfaces the missing grant). Categories: **email**, **external API**, **cloud CLI**, **out-of-workspace destruction**, **other**. The default is empty (no irreversible side-effects allowed). Only consulted when the *command guard* is on; chat turns ignore the grant entirely (they're asked every time). The grant is set by the user, not by the agent — `create_trigger`/`update_trigger` (the LLM tools) can't set it, so an agent can't widen its own unattended authority. The same grant **also governs *coding-agent thread*s a trigger spawns** (Claude Code / Codex), where it's inherited down the spawn tree: such a thread runs unattended, so the engine resolves the coding agent's permission cards from the root trigger's grant instead of hanging — benign in-workspace work is auto-allowed, a granted irreversible category is auto-allowed, an ungranted one (or a catastrophic command) is auto-denied. Unlike the chat command guard, this denies the single request rather than failing the whole run, and applies regardless of the command-guard toggle. See `coding-agent-events.md` § "Unattended auto-resolution".

### Signer manifest
The `<name>.manifest.json` sidecar next to a `<name>.wasm` signer artifact in `data/auth-modules/`. Carries WASM-host metadata (`secret_handles`, `body_mode`, `capabilities`). The engine never auto-loads provider config from it — `data/config/apis.json` is the single source of truth for proxy entries.

### Source event
The specific *event* a notification points to, stored as `notifications.event_id`. Used by the *in-app surface* to decide whether the user is currently looking at the very thing the notification is about: if the page is on the source event's thread AND the source event is in viewport, the notification is auto-marked-read with no toast and no badge increment. A notification with `tap = { kind: 'navigate', to: { target: 'thread', id: '...', event_id: '...' } }` also lands on the source event (scroll + pulse).
See also: `system-knowhow/notifications.md` §§2, 4.

### Spawning thread
The *thread* that issued the `run_thread` / `run_coding_agent` / `lucidos spawn-thread` call. For `relation: "child"`, the spawning thread IS the parent. For `relation: "top"`, there's a spawning thread but no parent and no callback wiring.

### Sub-thread
Any descendant in the *thread* tree (transitive). A *child thread* is a sub-thread; a grandchild is a sub-thread. Use *child thread* when you mean the direct relationship; use *sub-thread* when depth is irrelevant or the relationship is transitive.

### Thread
A single conversation — a stream of events sharing one `aggregate_id`. Every chat reply, trigger run, and *coding-agent thread* run is a thread. Threads have a persisted `source` (`chat` / `trigger` / `claude_code` today — values are *channel* identifiers; see dev glossary), while user-facing/API source filters call the coding-agent bucket `coding-agent` and accept legacy `claude_code`. Threads also have a compose state (`composing` / `active` / `discarded` on the compose side; running / idled / failed on the runtime side), an archive flag (`inbox` / `archived`, orthogonal to compose state — an archived thread keeps `state='active'` and only flips `archive_state`), and may spawn other threads.

### Thread Queue
System-wide admission control for the shared thread pool. Every path that creates running work shares one capacity pool: background spawns — an event *trigger* firing, a scheduled (cron) fire, an agent-driven *sub-thread* or *coding-agent thread* spawn (`run_thread` / `run_coding_agent`, agent-mode `lucidos spawn-thread`, cross-workspace task POSTs) — AND user-initiated work (a person's chat / user-typed coding-agent threads). Within the *capacity policy* work runs immediately; over capacity it waits. User-initiated work is **prioritized, not exempt** (ADR 0008): it counts against the ceiling, drains ahead of background, ignores the per-kind/per-trigger caps, and queues only at true pool-max (a person briefly sees "requesting") — `reserved_background` keeps that priority from starving triggers/cron. Background ordering is FIFO (strict per trigger, best-effort across triggers) — except cron fires, which **coalesce** to at most one entry per trigger (a cron fire carries no distinct payload, so a redundant one is dropped rather than queued, and a restart's duplicate cron rows collapse to one on recovery; event triggers keep strict FIFO). Background entries are persisted (the `thread_queue` projection, event-sourced from `ThreadQueued` / `ThreadQueueAdmitted` / `ThreadQueueDropped` / `ThreadQueueCompleted`), so an engine restart re-queues work that never ran and drains it as capacity frees; user-initiated slots are in-memory only (a dead response is gone on restart, never re-fired). Surfaced in the **Thread Queue panel** (Running counts background + user; run now / drop / edit the capacity policy); a significantly delayed trigger or a pool at capacity raises notifications that tap through to the panel.
See also: `system-knowhow/thread-queue.md`, ADR 0008.

### Thread summary
A projected snapshot of a *thread*'s metadata — title, source, status, last activity, parent / trigger / repo links, coding-agent flags. The engine maintains it from the event stream on the `thread_summaries` DB table. **Same name everywhere**: DB table `thread_summaries`, Rust struct `ThreadSummary`, TS / JS SDK type `ThreadSummary`, wire JSON, this glossary entry. Returned by `GET /api/v1/threads/list`, `lucidos threads list`, the `list_threads` LLM tool, and `lucidos.threads.list()` — the single canonical surface for "give me thread metadata." Distinct from the *thread* itself (the underlying event sequence is the source of truth).
See also: `system-knowhow/lucidos-cli.md` § `lucidos threads list`, `system-knowhow/js-sdk.md` § `lucidos.threads`.

### Todo item
One row of a *todo list*. Three fields: `content` — imperative form ("Run tests"), what the item is. `active_form` — present-continuous form ("Running tests"); shown only while the item is `in_progress`, pending / completed / abandoned items render `content`. `status` — one of `pending`, `in_progress`, `completed`, `abandoned`. The first three are LLM-writable via `todo_write`; `abandoned` is engine-only — the engine flips any non-completed item to `abandoned` at response termination (see *Todo list*), so the user sees at a glance the agent walked away from it. Item shape (minus `abandoned`) matches *Claude Code*'s `TodoWrite` items on purpose — same mental model across agents.

### Todo list
A per-*thread*, *Lucidos Agent*-maintained list of *todo items* the agent is working through during a response. The agent calls the `todo_write` tool to set it; each call replaces the whole list (at most 50 items, at most one `in_progress`). Surfaced in the prompt bar as a collapsible indicator showing `completed/total` — expanding it shows the list. LLM-only writer in v1 — the user reads but does not toggle. *Coding-agent threads* continue to use *Claude Code*'s own `TodoWrite` rendering instead of this list (it shows inline on the tool-call step). Distinct from *intent* (the user's stable, prose goal) and *scheduled task* (a cron job).

**Abandonment.** When a response terminates (`ResponseGenerated` / `ResponseCanceled` / `ResponseAborted`), the engine looks at the thread's latest `TodoListWritten`. If every item is `completed`, the list is left alone — finished lists persist for the thread's lifetime. If any item is still `pending` or `in_progress`, the engine emits a new `TodoListWritten` with those items flipped to `abandoned` (completed items keep their status). The panel keeps showing the list — abandoned rows render with a dashed strike-through and an `abandoned` tag so it's obvious the agent did not see them through. To avoid the auto-flip the agent must either finish the list (every item `completed`) or call `todo_write` with `[]` to drop it explicitly.

### Top-thread
A spawn with `relation: "top"` (the CLI default for `lucidos spawn-thread`). Has no parent and no callback wiring — appears in the main thread list as an independent top-level thread. The *spawning thread* is **not** resumed when it finishes.

### Trigger
A workspace configuration that fires either on a schedule (`run.cron`) or on one of its *event subscriptions* (`on`). The `run` is one of two shapes:
- `run.type: "intent"` — spawns a *trigger thread* whose LLM is given `run.intent` (the user's voice — non-technical prose) as a user message and discovers the knowhow it needs via `load_knowhow` at fire time. No per-trigger knowhow allowlist.
- `run.type: "script"` — executes the *script* at `run.path` directly, no LLM. The engine sets `TRIGGER_EVENT_TYPE` / `TRIGGER_EVENT_PAYLOAD` / `TRIGGER_EVENT_ID` / `TRIGGER_EVENT_THREAD_ID` env vars on event fires so the script can branch deterministically and deep-link any notification back to the originating event. Right when the work is a deterministic transformation that doesn't need LLM judgement.

Lifecycle (both shapes): defined by `TriggerCreated`; each firing emits `TriggerStarted` then `TriggerCompleted`.
See also: `system-knowhow/building-a-trigger.md`, `docs/taxonomy.md` § Triggers.

### Event subscription
One entry in a *trigger*'s `on` list: an `event_type` plus an optional payload `condition` scoped to that event. A trigger fires when an incoming event matches *any* of its subscriptions' `event_type` and that subscription's `condition` (if any) evaluates true. A trigger may carry several subscriptions — the same workflow can react to multiple event types without duplicating the trigger, and each entry's filter only constrains its own event so different payload shapes never interfere.
See also: `system-knowhow/building-a-trigger.md` § "One trigger, multiple events".

### Trigger thread
A *thread* spawned by a *trigger* firing. Distinguished by `source = 'trigger'`. The LLM driving it has the same knowhow access as a chat thread: the system prompt advertises the intent registry, and the LLM calls `load_knowhow` when it judges a recipe relevant. No per-trigger knowhow allowlist. Terminal event: `TriggerCompleted`.

### Trigger group
A user-visible folder that organizes *triggers* in the triggers panel. Pure label: belongs to no agent, has no schedule, runs no code. Each trigger may belong to at most one group via `group_id`; ungrouped triggers render under an implicit "Ungrouped" section. Useful for surfacing emergent workflows — chains of triggers connected by `emit_event` → `on_event` — as a single group in the panel, without changing how they fire. Lifecycle: `TriggerGroupCreated`, `TriggerGroupRenamed`, `TriggerGroupReordered`, `TriggerGroupDeleted`. Groups with at least one member cannot be deleted — the LLM (or user) must reassign or delete the triggers first. Panel order is governed by `order: i32`, ascending.

### Wake question
An `ask_user_question` called with exactly one option. The *Lucidos Agent* uses it to step aside and let the user tap a single suggested follow-up — like "Show results" or "Stop sweep" — instead of typing a wake message. The agent's context lives in the `question` text; the option's label is the user-perspective prompt the tap effectively sends. The thread shows the "?" attention status (`WaitingForUserAnswer`) until the user taps or sends a free-form override (which auto-resolves the pending question).
See also: `system-knowhow/running-python.md` § The drain pattern.

### Workspace
A user's complete Lucidos instance: one PostgreSQL database inside the shared Lucidos Postgres cluster + `data/` directory (artifacts, apps, knowhow, triggers, intents, config, auth-modules, scripts). Multiple workspaces run concurrently (each is its own isolated engine + database), fronted by a single *workspace gateway* (dev term) that addresses each one by path prefix `/<slug>/`; you switch between them, and create / rename / delete them — toggle each one's *auto-start*, or **restore one from a backup** (drop in an encrypted `.enc` backup file + its backup key; you're asked for a workspace name only if the backup's original name collides with an existing one) — from the workspace picker (at `/~/`, or just `/` when there's more than one). Every workspace you've launched stays **listed** in the picker even after it stops; opening a stopped one starts it on demand. On first run there are no workspaces yet, so the picker asks you to name your first one (suggesting "personal" or "work") — nothing is auto-created for you.

### Auto-start
A per-workspace toggle (set in the workspace picker) controlling whether the *workspace gateway* brings that workspace's engine up automatically when the gateway starts. **On** = always-on: the workspace is spawned on every gateway (re)start (a packaged install's login-launched gateway brings up its auto-start workspaces). **Off** (the default for a newly-created workspace; turned on only via this toggle) = the workspace is still listed in the picker but its engine starts only when you explicitly open or launch it. An already-running workspace is re-adopted across a gateway restart regardless of this setting.

## Advanced — coding agents

These terms describe the surface for users running coding-agent workflows: Claude Code and Codex (the two coding agents), the Apply / Discard flow on Lucidos's own repo, the hardening gate, and the external-repo variant for working on user-added repositories. Most users never encounter these; chat and triggers cover the rest.

### Apply
The user-clicked action that merges a *coding-agent thread*'s worktree branch into `main`. The engine derives the button label from the touched files: **Apply** (no restart needed) vs **Apply & Restart** (restart needed). If the session didn't run *hardening* first, Apply runs it synchronously and the user waits. Source: `crates/lucidos-engine/src/engine/git_ops.rs` (`files_require_restart`).

### Apply All
The user-clicked action that triggers an *Apply* on every pending *change* in one batch. UI button label: **Apply All** (sibling to per-row *Apply* / Discard on the changes panel). Engine emits `ApplyAllBatchStarted` with the full change-id list + actor, then advances the batch as each member's `ChangeApplied` / `ChangeApplyFailed` event lands, and emits `ApplyAllBatchCompleted` with `applied: Vec<Uuid>` + `failed: Vec<ApplyFailure>` when every member has resolved. Member status is first-write-wins — one failure does not abandon the rest of the batch. Each member individually goes through the same *hardening* and restart-derivation rules as a single *Apply*. Persisted under aggregate `apply_all_batch`, `aggregate_id` = `batch_id` (UUID). While the batch runs, a sticky toast with a spinner shows progress and offers **Cancel** (`POST /api/v1/changes/apply-all/cancel`): the engine stops advancing to further members, interrupts the in-flight *hardening*/merge session, and marks the remaining members `failed` with "Apply All canceled" so the batch resolves and `ApplyAllBatchCompleted` still fires. Already-applied members stay applied; the rest return to pending (best-effort for an in-progress merge that already landed). A single *Apply* that woke a *hardening* or merge session can likewise be canceled from its *coding-agent thread* (the thread's Cancel button).

### Cancel (Stop)
The user-clicked **Stop** action on a working *coding-agent thread*. Behaves like pressing **Esc** in the *Claude Code* CLI: it *interrupts* the current turn but keeps the session resumable — the same `cc_session_id` and branch are preserved, so the next message continues the *same* conversation (a `--resume`) with full context. It is NOT a kill and NOT a fresh start. Emits `ResponseCanceled` (the visible "Canceled" chip) + `CodingAgentIdled` (the resume anchor). Distinct from *Apply* / Discard / Archive, which terminate the turn via their own lifecycle event. Routed through `interrupt_agent` (`POST /api/v1/claude-code/stop`, default `StopReason::UserStop`); a bounded fallback hard-stops only if the agent fails to honor the interrupt. Source: `crates/lucidos-engine/src/engine/claude_code/control.rs`, `agent_session/lifecycle.rs` (`SessionEndAction::KeepCanceledBranch`).

### Change
A *coding-agent*-proposed set of file edits shown as a pending branch in the UI. Resolved by *Apply* (merge into main, with optional restart) or Discard. Lifecycle events: `ChangeProposed`, `ChangeApplied`, `ChangeDiscarded`. Stored as a row in the `changes` table. Internal (Lucidos-repo) coding-agent threads produce changes; *external-repo coding-agent threads* skip this flow.

### Claude Code
Anthropic's coding-agent CLI; the default *coding agent* product Lucidos integrates (the other is *Codex*). Often abbreviated **CC**. Modeled in code as `CodingAgent::ClaudeCode` (enum, wire value `"claude-code"`). The thread channel value `"claude_code"` is historical and shared by every coding-agent thread regardless of backend — it means "coding-agent channel", not "this thread runs Claude Code"; the per-thread backend lives in the `coding_agent` column / event field.

### Codex
OpenAI's coding-agent CLI; the second *coding agent* product Lucidos integrates. Modeled in code as `CodingAgent::Codex` (enum, wire value `"codex"`). Picked per thread via the coding-agent chip on the *compose destination* picker (default: *Claude Code*, remembered per workspace via the `coding_agent_default` preference); the choice is locked at the thread's first message — an existing thread can never switch backends. Codex sessions run inside an OS sandbox scoped to the thread's *worktree*. User questions work the same as for Claude Code (Codex asks via the `ask_user_question` tool and the answer renders as the usual question card); permission cards appear when a Codex command or file change needs to escalate past the sandbox (default protocol — the `exec` escape-hatch protocol instead runs non-interactively with the sandbox as the only guard). The Apply / Discard flow, *changes*, and *hardening* work the same as for Claude Code.

### Coding agent
Role: a subprocess driving a *thread* to make code changes inside an isolated git *worktree* (dev). Lucidos integrates two coding agents: *Claude Code* (default) and *Codex*. Modeled in code as `CodingAgent` (enum). The thread it drives is a *coding-agent thread*; which agent drives it is chosen at the thread's first message and locked thereafter.

### Coding-agent thread
A *thread* driven by a *coding agent* (Claude Code or Codex) inside an isolated git worktree. Distinguished by `is_coding_agent = true` on `thread_summaries`. The persisted `source` value is `"claude_code"` for every coding-agent thread (historical channel name, backend-agnostic); public source filters should use `coding-agent`, with `claude_code` accepted only as a legacy alias. The `coding_agent` column identifies which product (`'claude-code' | 'codex'`, NULL = legacy Claude Code row); the `coding_agent_kind` column discriminates the worktree flavor (`'lucidos' | 'app' | 'external'`). Emits `CodingAgent*` events instead of chat `Response*` events. Three flavors:

- **Lucidos-internal coding-agent thread** — works on the Lucidos workspace repo itself. Produces *changes* surfaced via the Apply / Discard UI on completion.
- **App coding-agent thread** — works on a single app folder under the user's workspace (`data/apps/<id>/`) via a sparse-checkout *worktree* of the workspace git. Produces *changes* with the same Apply / Discard UI; Apply does **not** restart the engine and Lucidos's `/harden` does **not** run.
- **External-repo coding-agent thread** — works on a user-registered external git *repository*. Uses a different worktree-creation path and a minimal system prompt; **skips** the Lucidos change-proposal flow on session end.

See also: `system-knowhow/coding-agent-events.md`.

### External-repo coding-agent thread
A *coding-agent thread* (see) running against a user-registered external git *repository* rather than the Lucidos workspace itself. No Apply / Discard surface — the user reviews diffs via the external-repo diff viewer. Worktree creation and system prompt differ from the Lucidos-internal variant; documented in `docs/plans/2026-03-17-external-repos-plan.md`.

### App coding-agent thread
A *coding-agent thread* whose isolated *worktree* sparse-checks out the workspace git on a single `data/apps/<id>/` folder. Same machinery as a Lucidos-internal coding-agent thread (worktree, branch, *change*, *Apply* ff-merge) but on the user's workspace git rather than the Lucidos source repo. No engine restart on *Apply*; Lucidos's `/harden` does not run (apps own their hardening). On *Apply*, the engine emits a transient `AppUiRefreshRequested { app_id }` if any iframe-bundled file changed so open iframes reload with the merged content. The *WIP app preview* surface lets the user see the in-flight app from the worktree while the thread is still open. Branch name shape: `claude-code/app/<app_id>/<ts>-<uuid>`.
See also: `docs/plans/2026-05-27-app-coding-agent-threads-design.md`.

### WIP app preview
The in-progress rendering of an *app* served from an open *app coding-agent thread*'s *worktree* instead of from the workspace's main copy. Reachable by adding `?thread_id=<id>` to the app UI URL — the panel-overlay slot swaps from the live app (served from `<workspace>/data/apps/<id>/`) to the WIP (served from `<worktree>/data/apps/<id>/`). The WIP iframe loads its HTML/CSS/JS from the worktree, but its SDK calls (`lucidos.data.*`, `lucidos.events.*`) still hit the live workspace endpoints — data-coupled UI edits show their full effect only after *Apply*. The toggle reverts to live when the user navigates away from the thread or after *Apply* removes the worktree.

### Hardening
The quality gate every *coding-agent thread* must run via `/harden` before handing back to the user. Reviews the diff against project rules, runs relevant test suites (Rust + TS + e2e, auto-skipping irrelevant layers), and verifies system-knowhow drift. If the hardening marker is missing when the user clicks *Apply*, Apply runs `/harden` synchronously and the user waits.
See also: `CLAUDE.md` § "Hardening enforced at Apply time".

### Repository
A user-registered external git repository (row in the `repositories` table) that an *external-repo coding-agent thread* can run against. Distinct from `data/imported/` *imported* repos, which are flattened to plain files as *artifacts*.

## When to add a term

Add a term here when a new concept appears in the user-facing surface — UI strings, chat prose, app/trigger intents, knowhow file frontmatter, or `system-knowhow/*.md` content. If the new concept is dev-internal only (engine plumbing, DB schema, event-bus mechanics), put it in `docs/glossary.md` instead. Coding-agent-only concepts go in the **Advanced — coding agents** section.

## When a term changes

If a term is renamed, retired, or its meaning shifts, update this file in the same commit. Per `.claude/rules/system-knowhow.md`, every `system-knowhow/*.md` file that uses the term must be updated alongside. The `/harden` check enforces this — drift between code/UI and the glossary is a hardening failure.
