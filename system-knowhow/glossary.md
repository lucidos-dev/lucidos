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

### App
A user-installed mini-application with its own UI (HTML/CSS/JS) at `data/apps/<id>/`, plus optional *knowhow* / *intents* / *scripts* / *triggers*. Chat is not per-app: every conversation is a regular *chat thread*. When an app is open in the panel-overlay slot, its `manifest.json` and discovered context flow into the *Lucidos Agent*'s prompt, so the agent can answer in-context. Quick edits to an app happen through the agent's file tools on the chat path; heavier edits spawn an *app coding-agent thread*. The *app manifest* is user-facing metadata shown in the UI; `knowhow/` and `intents/` are engine-facing context loaded into the LLM when the app is active. The app's interactive surface is the *app UI* — see entry.
See also: `system-knowhow/building-an-app.md`, `docs/taxonomy.md` § Apps.

### App manifest
The metadata file for an app at `data/apps/<id>/manifest.json`. Holds name, description, icon — what the UI shows. **Not** loaded into the LLM context; operational knowledge belongs in `knowhow/`, not the manifest.

### App UI
The iframe that renders an app's HTML/CSS/JS (from `data/apps/<id>/ui/`) inside Lucidos's panel-overlay slot. Distinct from the *app* (the whole installed unit — UI plus scoped chat, knowhow, intents, scripts, triggers): "open the app" means open its UI inline and make its chat the active conversation; "refresh the app UI" means reload the iframe without changing chat context. The `navigate_ui` tool's `app-ui` target and the `AppUiRefreshRequested` event name both refer to this iframe surface specifically.

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
A *sub-thread* whose state requires user action to progress — paused on a user question (WaitingForUserAnswer), or a *coding-agent thread* with pending *changes*. Strict subset of *blocking descendants* — drops the `Running` case (running work is delegated, not pending attention). Counted in `attention_descendant_count` on the thread aggregate; bubbles the ancestor chain to the **Review** section so the user notices the attention card even when sibling descendants are still running.

### Cascading archive
Archiving a parent *thread* also archives every *sub-thread* under it, in one atomic operation. Disabled when any descendant is a *blocking descendant*.

### Chat thread
A *thread* whose `source = 'chat'` — the user typed the opening message in the Lucidos chat UI. Answered by the *Lucidos Agent*. Contrast with *trigger thread* and *coding-agent thread*.

### Child thread
A direct descendant *thread* created by a `relation: "child"` spawn (`run_thread` / `run_claude` / `lucidos spawn-thread --relation child`). The engine wires a callback so the *parent thread* resumes with the child's result when it terminates. Identifiers: DB column `parent_thread_id` on the child row points up to the parent; Rust struct field and event payload field `child_thread_id` (on the `Callback` struct and the `ChildThreadCompleted` event) name the child. A child is a *sub-thread*; the reverse isn't true.

### Config
Workspace configuration files under `data/config/`, principally `apis.json` (proxy entries, signer wiring, OAuth flows). Users edit these directly or via the engine's auth-handshake flow.
See also: `system-knowhow/building-an-auth-handshake.md`.

### Connected-but-hidden
A device whose Lucidos page is alive (SSE EventSource still streaming) but not currently *active*: a different browser tab is selected, the window is behind another app, or the iOS PWA is in the app switcher. Receives the `NotificationCreated` SSE message and updates its bell badge silently, but does NOT show a toast (the user can't see it). Eligible for an *OS surface* push (subject to global suppression in §2 of `system-knowhow/notifications.md`). Distinct from *Offline*, where there's no SSE at all.

### Domain event
An *event* the workspace itself emits via the `emit_event` LLM tool or `lucidos events emit` CLI — anything observable about the user's world (`MorningRoutineCompleted`, `JobListingFound`, `PanasonicHeatpumpAdjusted`). Persisted with the inner event type (not the literal string `"DomainEvent"`). Flows through the trigger matcher unconditionally, so a *trigger*'s `on_event:` can subscribe to any domain event name. Persisted `ThreadEvent` variants are also subscribable except per-token streaming ones — see *scheduler blocklist* (dev).
See also: `system-knowhow/thread-events.md` § "Today the scheduler uses a blocklist", `.claude/rules/rust.md` § "Apps — Event APIs".

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

### Lucidos Agent
The LLM driving a *thread* on the user's behalf — chat responses, trigger-thread runs, sub-thread callbacks, anything the LLM authored. UI actor-chip label: "Lucidos Agent". Returned by `mcp_client_name(ActorMode::Agent)` in `crates/lucidos-engine/src/mcp/client.rs`. Contrast with *Lucidos Engine*.

### Lucidos Engine
The engine itself acting without LLM mediation — recovery sweeps, *hardening*, scheduler ticks, system-initiated cancellations. UI actor-chip label: "Lucidos Engine". Returned by `mcp_client_name(ActorMode::Engine)`.

### OS surface
The notification surface outside the Lucidos UI: a web-push notification delivered by the device's push service (APNs on iOS, FCM on Chrome/Edge, Mozilla autopush on Firefox) and rendered by the registered service worker as an OS-level notification banner. Each push is required by the browser to result in a visible `showNotification()` call (`userVisibleOnly: true`) — silent pushes are penalised and can revoke the subscription. The engine decides server-side whether to send the push (see PresenceCheck protocol) so the SW always displays. Independent from the *in-app surface*.
See also: `system-knowhow/notifications.md` §§1, 3.

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

### Script
Code (Python, shell, JS) invoked by an *intent* or *knowhow*. Lives with its primary consumer when scoped (`data/apps/<id>/scripts/`, `data/triggers/<slug>/scripts/`, `data/knowhow/<domain>/scripts/`) — or at top level (`data/scripts/`) when shared across multiple consumers (mirroring how knowhow can be standalone or app-scoped).

### Signer manifest
The `<name>.manifest.json` sidecar next to a `<name>.wasm` signer artifact in `data/auth-modules/`. Carries WASM-host metadata (`secret_handles`, `body_mode`, `capabilities`). The engine never auto-loads provider config from it — `data/config/apis.json` is the single source of truth for proxy entries.

### Source event
The specific *event* a notification points to, stored as `notifications.event_id`. Used by the *in-app surface* to decide whether the user is currently looking at the very thing the notification is about: if the page is on the source event's thread AND the source event is in viewport, the notification is auto-marked-read with no toast and no badge increment. A notification with `tap = { kind: 'navigate', to: { target: 'thread', id: '...', event_id: '...' } }` also lands on the source event (scroll + pulse).
See also: `system-knowhow/notifications.md` §§2, 4.

### Spawning thread
The *thread* that issued the `run_thread` / `run_claude` / `lucidos spawn-thread` call. For `relation: "child"`, the spawning thread IS the parent. For `relation: "top"`, there's a spawning thread but no parent and no callback wiring.

### Sub-thread
Any descendant in the *thread* tree (transitive). A *child thread* is a sub-thread; a grandchild is a sub-thread. Use *child thread* when you mean the direct relationship; use *sub-thread* when depth is irrelevant or the relationship is transitive.

### Thread
A single conversation — a stream of events sharing one `aggregate_id`. Every chat reply, trigger run, and *coding-agent thread* run is a thread. Threads have a `source` (`chat` / `trigger` / `claude_code` today — values are *channel* identifiers; see dev glossary), a compose state (`composing` / `active` / `discarded` on the compose side; running / idled / failed on the runtime side), an archive flag (`inbox` / `archived`, orthogonal to compose state — an archived thread keeps `state='active'` and only flips `archive_state`), and may spawn other threads.

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
A user's complete Lucidos instance: PostgreSQL event store + `data/` directory (artifacts, apps, knowhow, triggers, intents, config, auth-modules, scripts). One workspace per user (typically); multiple workspaces can run concurrently on one host with isolated ports and DB containers.

## Advanced — coding agents

These terms describe the surface for users running coding-agent workflows: Claude Code (today's only coding agent), the Apply / Discard flow on Lucidos's own repo, the hardening gate, and the external-repo variant for working on user-added repositories. Most users never encounter these; chat and triggers cover the rest.

### Apply
The user-clicked action that merges a *coding-agent thread*'s worktree branch into `main`. The engine derives the button label from the touched files: **Apply** (no restart needed) vs **Apply & Restart** (restart needed). If the session didn't run *hardening* first, Apply runs it synchronously and the user waits. Source: `crates/lucidos-engine/src/engine/git_ops.rs` (`files_require_restart`).

### Apply All
The user-clicked action that triggers an *Apply* on every pending *change* in one batch. UI button label: **Apply All** (sibling to per-row *Apply* / Discard on the changes panel). Engine emits `ApplyAllBatchStarted` with the full change-id list + actor, then advances the batch as each member's `ChangeApplied` / `ChangeApplyFailed` event lands, and emits `ApplyAllBatchCompleted` with `applied: Vec<Uuid>` + `failed: Vec<ApplyFailure>` when every member has resolved. Member status is first-write-wins — one failure does not abandon the rest of the batch. Each member individually goes through the same *hardening* and restart-derivation rules as a single *Apply*. Persisted under aggregate `apply_all_batch`, `aggregate_id` = `batch_id` (UUID).

### Change
A *coding-agent*-proposed set of file edits shown as a pending branch in the UI. Resolved by *Apply* (merge into main, with optional restart) or Discard. Lifecycle events: `ChangeProposed`, `ChangeApplied`, `ChangeDiscarded`. Stored as a row in the `changes` table. Internal (Lucidos-repo) coding-agent threads produce changes; *external-repo coding-agent threads* skip this flow.

### Claude Code
Anthropic's coding-agent CLI; today's only *coding agent* product Lucidos integrates. Often abbreviated **CC**. Modeled in code as `CodingAgent::ClaudeCode` (enum); identified on the wire by the channel value `"claude_code"`. A second coding-agent product (e.g. Codex) would add its own enum variant and wire value.

### Coding agent
Role: a subprocess driving a *thread* to make code changes inside an isolated git *worktree* (dev). Today the only coding agent Lucidos integrates is *Claude Code*; future products would also be coding agents. Modeled in code as `CodingAgent` (enum). The thread it drives is a *coding-agent thread*.

### Coding-agent thread
A *thread* driven by a *coding agent* (today: Claude Code) inside an isolated git worktree. Distinguished by `is_coding_agent = true` on `thread_summaries`. The `source` value identifies which product (`"claude_code"` today); the `coding_agent_kind` column discriminates the worktree flavor (`'lucidos' | 'app' | 'external'`). Emits `CodingAgent*` events instead of chat `Response*` events. Three flavors:

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
