---
name: Lucidos Glossary
description: Canonical user-facing terms: app, artifact, intent, knowhow, plugin, script, thread, trigger, workspace, event, and the coding-agent ones (Apply, change, Claude Code, coding agent, hardening). Load to disambiguate one, or when a concept seems to have two names.
---

# Lucidos Glossary

Canonical definitions for the words Lucidos uses with the user. One term, one place, one definition. When prose in another file (`system-knowhow/*.md`, the engine system prompt, app/trigger intents, UI strings) names a term below, it uses *this* meaning — never a synonym.

If you find yourself reaching for a near-synonym (*child thread* for the transitive descendant concept *sub-thread*, *task* for *intent*, *recipe* for *knowhow*, *attachment* for *artifact*), use the canonical word from this file instead. If a needed concept genuinely isn't here, add it in the same change that introduces it.

Terms are split into two sections: **Core** terms most users encounter, and **Advanced — coding agents**, the surface only users running coding-agent workflows hit (everything around Claude Code, worktree-based code changes, the Apply / Discard flow, hardening).

This file is the **base layer**. `docs/glossary.md` extends it with dev-only internal terms (aggregate, actor, ActorMode, EventBus, BusEvent, ThreadEvent, projection, worktree, …) that the workspace LLM doesn't need.

## Core terms

### Active device
A device currently reporting itself visible to the engine. On desktop: `document.visibilityState === 'visible'` AND `document.hasFocus()` (the tab is in the foreground stack AND the browser window has OS focus). On iOS PWA standalone: `visibilityState === 'visible'` only (Safari leaves `hasFocus()` false even when the PWA is fully foregrounded). On the Tauri desktop client this is additionally gated on the *native window* being active (focused AND on-screen): the embedded WKWebView can't observe macOS `orderOut:` (a window trayed to the menu bar keeps `visibilityState='visible'` / `hasFocus()=true`), so the authoritative AppKit state is bridged to the page via a `native-window-active` event and a trayed/unfocused client correctly reports inactive. When at least one device is active at notification time, no OS push fires anywhere — the active device gets the *in-app surface* via the `NotificationCreated` SSE channel instead. Determined per-notification by the PresenceCheck protocol, not by a stale heartbeat window.
See also: `system-knowhow/notifications.md` §§1, 3.

### Active (thread state)
A *thread* that is currently doing work — the system's turn — surfaced by the row's status icon, **not** by a separate section. A thread no longer changes section when it starts or stops running: it stays in the *Current section* and just shows (or drops) the Active indicator in place. Contrast the *Current section* (where it lives) and *attention* (the user's turn — a pending *change*, an awaiting answer, or a failure — surfaced by a count badge and the attention filter rather than by reordering Current).

As a **query filter** (`--active` on `lucidos threads list` / `count`, `active` on `GET /api/v1/threads/{list,count}`, the `threads` tool and `lucidos.threads`), "active" is the wider **union** of the `running` and `waiting_for_user_answer` statuses: the agentic loop is mid-flow in either direction. Those two are opposites in the sense that matters to a caller, so the union answers "is anything busy?" wrongly, because a thread parked on an unanswered question is blocked on the human, not working. Ask that question with the *status filter* instead.

<!--gloss-app-start-->
### App
A user-installed mini-application with its own UI (HTML/CSS/JS) at `data/apps/<id>/`, plus optional *knowhow* / *intents* / *scripts* / *triggers*. Chat is not per-app: every conversation is a regular *chat thread*. When an app is open in the panel-overlay slot, its `manifest.json` and discovered context flow into the *Lucidos Agent*'s prompt, so the agent can answer in-context. Quick edits to an app happen through the agent's file tools on the chat path; heavier edits spawn an *app coding-agent thread*. The *app manifest* is user-facing metadata shown in the UI; `knowhow/` and `intents/` are engine-facing context loaded into the LLM when the app is active. The app's interactive surface is the *app UI* — see entry.
See also: `system-knowhow/building-an-app.md`, `docs/taxonomy.md` § Apps.
<!--gloss-app-end-->

### App-icon badge
The unread-notification *count* painted on the installed app's **icon**: a PWA's home-screen icon via the web Badging API, or the Tauri macOS **dock** icon via the dock tile. The macOS client carries the same count in its **menu-bar tray-icon title** as well, at all times, including while it is menu-bar-only and has no dock tile to badge. Distinct from the *in-app surface*'s bell badge, which lives inside the Lucidos UI. The value depends on the install: a **workspace PWA** (installed at `/<slug>/`, or a direct engine at `/`) shows its own workspace's unread count; the **gateway-root PWA** (installed from the workspace picker at `/~/`) and the **Tauri desktop app** show the aggregate total across running workspaces. The aggregate is running-workspaces-only: the gateway HTTP-polls each running engine's count (it holds no database handle), so a stopped workspace contributes nothing. A workspace PWA's icon and its bell badge must never show different numbers while the app is open: because the OS also writes the icon (from a push payload's `app_badge`) behind the page's back, the page **re-asserts** the count rather than writing it only when the number changes. While the app is closed the icon can lag a read that happened elsewhere, until the next push or the next time the app is opened. See `system-knowhow/notifications.md` § App-icon badge.

### App manifest
The metadata file for an app at `data/apps/<id>/manifest.json`. Holds name, description, icon — what the UI shows. **Not** loaded into the LLM context; operational knowledge belongs in `knowhow/`, not the manifest.

### App UI
The iframe that renders an app's HTML/CSS/JS (from `data/apps/<id>/ui/`) inside Lucidos's panel-overlay slot. Distinct from the *app* (the whole installed unit — UI plus scoped chat, knowhow, intents, scripts, triggers): "open the app" means open its UI inline and make its chat the active conversation; "refresh the app UI" means reload the iframe without changing chat context. The `navigate_ui` tool's `app-ui` target and the `AppUiRefreshRequested` event name both refer to this iframe surface specifically.

### API caller
An external HTTP caller of the engine's `/api/v1/...` surface that did NOT self-identify as one of the known actors (`You`, `Lucidos Agent`, `Lucidos Engine`, `System`). Reaches the engine without an `x-lucidos-device-id` (no browser session) and without an `x-lucidos-agent-origin-token` (not a Lucidos-spawned subprocess). UI actor-chip label: "API caller" with the 🔌 icon; the origin popover discloses the User-Agent string for forensics. Reserved label so an anonymous mutating POST can never impersonate the user as "You". A Lucidos-spawned `run_python` / `run_bash` subprocess that hand-rolls `urllib.request` / `curl` instead of using the `lucidos` CLI used to fall in here — the venv agent-origin shim (`crates/lucidos-engine/src/runtime/python.rs`), a `.pth`-loaded `_lucidos_agent_origin` module that survives a host `sitecustomize.py` such as Homebrew's, now auto-forwards the agent-origin token on Python calls to the engine port so those land as `Lucidos Agent` instead.

<!--gloss-artifact-start-->
### Artifact
A user-owned file under `data/artifacts/`. Git-tracked, never auto-deleted. Includes notes, imported API data, project folders, screenshots, generated images. The durable counterpart to ephemeral runtime state under `.lucidos/`.
See also: `system-knowhow/best-practices.md` § `artifacts/`.
<!--gloss-artifact-end-->

### Auth module
A WASM signer (plus optional `<name>.manifest.json` *signer manifest*) installed under `data/auth-modules/` to sign outbound proxy requests. Plugins can ship auth modules in their `auth-modules/` directory; the install-time LLM walks the user through wiring the matching `apis.json` snippet. Engine-side mechanics (host imports, capabilities, body modes) live under *signer* in `docs/glossary.md`.
See also: `system-knowhow/building-an-auth-handshake.md`.

### Blocking descendant
A *sub-thread* whose state currently prevents its ancestor from being cascade-archived: Running, paused on a user question (WaitingForUserAnswer), or a *coding-agent thread* with pending *changes*. Counted in `blocking_descendant_count` on the thread aggregate; surfaced in the frontend to hide the Archive button when non-zero.

### Attention-needing descendant
A *sub-thread* whose state requires user action to progress — paused on a user question (WaitingForUserAnswer), or a *coding-agent thread* with pending *changes*. Strict subset of *blocking descendants* — drops the `Running` case (running work is delegated, not pending attention). Counted in `attention_descendant_count` on the thread aggregate; bubbles the ancestor chain to the **Current** section so the user notices the attention card even when sibling descendants are still running.

### Canvas
The right-hand side of the side-by-side desktop layout — where the live system materializes and you act on it: an open *app*'s *app UI*, a file or *artifact* preview, a *change*'s diff, settings, a URL. The counterpart to the *Conversation*: you direct the *Lucidos Agent* in the Conversation, and its output appears — live and usable — in the Canvas. Not a static preview — apps in the Canvas read real workspace data through the SDK, so reshaping things from the Conversation updates the Canvas immediately (see *Live co-creation*). The same surface on every device: on desktop it sits beside the *Conversation* (the side-by-side layout); on mobile it's one swipe away rather than shown alongside. Canvas names a **side**, not a pane — it's what that side is *for*; the single pane filling it is the *content pane*. Reach for "Canvas" when the subject is the side or the Conversation↔Canvas back-and-forth, and for "content pane" when the subject is the pane itself (where a view lands, a shortcut, a resize).
See also: *Conversation*, *content pane*, *Live co-creation*.

### Capacity policy
The configurable caps governing the *Thread Queue*: `max_concurrent_total` (the shared ceiling for ALL threads — background spawns and user-initiated work alike), per-kind caps (event trigger / scheduled / sub-thread / coding agent — background only), per-*trigger* concurrency, a per-trigger queue ceiling with a choice of overflow behavior (drop the oldest waiting fire + notify, or pause the trigger + notify), and `reserved_background` — slots background can reclaim ahead of user-initiated work so user priority can't starve triggers/cron (0 = pure user priority). Concurrency caps of 0 mean "hold" — admission pauses and the queue accumulates until the cap is raised. Edited in the Thread Queue panel (`PUT /api/v1/thread-queue/policy`); stored event-sourced — the latest `CapacityPolicyChanged` event IS the policy. User-initiated work is prioritized but still subject to the ceiling (ADR 0008).

### Cascading archive
Archiving a parent *thread* also archives every *sub-thread* under it, in one atomic operation. Disabled when any descendant is a *blocking descendant*.

### Chat thread
A *thread* whose `source = 'chat'` — the user typed the opening message in the Lucidos chat UI. Answered by the *Lucidos Agent*. Contrast with *trigger thread* and *coding-agent thread*.

### Child follow-up
A message from a *parent thread* to one of its own *child threads*, sent with the `follow_up_child_thread` tool. The one privileged cross-thread write: it redirects a child that is going the wrong way, hands a child something a sibling learned, or tells a stalled child to continue. Deliberately not an any-to-any address space, a thread can address its **direct** children and nothing else: no sibling can address a sibling, no grandparent can reach a grandchild (it goes through the child), and no cross-workspace caller has children to address. The caller never states the relationship; the engine looks it up from the child's `parent_thread_id`. A follow-up returns as soon as the message is on the child's timeline and does **not** wait for the child, which reports back the usual way when its turn ends. It never creates a thread, never changes how many children the parent has, and consumes no child slot, so reviving an existing child is cheaper than spawning another one. By default it **queues**: a mid-turn child reads it at its next natural break. To stop the child's current turn instead, see *urgent follow-up*.
See also: *child thread*, *parent thread*, *urgent follow-up*.

### Child thread
A direct descendant *thread* created by a `relation: "child"` spawn (`run_thread` / `run_coding_agent` / `lucidos spawn-thread --relation child`). The engine wires a callback so the *parent thread* resumes with the child's result when it terminates. The reverse direction also exists: the parent can address a child it already spawned with a *child follow-up*, and a child that is followed up on reports again when its next turn ends. Identifiers: DB column `parent_thread_id` on the child row points up to the parent; Rust struct field and event payload field `child_thread_id` (on the `Callback` struct and the `ChildThreadCompleted` event) name the child. A child is a *sub-thread*; the reverse isn't true.

### Command guard
An opt-in safety gate over the *Lucidos Agent*'s shell/Python tools. Toggled under **Settings → Permissions → Command safety** (off by default). When enabled, a command that's clearly catastrophic (`rm -rf /`, a fork bomb, formatting a disk) is refused without running; a command that looks like an irreversible real-world side-effect (sending mail, a mutating HTTP request, a cloud-CLI change, spending money) or destruction outside the workspace pauses and shows the user a *command permission card* to approve; an in-workspace deletion/overwrite (recoverable) runs after a *command checkpoint* is taken, leaving a one-click Undo. A fast static check settles the obvious cases and a cheap LLM **judge** decides the ambiguous middle, erring toward asking. Under the master toggle, two sub-settings (only active while the guard is on): an **LLM judge** on/off switch (off falls back to a static classifier: the dangerous-command list plus a destruction scan, so out-of-workspace deletes/overwrites still ask and in-workspace ones still checkpoint) and the **judge model** (defaults to Haiku). Commands the user has chosen to always allow are kept in an editable list under **Settings → Permissions → Lucidos Agent permissions** (`~/.lucidos/agent-allowed-commands`). A *trigger* fires unattended, so it can't be asked, so it instead runs irreversible commands only within its declared *side-effect grant*; an ungranted one is blocked and fails the run. The safe majority of commands, including reads anywhere and writes inside the workspace, run untouched. See `system-knowhow/running-python.md` § The command guard.

### Command checkpoint
A pair of snapshots of the workspace's tracked content the *command guard* takes around an in-workspace destructive command (a delete or overwrite under the workspace), the recoverable, "reversible" lane. Instead of asking, the guard saves the current state, runs the command, saves the state again, and shows the command's card with a one-click **Undo** and a **Diff** button. Comparing the two snapshots is what tells Lucidos exactly what that command did, so Undo puts back what it deleted or overwrote **and** removes the files it created (leaving alone any you have edited since), and Diff shows you the whole thing. If the command turns out to have changed nothing Lucidos can see, no card appears at all: that happens when the target is a path git ignores, which the snapshot never captured, and an Undo there could neither restore nor remove anything. The snapshots are kept for 30 days so the diff stays viewable, then reclaimed. Out-of-workspace destruction can't be checkpointed, so it goes to the *command permission card* (ask) lane instead. Only taken when the *command guard* is on.

### Command permission card
The approval card the *command guard* shows when a shell/Python command needs the user's go-ahead. Same UI as the *coding-agent permission card*: Deny, Allow once, Allow for this thread, or Always allow (remembered for similar commands). Until answered, the thread waits on the user; answering lets the command run or refuses it. Unlike the coding-agent card, this lane's "Allow for this thread" is forgotten if Lucidos restarts.

### Compose destination
The compose view's single "who/where" pick for a new *thread*, shown as a single destination picker: either the *Lucidos Agent* (default — a *chat thread*) or a coding target — the Lucidos source, an *app*, or a registered *repository* — which spawns a *coding-agent thread* of the matching flavor. A one-line caption under the picker states the consequence of the current pick (the Lucidos Agent can hand off to a *coding agent*; a coding target produces a reviewable *change*, except *external-repo coding-agent threads* which review the diff from the thread). The Claude Code vs Codex pick is a separate coding-agent chip shown only for coding targets, remembered per workspace (`coding_agent_default` preference) and locked at the thread's first message. Replaces the former mode toggle + scope chain. The Lucidos source target is offered only on a dev build (a source checkout exists); a packaged install has no source tree to edit, so the picker hides it there (gated on the `/health` `packaged` flag). The *Lucidos Agent* path is gated the same way and by the same signal: its system prompt states whether this install has platform source, and `run_coding_agent` with `folder` omitted is refused when it doesn't — so the picker and the agent can never disagree about whether "Lucidos source" exists.

### Config
Workspace configuration files under `data/config/`, principally `apis.json` (proxy entries, signer wiring, OAuth flows). Users edit these directly or via the engine's auth-handshake flow.
See also: `system-knowhow/building-an-auth-handshake.md`.

### Connected account
A service the user has signed in to, so Lucidos can act on their behalf: the
stored result of an OAuth authorization (access token, refresh token, granted
scopes, and the account's email where the provider reports one). Listed under
**Settings → Accounts → Connected accounts**, one row per provider. Created by
the *Lucidos Agent*'s `connect_oauth_account` tool or by the Connect button on
that page; both hand the provider's authorization page to the user's own browser
(the in-app browser panel, their system browser, or a new tab, whichever they
have configured) and store the tokens when it comes back.

Distinct from the *credential* that backs it. A connected account is a **sign-in**;
the OAuth Client credential beside it is the **app registration**
(`client_id`, optional `client_secret`, and the provider's endpoint URLs) that
made the sign-in possible. One provider therefore shows one row in each list,
which is expected and not a duplicate. The registration is created inside the
Connect flow, prefilled from the *OAuth provider registry*, and saving it
continues straight into the browser: there is no second button to press. A
provider name may be a *derived provider*.

It records **two** scope sets: what the provider **granted**, and what it was
**asked for**. They differ whenever a provider refuses part of a request, which
is a real state rather than an error (a Dropbox app whose Permissions tab has not
enabled a scope). *Reconnect* re-requests the asked-for set, because re-requesting
the granted one could only ever ask for what the account already had.

Backup uploads read the connected account for their `backup_provider`; the Backup
page has no account UI of its own and links here, handing over the provider AND
the scopes an upload needs, so one authorization covers signing in and granting
access.
See also: *credential*, *OAuth provider registry*, *derived provider*,
*OAuth client type*, *OAuth redirect URI*,
`system-knowhow/oauth-providers.md`.

### Connected-but-hidden
A device whose Lucidos page is alive (SSE EventSource still streaming) but not currently *active*: a different browser tab is selected, the window is behind another app, or the iOS PWA is in the app switcher. Receives the `NotificationCreated` SSE message and updates its bell badge silently, but does NOT show a toast (the user can't see it). Eligible for an *OS surface* push (subject to global suppression in §2 of `system-knowhow/notifications.md`). Distinct from *Offline*, where there's no SSE at all.

### Content pane
The pane where an opened thing lands and runs: an *app*'s *app UI*, a file or *artifact* preview, a *change*'s diff, settings, a URL. The single pane filling the *Canvas* side — third of the three panes, alongside the *thread drawer* and the *thread pane*. CSS container `.pane-content` (`FocusedPane = 'content'`; on mobile the rightmost swipe pane, `MobileView = 'content'`).
See also: *Canvas*, *thread drawer*, *thread pane*.

### Conversation
The left-hand side of the side-by-side desktop layout — where you converse with the *Lucidos Agent* to direct the work. The counterpart to the *Canvas*: the Conversation is where intent is expressed; the Canvas is where the result lives and runs. Like Canvas, Conversation names a **side**, not a pane — but where Canvas is filled by a single *content pane*, the Conversation covers **two**: the *thread drawer* (the list of your threads) plus the *thread pane* (the open thread's transcript and prompt input). The same surface on every device: on desktop it sits beside the *Canvas*; on mobile you swipe between them rather than seeing both at once.
See also: *Canvas*, *thread drawer*, *thread pane*, *Live co-creation*.

### Current section
The *thread drawer* section holding the live working set: every *thread* that isn't pinned or archived — whether running (the system's turn, shown via the *Active* row indicator), awaiting the user (their turn), or recently idle. Replaces the former Active + Review sections, merged so a thread no longer jumps sections every turn. Current is ordered by *thread* creation time (newest first) — a stable order that doesn't reshuffle as agents work or a thread gains a call-to-action. Attention-needing threads (awaiting answer/permission, pending *change*, failed) are NOT bubbled to the top; they're surfaced by a count badge and the attention filter icon instead. That count, summed with the Pinned section's, drives the thread-drawer toggle badge. The drawer's other sections are Pinned and Archive. (The Pinned section is user-facing terminology; the underlying section key, `is_saved`, and the `ThreadSaved`/`ThreadUnsaved` events still use "saved".)

### Context window
How many tokens a *model* can hold in one request — prompt plus reply. The engine
sizes its context budget from this: it reserves room for the reply, then trims the
oldest *conversation* history and the largest tool results until the rest fits. A
window set too low means context is thrown away that the model could have held.

Each row in the *model registry* may declare its window (Settings → Models →
Context window). Leave it blank and the engine falls back to guessing from the
model id — a guess that knows only Claude and GPT-5 ids and treats everything
else as 200k, so an OpenRouter, Gemini, or local model is under-budgeted until
you set it. Every guess errs low deliberately: a window set too low only trims
early, while one set too high makes the engine build a prompt the provider
rejects. Builtins ship with theirs declared where it could be verified.
See also: *Model registry*, *Provider*.

### Credential
A secret Lucidos stores on the user's behalf: an API key, bearer token, username
and password, mailbox password, or an OAuth client registration. Listed under
**Settings → Accounts → Credentials**, keyed by a **service name**, and injected
into every subprocess Lucidos spawns as `CRED_<NAME>` (plus an optional custom
env var name as an extra alias). Also what the proxy auth pipeline
(`data/config/apis.json`) resolves when it signs an outbound request. Deliberately
distinct from an *environment variable*, which is non-secret by design and
appears in tool-call payloads, logs, and the *event* store.

A credential is identified by its service name **together with its auth type**,
not by the name alone. That matters for exactly one pair: an OAuth Client
registration may share a name with an ordinary credential for the same provider,
so `google` can be both an API key and the Google app registration, listed as two
rows telling themselves apart by their type badge. Every other type keeps a name
unique to itself, because that name is what `CRED_<NAME>` and `apis.json` resolve.
(Until 2026-08-05 the two engine-owned types wrapped their names in an `oauth:` /
`email:` prefix instead; the type carries that now, so the name is just the
provider or the mailbox account.) An OAuth Client is the one type NOT injected as
`CRED_<NAME>`: only the OAuth flow reads it, and it reads it from the database.
A secret is never a *preference*.
See also: *connected account*, *environment variable*, *config*.

### Derived provider
A provider name that is not itself a service, but a second, separately scoped
connection to one that is: a health-only connection on Google's endpoints under
its own name, say, so a narrowly-scoped *connected account* can be held apart
from the everyday one. Some APIs require this, refusing any token that also
carries unrelated scopes.

It gets its own *credential* and its own connected-account row, and runs on the
base provider's endpoints. Because aliases are ad hoc, a derived name is
deliberately absent from the *OAuth provider registry* and is never guessed from
its spelling: the Connect form asks which known provider it runs on, then fills
that provider's endpoints in while keeping the name you gave it.
See also: *connected account*, *OAuth provider registry*.

### Domain event
An *event* the workspace itself emits via the `emit_event` LLM tool or `lucidos events emit` CLI — anything observable about the user's world (`MorningRoutineCompleted`, `JobListingFound`, `PanasonicHeatpumpAdjusted`). Persisted with the inner event type (not the literal string `"DomainEvent"`). Flows through the trigger matcher unconditionally, so a *trigger*'s `on_event:` can subscribe to any domain event name. Persisted `ThreadEvent` variants are also subscribable except per-token streaming ones — see *scheduler blocklist* (dev).
See also: `system-knowhow/thread-events.md` § "Today the scheduler uses a blocklist", `.claude/rules/rust.md` § "Apps — Event APIs".

### Environment variable
A user-managed, **non-secret** `NAME=value` pair (Settings → System → Environment variables) that Lucidos injects as a real environment variable into every subprocess it spawns — `run_bash`, `run_python`, background tasks, scheduled scripts, *triggers*, and *coding agent* sessions — e.g. `CLAUDE_CODE_USE_VERTEX`, `LUCIDOS_REPO`, build flags, default model names. Stored DB-backed (the `environment_variables` table), editable in Settings or by the *Lucidos Agent* via the grouped `env_vars` tool (`list` / `set` / `delete`; `set_environment_variable` is a back-compat alias for `set`), and applied per-spawn so a change takes effect on the next tool call / agent turn with no engine restart. Deliberately distinct from a *credential*: env vars are non-secret (they appear in tool-call payloads, logs, and the *event* store — that's the point), whereas credentials hold secrets and feed the proxy auth pipeline. Names must be uppercase letters/digits/underscores (not starting with a digit) and may not clobber engine-owned names (`CRED_*`, `OAUTH_*`, `PG*`, `PATH`, internal `LUCIDOS_*`); engine-owned vars always win a collision. A credential can also be given a custom env var name so its secret injects as e.g. `GITHUB_TOKEN` **in addition to** the default `CRED_<NAME>` (an extra alias, so existing `CRED_<NAME>` references keep working).

<!--gloss-event-start-->
### Event
A past-tense fact about something that happened in the workspace. Always past-tense, including transient ones. Two persistence flavors live side by side: persisted events (written to the `events` table, replayable, drive projections, match triggers) and transient events (broadcast over SSE only, never persisted, never reach projections or the trigger matcher). Concrete subtypes: thread lifecycle events (`MessageReceived`, `ResponseGenerated`, …), system events (notifications, preferences, …), and *domain events*. There is no *command* concept — anything that would look imperative is reframed as a request event (e.g. `AppUiRefreshRequested`, not `RefreshAppUI`); a subscriber chooses whether to act.
<!--gloss-event-end-->

### File preview modal
A read-only view of one file, rendered by Lucidos over whatever the *content pane* is showing, without navigating there. Opened by an *app* through `lucidos.ui.previewFile` so a reader following a citation in a report glances at the file and carries on, instead of losing their place. It takes the same locators and the same `line` / `line_end` as the `file` navigation target (a workspace data path or a `repo:<repoId>:file:<path>` one), shows the same highlight and line numbers the *content pane*'s preview shows, and carries a link that escalates the glance into that full preview. Dismissed by Esc, a click outside it, or its close control. Distinct from the *content pane*'s file preview, which IS a navigation: that one replaces what the pane was showing and lands in the Back history.
See also: `system-knowhow/js-sdk.md` § lucidos.ui.

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

<!--gloss-live-cocreation-start-->
### Live co-creation
The principle at the heart of Lucidos: you and the *Lucidos Agent* shape the whole living system — **data and presentation together** — continuously and in place, with no build → deploy → observe gap. Because the *Conversation* and the *Canvas* are both always live and a gesture apart — side by side on desktop, a swipe apart on mobile — and the Canvas is backed by the real workspace (apps read live data through the SDK), you research, build, and iterate the whole thing inside Lucidos in one continuous motion, instead of building something, deploying it, and only then seeing how it behaves against real data. The back-and-forth between Conversation and Canvas — whichever way it's rendered — is the surface of live co-creation; the depth is that a single conversation reaches the entire stack.
<!--gloss-live-cocreation-end-->

### Lucidos Agent
The LLM driving a *thread* on the user's behalf — chat responses, trigger-thread runs, sub-thread callbacks, anything the LLM authored. UI actor-chip label: "Lucidos Agent". Returned by `mcp_client_name(ActorMode::Agent)` in `crates/lucidos-engine/src/mcp/client.rs`. Contrast with *Lucidos Engine*.

### Lucidos Engine
The engine itself acting without LLM mediation — recovery sweeps, *hardening*, scheduler ticks, system-initiated cancellations. UI actor-chip label: "Lucidos Engine". Returned by `mcp_client_name(ActorMode::Engine)`.

### Marketplace
A registered git repository (or GitHub tree URL) that the *Plugins panel* scans for installable *plugins* (its catalog, shown when the **Installed only** filter is unchecked). Stored in `data/config/plugin-marketplaces.json`; added/removed under Settings → Marketplaces (or the `register_plugin_marketplace` tool). A marketplace can contain a single plugin at its root or multiple plugin directories; GitHub marketplace subdirectories are converted into GitHub tree install URLs. The engine scans registered marketplaces at startup, after registration changes, and every five minutes; registering, renaming and removing one are announced, so an open *Plugins panel* (and Settings → Marketplaces) updates in place rather than waiting for a reload; when an installed plugin has a newer version it notifies the user (a single deduplicated "updates available" notification) rather than applying the update automatically: the user reviews and applies it from the *Plugins panel* (with **Installed only** unchecked).

### Max tool calls
How many tool calls the *Lucidos Agent* may make in a single turn before the engine ends the turn, set under **Settings → Models → Chat & triggers** (default 500). It counts individual calls, not replies, so three calls in one reply spend three of them, and it applies to *trigger* runs exactly as to chat. There is deliberately **no maximum**: a high cap costs time and tokens, which is the user's call to make, and roughly speaking the cap is how long a single turn can run (around 15 seconds per call, so 500 is a couple of hours). The minimum is 1. Reaching it is not an error: the turn ends with a message prefixed `[ENGINE-LIMIT]` that names the limit, links to the setting, and can be continued by sending any message. That prefix is the only trustworthy signal the limit was hit, since the agent cannot observe its own tool-call count and will otherwise invent one. Only the user can change it; the agent is refused, because the cap is the backstop over the agent's own work. Distinct from the *command guard*, which judges whether one command is safe rather than how many run.

### MCP permission card
The approval card the *Lucidos Agent* shows when it wants to call a tool on an *MCP* server that isn't already trusted. Same UI as the *command permission card*: Deny, Allow once, Allow for this thread, Always allow this tool, or Always allow this server. "Always allow" choices are remembered in an editable list (`~/.lucidos/mcp-allowed-tools`) — per-tool (`Mcp(<server>:<tool>)`) or whole-server (`Mcp(<server>:*)`). Until answered, the thread waits on the user. A *trigger* fires unattended, so it never shows this card — MCP tool calls in a trigger thread are auto-approved silently (as is any call to a server with auto-approve set).

### Model registry
The database-backed list of chat models the user manages in **Settings → Models**. It drives the *Lucidos Agent* model picker and tells the engine which *provider* serves each model. Known models are seeded by the engine; the user can add their own (id + label + provider, and optionally the model's *context window*) and enable/disable or delete them (builtins are disable-only). Separate from the *Claude Code* model picker, which keeps its own list.

### Provider
The backend that serves a *model*: **Vertex AI**; **Anthropic** (direct, via `api.anthropic.com`: supports a Claude subscription OAuth token or an API key, with the `ANTHROPIC_API_KEY` launch env var as a fallback below the stored credential); **OpenAI** (direct, via `api.openai.com`: an API key, with the `OPENAI_API_KEY` launch env var as a fallback, and below that an auto-detected key from the Codex CLI's `${CODEX_HOME:-~/.codex}/auth.json` `apikey` login as a lowest-precedence fallback, the parallel of Vertex reading the gcloud ADC file); **OpenRouter** (via `openrouter.ai/api/v1`: a Bearer API key, with the `LUCIDOS_OPENROUTER_API_KEY` env var as a fallback; serves e.g. GLM 5.2); or **Local** (any OpenAI-compatible server, Ollama / LM Studio / vLLM / llama.cpp, at a configurable base URL, default Ollama `http://localhost:11434/v1`, API key optional). OpenAI, OpenRouter, and Local all speak the OpenAI Chat Completions wire format but are distinct backends. Each entry in the *model registry* names its provider; the provider's credentials are configured once under Settings → Models → Providers. The same Claude model can be offered through more than one provider (e.g. Fable 5 via direct Anthropic, other Claude models via Vertex).

### Builtin provider proxy
A *provider*'s API exposed to app UIs through the engine's proxy route (`lucidos.proxy(<name>).fetch(path, init)` → `/api/v1/proxy/<name>/<path>`) **without** the workspace re-entering the credential in `data/config/apis.json`. When `<name>` matches a *model registry* provider (`vertex`, `openai`, `openrouter`, `anthropic`, `local`) and no `apis.json` entry exists, the engine forwards to that provider's API root and injects the provider's own credential server-side — the same one configured under Settings → Models → Providers — so the secret never reaches the iframe. An `apis.json` entry with the same name overrides the builtin (it is consulted first). `vertex` is addressed by the publisher/model suffix only: the engine owns the `…/projects/<project>/locations/<region>` URL prefix and mints the access token, so the app never needs the project id or a token. See `system-knowhow/js-sdk.md` § `lucidos.proxy`.

### OAuth provider registry
The list of OAuth providers Lucidos knows the endpoints for, stored as
`system-knowhow/oauth-providers.json`. Each row carries a provider's
authorization, token and userinfo URLs, its userinfo method, its authorization
parameters, its base URL, and where to register an app with it: the console link,
which client type to pick, which permissions to enable.

It is what makes **Settings → Accounts** offer a quick button per provider and
prefill a whole app registration, so a *credential* of type OAuth Client needs
only a Client ID. Endpoints are copied into the credential when it is saved, so
the credential still fully describes its own flow and a registry that later moves
an endpoint cannot silently change one that already works; the *Lucidos Agent*
repairs a stale credential on request. A provider absent from the registry still
connects: the form asks for its endpoints, or for which known provider a *derived
provider* name runs on.

`system-knowhow/oauth-providers.md` is the prose beside it (redirect URI forms,
confidential versus public clients, scope notes) and does not restate the rows.
Adding a provider is an edit to the JSON, never an engine change.
See also: *connected account*, *credential*, *derived provider*,
*OAuth client type*.

### OAuth redirect URI
The loopback URL the provider sends the user back to after they authorize, and which Lucidos must repeat byte-for-byte when it redeems the authorization code. Lucidos runs a temporary listener on a fixed port for exactly this callback, binding **both** loopback families, so three host forms are receivable: `http://127.0.0.1:14981/oauth/callback` (the default), `http://localhost:14981/oauth/callback`, and `http://[::1]:14981/oauth/callback`. The port and path are the engine's; only the host form is configurable, via the optional `redirect_uri` key on the *credential* — needed because providers disagree (Spotify rejects the name form, Microsoft's Entra portal rejects the IP form under its Web platform). The user must register the resolved URI with the provider exactly. Which form a given provider wants is recorded in `system-knowhow/oauth-providers.md`, never in engine code.

### OAuth client type
Whether Lucidos authenticates the token exchange as a **confidential client** (sends the `client_secret`) or a **public client** (sends no secret and proves the exchange with PKCE instead, per RFC 8252). Derived from one thing — whether the *credential* carries a `client_secret` — so the engine never needs to know which provider it is talking to. It must match how the app is registered with the provider: a web/confidential registration rejects a secret-less redemption, and a desktop/native/public one rejects a secret. Because Lucidos runs on the user's own machine, the public shape is the more natural fit wherever the provider offers it; leaving Client Secret blank in the credential modal selects it.

### OS surface
The notification surface outside the Lucidos UI: an OS-level notification banner. Two transports, chosen by client:
- **Web push** (browser / PWA): delivered by the device's push service (APNs on iOS, FCM on Chrome/Edge, Mozilla autopush on Firefox) and rendered by the registered service worker. Each push is required by the browser to result in a visible `showNotification()` call (`userVisibleOnly: true`) — silent pushes are penalised and can revoke the subscription.
- **Native desktop** (Tauri app): a native macOS notification driven by the *NativePushRequested* SSE, rendered + tap-routed by the app's `show_native_notification` command via Apple's `UserNotifications` framework (`UNUserNotificationCenter`). The embedded WKWebView can't subscribe to Web Push, so the engine reaches the desktop app over the open SSE stream instead. Requires a packaged `.app` build (inert in `tauri dev`).

Both ride the engine's single push-allowed decision (see PresenceCheck protocol), so a given notification reaches a device through exactly one transport and never collides with the *in-app surface* toast. Independent from the *in-app surface*.
See also: `system-knowhow/notifications.md` §§1, 3, 4.

### Parent thread
The direct ancestor of a *child thread*. Resolved via the child's `parent_thread_id` column. A thread can have at most one parent; a parent can have many children. The edge carries traffic both ways: each child reports its outcome upward when it terminates, and the parent can send a *child follow-up* downward to any child it spawned itself.

### Paused (thread status)
A *thread* whose turn the user's own *Switch to new version* interrupted, and which the engine has promised to resume. Its own status indicator, and the one that is not a dot: the standard pause glyph, in a neutral tone and deliberately not the red *failed* dot. The word "Paused" appears in the row's Info card, and the interrupted turn is labelled "Paused by restart" in the transcript. There is exactly one way in, and that is the point of the status: the engine auto-resumes the turn by itself, usually within seconds, so no **Continue** button is offered and nothing is being asked of you. Every OTHER interruption is *failed* instead, with the red dot, a place in the *attention* count, and the Continue button: a crash, an engine shutdown the user did not ask for, and a switch whose resume the next boot turned out not to be able to deliver (the boot says so explicitly, by replacing the pause with the error). So the pause glyph is a promise, and it is never shown for a turn nothing is coming back for. Paused is a *verdict* about the interrupted turn, not a resting state, so the events that merely close the turn out cannot walk it back to idle; sending a follow-up message clears it like any other new work. Distinct from *failed* as above, from `waiting` (a *change* is sitting in review) and from `waiting_for_user_answer` (the agent is parked on a question the user must answer). A paused thread does not count toward *attention*, because it resumes on its own.

### Plugin
A bundle of installable workspace content shipped as a single unit. Contains any of `apps/`, `knowhow/`, `triggers/`, `scripts/`, `auth-modules/` — mirroring the top-level `data/` directories. Defined by a *plugin manifest* at the root. At install time the contents merge into the target workspace's `data/`. Use a plugin when the pieces only make sense together (e.g. an app + its knowhow + its trigger); ship single files individually otherwise.
See also: `system-knowhow/plugins.md`.

### Plugin category
A topical tag on a *plugin* (e.g. `finance`, `health`, `developer-tools`) used to browse the *Plugins panel*'s catalog: it offers a filter per category and shows category chips on each card. A **controlled vocabulary**: an author tags a plugin in its *plugin manifest* (`categories = [...]`) from a fixed allowed set; a value outside the set is dropped and flagged (in the catalog scan's `errors`), never blocking install. Distinct from a plugin's *content* (the `apps`/`knowhow`/`triggers`/`scripts`/`auth-modules` kinds it ships, which the engine derives from the files, not the author). Allowed set + rationale in `system-knowhow/plugins.md`.

### Plugin manifest
The `manifest.toml` file at the root of a *plugin*. Declares `id`, `version`, `name`, `description`, optional topical `categories` (see *plugin category*), and optional install-time `setup` steps. Schema in `system-knowhow/plugins.md`.

### Plugin modified state
Whether a *plugin*'s shipped content has been locally edited since it was installed: the user (or the *Lucidos Agent*, or a *coding-agent thread*) changed an app, knowhow, script, or trigger the plugin owns. Surfaced as a **Modified** badge on the plugin's row in the *Plugins panel* (tooltip lists the changed paths) and as a warning when updating the plugin (the update overwrites local edits). **Derived, not stored**: the engine diffs the plugin's current `data/` content against its install commit (recorded in `PluginInstalled`), so the state self-heals when an edit is reverted and resets when the plugin is updated/reinstalled. An added file inside a plugin's app directory counts; a brand-new file in a shared root (`knowhow/`, `scripts/`, `auth-modules/`) is not attributed to any plugin. See `system-knowhow/plugins.md` § "Local modifications".

### Plugins panel
The top-level panel for discovering, installing, and managing *plugins* (including the *apps* they ship). It shows one unified list with an **Installed only** filter (a checkbox, **checked by default**): checked, the list shows every plugin on disk regardless of what it ships (app-bearing or not); unchecked, it widens to the whole marketplace catalog (installed + available — browse/install from registered *marketplaces*). A live search and a per-category filter (on the catalog) narrow the list and compose with the Installed-only filter. Each plugin shows as a card whose primary button progresses **Install** (or **Update**) → **Setup** → **Open**, with an **Uninstall** button once it is on disk; "Setup" appears while the plugin's *setup thread* is still running, "Open" launches its app once setup is done (or there was none). New installs/uninstalls route through the standard plugin confirmation panels, and the catalog re-scans whenever it is shown (the panel opens or **Installed only** is unchecked). The engine never silently updates installed marketplace plugins — when it finds a newer version it notifies the user, who applies the update from the card or the installed app's **Update** button (see *Marketplace*). When no marketplaces are registered yet, both the catalog empty state and the Settings → Marketplaces empty state offer a one-click **Add the official Lucidos marketplace** button (registering `github.com/lucidos-dev/plugins`); marketplaces are added/removed under Settings → Marketplaces. Distinct from the *Apps* panel, which lists the workspace's *apps*: an app that came from a plugin appears in Apps (open it) AND its plugin appears here (manage/update/uninstall it). The panel is also the home for plugins that ship no app — knowhow-, trigger-, script-, or auth-module-only bundles — and links to each shipped file. (History: it briefly had **Installed | Store** tabs, since replaced by the single Installed-only filter.) See ADR 0019.

### Preference
A single key→value user setting stored in the `preferences` table — theme, language, timezone, push notifications, the welcome message, chat model, UI scale, font, and so on. The bulk of what the user calls **Settings** (the umbrella also covers *models*, *credentials*, MCP servers, and *repositories*, which live in their own stores). A preference is **global** (workspace-wide) or **device-scoped** (a per-device override that wins over the global value on the device that set it). The *Lucidos Agent* reads/writes the agent-settable ones with `get_preferences` / `set_preference`; the human edits the same values in Settings. A write emits the persisted `PreferencesChanged` event (or `LanguageSet` / `TimezoneSet` for locale), which open pages live-apply. Distinct from *config* (the `data/config/` files like `apis.json`) and from a *credential* (a secret — never stored as a preference). See `system-knowhow/preferences.md`.

### PresenceCheck
The transient SSE event the *Lucidos Engine* broadcasts on every `NotificationCreated` to ask every connected page for its live presence. A **pure pong trigger** — it carries `notification_id`, `event_id` (so the pong can report `event_in_viewport`), a `deadline_ms` the page reads off the payload (set by `scheduler::push::DEADLINE_MS`, currently 2 s — sized to cover an iOS PWA's first packet after Tailscale wake-from-idle, where Tailscale's userspace WireGuard renegotiation pushes the round-trip into the 1100–1800 ms band), and `sent_at_ms`. It carries NO toast content: the in-app toast is driven separately by *NotificationToastRequested*, so it can no longer race the push decision. Each page answers with a *PresencePong*. The engine collects pongs up to the deadline and uses them to decide whether to send an *OS surface* push. Skipped entirely only when nobody is reachable — no page holds an open SSE connection AND no device has pinged visible within `PRESENCE_STALE_AFTER` (120 s, `core::device_presence`). The live SSE-connection count is the primary gate (`engine.sse_connections`); the heartbeat candidates are secondary (`expected_pong_count` in `scheduler::push`). The SSE count is what makes this robust — iOS suspends the 30 s heartbeat while a PWA is foregrounded, so the heartbeat row goes stale even though the page is connected and would pong; gating on the open connection lets the active page still suppress the push. See `system-knowhow/notifications.md` §3.

### PresencePong
The page's response to a *PresenceCheck*. POSTed to `/api/v1/presence-pong` with `notification_id`, `device_id`, `is_active`, `focused_thread_id`, `event_in_viewport`. The engine's decision: an OS push goes out iff NO pong reports `is_active`; multi-tab pongs on the same device OR within the device. Late pongs (after the deadline) ack 200 and are dropped — the race is normal. See `system-knowhow/notifications.md` §3.

### NotificationToastRequested
The transient SSE event the *Lucidos Engine* emits to drive the *in-app surface* toast. Emitted from the `NotificationCreated` fan-out **only on the push-suppressed branch** — i.e. when the *PresenceCheck* pongs say an *active device* exists, so the *OS surface* push is withheld. Carries the toast content (`title`, `body`, `thread_id`, `event_id`, `app_id`, `tap`, `sent_at_ms`) so the page renders without a re-fetch; active pages render the toast (or auto-read when looking at the *source event*), hidden pages ignore it. Because it and the OS push hang off opposite branches of one decision, a device never receives both for one notification — the in-app toast and the OS push are mutually exclusive by construction, not by a page-side timing race. See `system-knowhow/notifications.md` §4.

### NativePushRequested
The transient SSE event the *Lucidos Engine* emits to drive the native-desktop *OS surface*. The mutually-exclusive complement of *NotificationToastRequested*: emitted from the `NotificationCreated` fan-out **only on the push-ALLOWED branch** (no *active device*). Same payload shape as the toast event. A connected Tauri desktop app renders a native macOS notification from it via its `show_native_notification` command, using Apple's `UserNotifications` framework (`UNUserNotificationCenter`) with a delegate that captures the click to route the tap — the embedded WKWebView can't receive Web Push, so this is the desktop counterpart of the web-push fan-out, delivered over the open SSE stream. Native delivery needs a packaged `.app` build (inert in `tauri dev`). Browser / PWA pages ignore it (the handler gates on Tauri) and receive the real web push on the same branch. See `system-knowhow/notifications.md` §§1, 4.

### Scratch
Ephemeral working files under `.lucidos/tmp/`, at the *workspace* root and so **outside** `data/`: gitignored, not indexed, not counted as *artifact*s, safe to delete at any time. Where `http_request(temp_path)` saves a raw response, where `git_clone` puts an inspect-only checkout, and where plugin archives are staged during install. The file tools **read** it (`read_file`, and `copy_file` as a source, which is how you promote a file out of scratch into `artifacts/imported/<name>/`) but never **write** it: they git-commit everything they write, so `write_file` / `edit_file` / `delete_file` refuse a scratch path and point at `run_python`, whose cwd is the workspace root. Only `.lucidos/tmp/` is addressable this way; the rest of `.lucidos/` (coding-agent *worktree*s, `exhaust/`, `engine.pid`) is engine runtime state and is refused in both directions. The ephemeral counterpart to an *artifact*.
See also: `system-knowhow/best-practices.md` rules 8 and 10; ADR 0051.

### Script
Code (Python, shell, JS) invoked by an *intent* or *knowhow*. Lives with its primary consumer when scoped (`data/apps/<id>/scripts/`, `data/triggers/<slug>/scripts/`, `data/knowhow/<domain>/scripts/`) — or at top level (`data/scripts/`) when shared across multiple consumers (mirroring how knowhow can be standalone or app-scoped).

### Setup interview
A guided interview the *Lucidos Agent* runs to work out what this particular person should use Lucidos for, ending with *app*s, *trigger*s and *knowhow* actually built in their *workspace* in that session. Deliberately not work-only: its first question asks which parts of the person's life to cover (work, home and personal admin, health and training, learning and side projects) and takes more than one answer, so a kit can be built around training or a household just as readily as around a job. Started from the "Help me get the most out of Lucidos" button on the first-run welcome, or at any later point from the "Setup guide" row in the *Lucidos menu* (the menu the Lucidos mark opens, on every viewport; that row is the one surface whose label says "guide" rather than "interview", which reads as an interrogation to someone who does not yet know what the row does), or from the help button beside New thread on the desktop header. Both of those later entry points confirm first, since they send. There is no such header *button* on mobile, where a header row has no slot to spare for a once-or-twice action, but the menu row costs no row space and is the durable mobile route; mobile can also start it from the welcome or by asking. Every entry point sends the same ordinary message, so it is always reachable by typing. Driven by `system-knowhow/setup-interview`, which owns which areas to ask about, the question ladder, which cards allow more than one answer, the mapping from answers to a kit, the confirm-before-building rule, and what gets persisted: the interview record at `artifacts/setup-interview.md` (appended per run, never overwritten) plus a `SetupInterviewCompleted` *domain event*. Only facts the user actually stated reach memory or `user_profile.md`; everything the agent merely concluded stays in the artifact. Sibling to the two other workspace-wide recipes and distinct from both: *workspace audit* asks whether the workspace matches current conventions, *workspace learning* asks whether the conventions match this user, and the setup interview asks whether the workspace matches the person. It is the only one of the three that needs the user present, because it asks and then builds rather than sweeping and proposing. Distinct from a *setup thread*, which finishes installing one *plugin*.

### Setup thread
A *Lucidos Agent* *thread* the engine spawns when a *plugin* that shipped a `setup` field is installed (from the *Plugins panel* or the `install_plugin` tool). On an **update** it spawns only when the new version's `setup` differs from the installed version's — an unchanged version bump re-runs nothing. Its first message is just a short user-facing line (`Set up the newly installed <name> plugin.`); the "how to run a plugin setup" guidance lives in `system-knowhow/plugin-setup`, which the agent loads to plan the steps as a todo list and to find the author's setup instructions (referenced from the `PluginInstalled` event, not embedded in the thread). It then walks the user through completing them — asking for credentials/choices and doing the wiring it can. The user is navigated straight into it on install. Its id is recorded in the `PluginInstalled` event so the plugin's card *Setup→Open* button can reopen it; the card treats setup as done once the thread is no longer `running` or `waiting_for_user_answer`, and also once the thread is gone entirely (no summary row and no live *Thread Queue* entry), so a lost or stale setup thread degrades to *Open* rather than a *Setup* button that errors. The engine's background marketplace update check never installs (it only notifies), so it never spawns a setup thread — those come only from a user-confirmed install.

### Side-effect grant
The set of irreversible side-effect categories a *trigger* is pre-authorized to perform unattended (set per-trigger in the trigger's settings under "Allowed side-effects"). A trigger fires with no human present, so the *command guard* can't pause to ask it — it consults the grant instead: an irreversible command (sending email, a mutating HTTP request, a cloud-CLI change, out-of-workspace destruction, or anything else irreversible) runs only if its category is in the grant; otherwise the command is blocked and the trigger run fails (a failure notification surfaces the missing grant). Categories: **email**, **external API**, **cloud CLI**, **out-of-workspace destruction**, **other**. The default is empty (no irreversible side-effects allowed). Only consulted when the *command guard* is on; chat turns ignore the grant entirely (they're asked every time). The grant is set by the user, not by the agent — `create_trigger`/`update_trigger` (the LLM tools) can't set it, so an agent can't widen its own unattended authority. The same grant **also governs *coding-agent thread*s a trigger spawns** (Claude Code / Codex), where it's inherited down the spawn tree: such a thread runs unattended, so the engine resolves the coding agent's permission cards from the root trigger's grant instead of hanging — benign in-workspace work is auto-allowed, a granted irreversible category is auto-allowed, an ungranted one (or a catastrophic command) is auto-denied. Unlike the chat command guard, this denies the single request rather than failing the whole run, and applies regardless of the command-guard toggle. See `coding-agent-events.md` § "Unattended auto-resolution".

### Signer manifest
The `<name>.manifest.json` sidecar next to a `<name>.wasm` signer artifact in `data/auth-modules/`. Carries WASM-host metadata (`secret_handles`, `body_mode`, `capabilities`). The engine never auto-loads provider config from it — `data/config/apis.json` is the single source of truth for proxy entries.

### Source event
The specific *event* a notification points to, stored as `notifications.event_id`. Used by the *in-app surface* to decide whether the user is currently looking at the very thing the notification is about: if the page is on the source event's thread AND the source event is in viewport, the notification is auto-marked-read with no toast and no badge increment. A notification with `tap = { kind: 'navigate', to: { target: 'thread', id: '...', event_id: '...' } }` also lands on the source event (scroll + pulse). Both uses resolve the event the same way, and they resolve it whether it starts a turn or is folded into one as a step: an event rendered as a step is addressed by its own card (a failed response by its failure card), not by the turn around it. A source event that never renders is reported rather than silently ignored, and the transcript is left exactly where it was.
See also: `system-knowhow/notifications.md` §§2, 4.

### Spawning thread
The *thread* that issued the `run_thread` / `run_coding_agent` / `lucidos spawn-thread` call. For `relation: "child"`, the spawning thread IS the parent. For `relation: "top"`, there's a spawning thread but no parent and no callback wiring. Either way the spawn is *attributed*: the spawned thread's first message records which thread launched it, so its route popover names and links back here. Attribution is not linkage, so it never makes a top-thread report back, count as a child, or inherit the spawning thread's permissions.

### Status filter
The `--status` flag on `lucidos threads list` / `count`, and the matching `status` parameter on `GET /api/v1/threads/{list,count}`, the `threads` tool and `lucidos.threads`. Names exactly the *thread* statuses to keep, out of `idle`, `running`, `waiting`, `waiting_for_user_answer`, `paused`, `failed`: the same values every returned *thread summary* carries in its `status` field, so a caller filters on what it reads. The precise form of the *Active (thread state)* union. `status=running` is "is the workspace busy?", `status=waiting_for_user_answer` is "is anything waiting on me?", and `active=true` is both at once. Passing `status` together with `active` is refused rather than intersected, as is an unrecognized or empty value.
See also: `system-knowhow/lucidos-cli.md` § `lucidos threads list`.

### Style override
One entry in the `style_overrides` *preference*: a CSS custom property name and the value to paint it with, applied straight onto the app's root element. Writing one repaints every connected client live, over the same `PreferencesChanged` fan-out that carries theme, font and UI scale, so a design value can be retuned on a running Lucidos with no rebuild. Device-scoped like the other appearance preferences, so tuning on a phone leaves a desktop alone. Values only: an override can retune a colour, a size, a duration, never move a control or change what a screen does. Two ways back out if a value makes the UI unusable, **Settings → Appearance → Style overrides → Clear all**, and `?style-reset` on the URL, which clears them before the first pixel is painted and so works when nothing on screen is readable.

### Style remote
The app that writes *style overrides*: sliders and colour pickers over the design tokens, one knob per custom property. Being an *app* it is workspace data rather than product code, so its knob list is edited in place and never goes through *Apply*.

### Sub-thread
Any descendant in the *thread* tree (transitive). A *child thread* is a sub-thread; a grandchild is a sub-thread. Use *child thread* when you mean the direct relationship; use *sub-thread* when depth is irrelevant or the relationship is transitive.

### Thread
A single conversation — a stream of events sharing one `aggregate_id`. Every chat reply, trigger run, and *coding-agent thread* run is a thread. Threads have a persisted `source` (`chat` / `trigger` / `claude_code` today — values are *channel* identifiers; see dev glossary), while user-facing/API source filters call the coding-agent bucket `coding-agent` and accept legacy `claude_code`. Threads also have a compose state (`composing` / `active` / `discarded` on the compose side; running / idled / failed on the runtime side), an archive flag (`inbox` / `archived`, orthogonal to compose state — an archived thread keeps `state='active'` and only flips `archive_state`), and may spawn other threads.

### Event wait
The internal name for a **thread subscription**: a *thread* asking to be woken when something happens, instead of checking over and over. The word survives because it is on disk, in the persisted `EventWait*` events and in the `await_event` tool's own name, so the code and the event log say *event wait* where this glossary says *thread subscription*. They are the same thing.

The agent says what it is waiting for (an *event subscription*, optionally filtered), why, and for how long, then finishes its turn. The thread holds nothing and blocks nothing while it watches, and Lucidos re-opens it the moment a matching *event* arrives, or tells it the wait timed out. The *subscription indicator* shows what it is watching and how long is left. Available to the *Lucidos Agent* and to a *coding agent* alike, and each can also list what it is watching and stop watching (`list_event_waits` / `cancel_event_wait`, or `lucidos event-waits list` / `cancel`).

A watching thread reads as **Waiting**, the same status a thread waiting on its *sub-threads* shows, because it means the same thing: this is not finished, and something else will wake it. You do not have to open the thread to see it, and it is still there after a reload.

Two things separate it from a *trigger*, and the first is the one people skip. **Where the answer goes:** a trigger runs in its own thread and reaches you as a *notification*; an event wait resumes the conversation you are already reading, so the report lands in it. **How long it lasts:** a trigger is a standing rule that starts a NEW thread every time, indefinitely; an event wait resolves on the first match and the agent re-arms it per event, bounded by a cap on consecutive parks. So "tell me **here** when a change is proposed" is an event wait even though it sounds like a standing rule, while "notify me whenever a change is proposed, from now on" is a trigger. Both are often right at once: watch here now, and add a trigger if it should keep running after this conversation is done.

**Neither a message nor Stop ends it.** Sending a message to a watching thread runs an ordinary turn and leaves every subscription exactly as it was, and **Stop** ends the running turn and nothing else. A wait that actually *fires* is the opposite: it is used up, and the agent has to subscribe again to catch the next one.

**Four things do end one, and each says so.** **Stop waiting** in the *subscription indicator*, archiving the thread (which asks first, naming every subscription the archive would stop, its *sub-threads* included), discarding it, and the agent standing it down when you tell it to. Each leaves a line in the conversation saying what stopped and how, so a watch can never end in silence.
See also: *event subscription*, *trigger*, *subscription indicator*, `system-knowhow/thread-events.md`.

### Subscription indicator
The control on the prompt bar showing what the open *thread* is currently waiting for. It appears only when the thread holds at least one live *thread subscription* (an *event wait*), and lists each one with the agent's reason, the event it is watching, a countdown to its deadline, and a **Stop waiting** button. It is the answer to "is this thread stuck, or is it asleep on purpose?", readable at any time without scrolling back through the conversation. The thread's **Waiting** status says *that* it is watching, on every list it appears in; this says *what for*, on the one thread you have open.

Its header is the unqualified **SUBSCRIPTIONS**, and that is exact rather than loose: a *trigger subscription* belongs to a trigger and never appears on a thread screen, so the only species that can be listed here is the thread one.

### Thread drawer
The pane listing your *threads*: the Pinned, *Current* and Archive sections, with the attention badge and the thread filter. That badge has a second home: the same needs-attention count rides the **thread-drawer toggle** whenever the list itself is hidden (the drawer closed on desktop; any pane other than the threads pane on mobile), so a *thread* waiting on you stays visible from the conversation. Exactly one of the two shows it at a time. On mobile the toggle is the leading control of the *thread pane* header and takes you to the threads pane, with the hamburger **menu drawer** mirrored at that header's trailing edge (it slides out from the right, the edge its button sits on). First of the three panes; one of the two making up the *Conversation* side, the other being the *thread pane*. CSS container `.thread-drawer` (`FocusedPane = 'drawer'`; on mobile the leftmost swipe pane, `MobileView = 'threads'`). Always say *thread drawer*, never a bare "drawer". The hamburger **menu drawer** (Files / Apps / Plugins / Triggers plus pinned *apps*, `Drawer.tsx` / `drawerOpen`) is a different surface.
See also: *Conversation*, *thread pane*, *content pane*, *Current section*.

### Thread pane
The pane showing the open *thread*'s transcript plus the prompt input — where you read what the *Lucidos Agent* did and type the next message. Second of the three panes; the other half of the *Conversation* side, alongside the *thread drawer*. CSS container `.pane-thread` (`FocusedPane = 'thread'`; on mobile the middle swipe pane, `MobileView = 'thread'`).
See also: *Conversation*, *thread drawer*, *content pane*.

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
A spawn with `relation: "top"` (the CLI default for `lucidos spawn-thread`). Has no parent and no callback wiring, so it appears in the main thread list as an independent top-level thread. The *spawning thread* is **not** resumed when it finishes. It still records WHO launched it: the route popover on its first message names the spawning thread and links to it. "No parent" is about the callback, not about provenance.

<!--gloss-trigger-start-->
### Trigger
A workspace configuration that fires either on a schedule (`run.cron`) or on one of its *event subscriptions* (`on`). The `run` is one of two shapes:
- `run.type: "intent"` — spawns a *trigger thread* whose LLM is given `run.intent` (the user's voice — non-technical prose) as a user message and discovers the knowhow it needs via `load_knowhow` at fire time. No per-trigger knowhow allowlist.
- `run.type: "script"` — executes the *script* at `run.path` directly, no LLM. The engine sets `TRIGGER_EVENT_TYPE` / `TRIGGER_EVENT_PAYLOAD` / `TRIGGER_EVENT_ID` / `TRIGGER_EVENT_THREAD_ID` env vars on event fires so the script can branch deterministically and deep-link any notification back to the originating event. Right when the work is a deterministic transformation that doesn't need LLM judgement.

Lifecycle (both shapes): defined by `TriggerCreated`; each firing emits `TriggerStarted` then `TriggerCompleted`. The trigger's panel row surfaces its **last-run status** (OK / failed — the most recent firing's outcome) beside the last-run time; there is no built-in run-history view. Deeper run detail comes from the trigger's own *event* stream — ask the *Lucidos Agent* ("what has this trigger been finding?") or build an *app* on the events.
The row also shows the **next runs**: the next few fire times, merged across every cron expression. A cron that can *never* fire (`0 0 9 31 2 *`, Feb 31) is rejected at create and update, with an error naming the offending fields. A trigger stored before that guard existed keeps loading and wears a **schedule error** instead of the "No more runs" a spent one-shot earns. Both have no next run, but one never worked and the other finished its job.
See also: `system-knowhow/triggers.md`, `docs/taxonomy.md` § Triggers.
<!--gloss-trigger-end-->

### Off-schedule run
A firing of an existing *trigger* asked for by a person rather than by its schedule or an *event subscription*: `triggers(action="run")`, `lucidos triggers run --id`, `lucidos.triggers.run(id)`, or the **Run once** button on the trigger's panel row. It is deliberately **indistinguishable downstream** from a scheduled fire: same `TriggerExecuted` / `TriggerCompleted`, same `last_run` and last-run status, same *trigger thread*, `go_to_review` routing and *side-effect grant*, and no actor stamp. Nothing has to learn a third kind of run, and a manual run correctly suppresses a redundant catch-up of the slot it covered.

Distinct from the **Run now** button in the *Thread Queue* panel, which force-admits an entry that is *already queued* and cannot create a fire. Refused when the trigger is paused (resuming restores the schedule but runs nothing by itself) and when the trigger has no cron schedule (emit its subscribed event instead, which is the faithful reproduction for an event-driven one). Reported rather than started when a fire of the same trigger is already active or queued, because cron fires coalesce to at most one pending run per trigger.
See also: `system-knowhow/triggers.md` § "Running an existing trigger once, off-schedule".

### Trigger definition
The on-disk `trigger.toml` at `data/triggers/<slug>/trigger.toml`, a **derived read-model** of a *trigger*'s durable config, maintained by the engine from the trigger events (written on create/update, removed on delete, rebuilt from events on boot). NOT the source of truth (events are, and the scheduler never reads the file) and **not version-controlled** (the engine adds it to the repo's local `.git/info/exclude`); a hand-edit changes nothing that fires and is overwritten. It exists so a trigger is inspectable and so a *plugin* can SHIP a trigger by declaring one (plugin install parses the declaration into a `TriggerCreated`). See ADR 0019, `system-knowhow/triggers.md` § "On-disk trigger definition".

### Event subscription
A standing request to be told when a matching *event* happens: an `event_type` plus an optional payload `condition` scoped to that event. One shape, one matcher, and two species. A subscription with no condition matches every event of its type; with one, only the events whose payload satisfies it.

A **trigger subscription** is one entry in a *trigger*'s `on` list. On a match it **spawns a new thread** and **stays armed** for the next one. That is what makes a trigger a standing rule: it outlives every thread it starts, and it keeps firing until you pause or delete it. A trigger may carry several, and each entry's filter constrains only its own event, so different payload shapes never interfere.

A **thread subscription** is one an existing *thread* armed for itself, with `await_event` (the *Lucidos Agent*) or `lucidos await-event` (a *coding agent*). On a match it **resumes that thread**, and it is **spent**: the first match uses it up, so watching for the next one means arming another. Its internal name is *event wait*, which is the word the code and the event log use.

Both are waiting for events from their subscriptions, and the same matcher decides both, so a `condition` that fires for one fires for the other. What differs is who consumes the match and what it costs them. On a thread screen only the thread species can appear, which is why the *subscription indicator* needs no qualifier.
See also: *trigger*, *event wait*, *subscription indicator*, `system-knowhow/triggers.md` § "One trigger, multiple events".

### Trigger thread
A *thread* spawned by a *trigger* firing. Distinguished by `source = 'trigger'`. The LLM driving it has the same knowhow access as a chat thread: the system prompt advertises the intent registry, and the LLM calls `load_knowhow` when it judges a recipe relevant. No per-trigger knowhow allowlist. Terminal event: `TriggerCompleted`.

### Trigger group
A user-visible folder that organizes *triggers* in the triggers panel. Pure label: belongs to no agent, has no schedule, runs no code. Each trigger may belong to at most one group via `group_id`; ungrouped triggers render under an implicit "Ungrouped" section. Useful for surfacing emergent workflows — chains of triggers connected by `emit_event` → `on_event` — as a single group in the panel, without changing how they fire. Lifecycle: `TriggerGroupCreated`, `TriggerGroupRenamed`, `TriggerGroupReordered`, `TriggerGroupDeleted`. Groups with at least one member cannot be deleted — the LLM (or user) must reassign or delete the triggers first. Panel order is governed by `order: i32`, ascending.

### Urgent follow-up
A *child follow-up* the parent marked `urgent: true`, which stops the child's current turn so it reads the message now instead of at the child's next natural break. The default is the opposite, and deliberately so: an ordinary follow-up queues, because a steer should never throw away an in-flight build. The cost of urgency is exactly that, whatever the interrupted turn was mid-way through is lost, so it is for the messages that cannot wait (a cancellation, a "stop, you are working from a wrong assumption") and not for hurry. What it buys is unbounded otherwise: a child inside a long tool call reads a queued message only when that call returns, and a *coding-agent thread* parked in a ten-minute blocking wait really does sit on a STOP for ten minutes. The interrupted turn ends as "Superseded", not "Canceled": the work is being steered rather than abandoned, so the parent is not woken with a false completion card for a child that carries straight on in the next turn. Two caveats. A child parked on a question is blocked on a *human*, not on work, so urgency cannot unblock it. And on a **Codex** child it changes nothing, because a Codex turn cannot read a queued message at all until it ends, so every follow-up there already stops the current turn. Ask for it by what you mean, not by which backend the child runs.
See also: *child follow-up*, *coding agent*.

### Wake question
An `ask_user_question` called with exactly one option. The *Lucidos Agent* uses it to step aside and let the user tap a single suggested follow-up — like "Show results" or "Stop sweep" — instead of typing a wake message. The agent's context lives in the `question` text; the option's label is the user-perspective prompt the tap effectively sends. The thread shows the "?" attention status (`WaitingForUserAnswer`) until the user taps or sends a free-form override (which auto-resolves the pending question).
See also: `system-knowhow/running-python.md` § The drain pattern.

### Workspace
A user's complete Lucidos instance: one PostgreSQL database inside the shared Lucidos Postgres cluster + `data/` directory (artifacts, apps, knowhow, triggers, intents, config, auth-modules, scripts). Multiple workspaces run concurrently (each is its own isolated engine + database), fronted by a single *workspace gateway* (dev term) that addresses each one by its *workspace address*, the path prefix `/<slug>/`. From the workspace picker (at `/~/`, or just `/` when there's more than one) you switch between them, create / rename / delete them, toggle each one's *auto-start*, and **restore one from a backup** (drop in an encrypted `.enc` backup file + its backup key; the name is filled in from the backup, and you must change it when its address is already taken). Every workspace you've launched stays **listed** in the picker even after it stops; opening a stopped one starts it on demand. On first run there are no workspaces yet, so the picker offers both ways in side by side: name your first workspace (suggesting "personal" or "work"), or restore one from a backup. Nothing is auto-created for you.

### Workspace address
The path a *workspace* is served at (`/personal/`), also the name of its folder and of its database. It is derived from the workspace's name when the workspace is created (lower-cased, with anything that is not a letter or digit turned into `-`) and then **fixed forever**: renaming a workspace changes its label, never its address, so the two can end up different. The picker shows a workspace's address only when it would otherwise surprise you, which is exactly when a rename has moved the label off the address, or when two workspaces share a label and the address is the only thing telling them apart. **No two workspaces can share an address, and no two can share a name either**: creating or renaming to a name another workspace already has is refused, naming the one that has it (a name a running restore is about to give its workspace counts as taken too, and you can wait for it instead) (matched ignoring case and surrounding spaces, since "Work" and "work" are no more tellable apart in a list than two identical names). Where only the *address* is taken, because the workspace holding it goes by a different name now, a create simply gets the next free address, `/personal-2/`, and the picker says so before you create it; a restore is refused instead, since restoring into a suffixed address would quietly leave you with a second copy of the same workspace. Workspaces that already shared a name before this rule keep working untouched, and the picker shows their addresses so you can still tell them apart.

### Auto-start
A per-workspace toggle (set in the workspace picker) controlling whether the *workspace gateway* brings that workspace's engine up automatically when the gateway starts. **On** = always-on: the workspace is spawned on every gateway (re)start (a packaged install's login-launched gateway brings up its auto-start workspaces). **Off** (the default for a newly-created workspace; turned on only via this toggle) = the workspace is still listed in the picker but its engine starts only when you explicitly open or launch it. An already-running workspace is re-adopted across a gateway restart regardless of this setting.

## Advanced — coding agents

These terms describe the surface for users running coding-agent workflows: Claude Code and Codex (the two coding agents), the Apply / Discard flow on Lucidos's own repo, the hardening gate, and the external-repo variant for working on user-added repositories. Most users never encounter these; chat and triggers cover the rest.

### Apply
The user-clicked action that merges a *coding-agent thread*'s worktree branch into `main`. **Always non-disruptive** — it never restarts the engine on click. The button reads **Apply** for a change that needs no restart, and **Apply\*** (a compact asterisk marker, explained by the button tooltip) when the change is engine-affecting; the former "Apply & Restart" dual label — which implied the restart happens on click — stays retired, because even for an **Apply\*** the restart is the separate *Switch to new version*, never Apply itself. When a merged *change* touches engine-affecting source, Apply additionally kicks off a background engine rebuild (dev only) that later surfaces as *New version available / Switch to new version*. If the session didn't run *hardening* first, Apply runs it synchronously and the user waits. Source: `crates/lucidos-engine/src/engine/git_ops.rs` (`files_require_restart`).

### New version available / Switch to new version
The affordance for moving the *engine* onto a newer version. Lucidos surfaces **"New version available"** (a dismissible toast + a persistent badge in the control panel) and offers **"Switch to new version"** — a deliberate, user-triggered restart onto the new version, behind a brief blocking overlay + "Starting new version…" notice. One affordance, two sources: in a **dev** build, *Apply*-ing a *change* to Lucidos's own source auto-rebuilds the engine binary in the background (the old engine keeps serving until you switch — no "Aborted" during the build); in a **packaged** build, the app updater detects a newer release. The packaged path additionally **narrates itself**: because it downloads and swaps a whole signed bundle before restarting the stack, it reports a named *update phase* the whole way — *Checking for updates* → *Downloading* (with bytes transferred, and a progress bar whenever the server declares a size) → *Verifying* → *Installing* → *Restarting background services* → *Relaunching* — in the toast and in **Settings → System**, which always agree. The download can be **cancelled** (nothing has been written to disk yet, and the update stays on offer); from *Installing* onward there is no half-installed state to return to, so the affordance is withheld rather than offered dishonestly. A failure names its reason instead of leaving a spinner. In-flight threads auto-resume after a user-initiated switch — *coding-agent threads* pick up where they left off, and chat / trigger threads re-enter with a note summarising what the interrupted run already did, so neither needs a manual *Continue* click; a thread parked awaiting an *AskUserQuestion* answer is preserved instead (answering it resumes). A restart that was **not** a deliberate switch (a crash) never auto-resumes — those threads keep the manual *Continue* button so work that may have crashed the engine can't loop. The engine toast **and its persistent control-panel badge are ONE signal** (keyed on the on-disk binary build id): both appear only once the rebuild has actually produced **a newer binary to switch onto** — never at *Apply* time, when the build has only just started, and never for a build that finished without producing anything newer. "Newer" is literal: a binary that merely *differs* from the running one is not offered if it is provably **older** (co-located dev workspaces share one build output, so an older binary can land there — switching onto it would be a downgrade). Dismissing the toast only **defers** it — the badge stays lit as the persistent switch affordance, and a genuinely newer on-disk build re-surfaces the toast. Distinct from the client-bundle **"New version available — Refresh"**, which reloads the frontend page, not the engine: its badge + toast are **likewise one signal** with the same defer-on-dismiss behavior (the badge persists while the loaded bundle is stale), and in dev the engine only ever *serves* a client compatible with the running engine — a boot-pinned `dist/` snapshot — so a reload never loads a newer client against an older engine. A **frontend-only** *Apply* leaves the engine binary unchanged, so the engine re-snapshots and advances the served client **in-process** (no switch needed), surfacing this Refresh affordance; a **mixed** (engine + frontend) change advances the client only together with the engine, on a switch. Source: `crates/lucidos-engine/src/engine/engine_version.rs`, `crates/lucidos-engine/src/api/frontend_snapshot.rs`, `crates/lucidos-engine/src/engine/frontend_refresh.rs`, `crates/lucidos-app/src/store/actions/engine-update.ts`, `client-update.ts`.

One packaged failure is reported differently from the rest, and it is worth knowing on sight: if the swap leaves no runnable app on disk, Lucidos does **not** restart anything. It says so, and tells you to reinstall from the `.dmg`, because retrying an update has nothing left to install over. The background service keeps running the version it already loaded, so your workspaces stay up until you reboot.

### Apply All
The user-clicked action that triggers an *Apply* on every pending *change* in one batch. UI button label: **Apply All** (sibling to per-row *Apply* / Discard on the changes panel). The batch skips exactly what a per-row *Apply* refuses — changes whose thread is still working, and changes with no file changes left — so the bulk path can't do what the button won't. Discard All skips neither. Engine emits `ApplyAllBatchStarted` with the full change-id list + actor, then advances the batch as each member's `ChangeApplied` / `ChangeApplyFailed` event lands, and emits `ApplyAllBatchCompleted` with `applied: Vec<Uuid>` + `failed: Vec<ApplyFailure>` when every member has resolved. Member status is first-write-wins — one failure does not abandon the rest of the batch. Each member individually goes through the same *hardening* and restart-derivation rules as a single *Apply*. Persisted under aggregate `apply_all_batch`, `aggregate_id` = `batch_id` (UUID). While the batch runs, a sticky toast with a spinner shows progress and offers **Cancel** (`POST /api/v1/changes/apply-all/cancel`): the engine stops advancing to further members, interrupts the in-flight *hardening*/merge session, and marks the remaining members `failed` with "Apply All canceled" so the batch resolves and `ApplyAllBatchCompleted` still fires. Already-applied members stay applied; the rest return to pending (best-effort for an in-progress merge that already landed). A single *Apply* that woke a *hardening* or merge session can likewise be canceled from its *coding-agent thread* (the thread's Cancel button).

### Cancel (Stop)
The user-clicked **Stop** action on a working *coding-agent thread*. Behaves like pressing **Esc** in the *Claude Code* CLI: it *interrupts* the current turn but keeps the session resumable — the same `cc_session_id` and branch are preserved, so the next message continues the *same* conversation (a `--resume`) with full context. It is NOT a kill and NOT a fresh start. Emits `ResponseCanceled` (the visible "Canceled" chip) + `CodingAgentIdled` (the resume anchor). Distinct from *Apply* / Discard / Archive, which terminate the turn via their own lifecycle event. Routed through `interrupt_agent` (`POST /api/v1/claude-code/stop`, default `StopReason::UserStop`); a bounded fallback hard-stops only if the agent fails to honor the interrupt. Source: `crates/lucidos-engine/src/engine/claude_code/control.rs`, `agent_session/lifecycle.rs` (`SessionEndAction::KeepCanceledBranch`).

### Change
A *coding-agent*-proposed set of file edits shown as a pending branch in the UI. Resolved by *Apply* (non-disruptive merge into main; an engine-affecting change then surfaces *New version available / Switch to new version*) or Discard. Lifecycle events: `ChangeProposed`, `ChangeApplied`, `ChangeDiscarded`. Stored as a row in the `changes` table. Internal (Lucidos-repo) coding-agent threads produce changes; *external-repo coding-agent threads* skip this flow.

A change's file list tracks git: when later commits on the branch cancel the diff out (a commit plus its revert), the engine re-syncs the row to **zero files** and the card reads "No file changes" instead of claiming edits its Diff can't show. Such a change stays pending — the engine never resolves a change on the user's behalf — but *Apply* is refused (there is nothing to merge, and it would only add no-op commits); **Discard** is the resolution. The re-sync runs when the coding agent next idles, when its session ends, and as an engine-startup sweep for rows that went stale while nothing was running.

### Claude Code
Anthropic's coding-agent CLI; the default *coding agent* product Lucidos integrates (the other is *Codex*). Often abbreviated **CC**. Modeled in code as `CodingAgent::ClaudeCode` (enum, wire value `"claude-code"`). The thread channel value `"claude_code"` is historical and shared by every coding-agent thread regardless of backend — it means "coding-agent channel", not "this thread runs Claude Code"; the per-thread backend lives in the `coding_agent` column / event field.

### Codex
OpenAI's coding-agent CLI; the second *coding agent* product Lucidos integrates. Modeled in code as `CodingAgent::Codex` (enum, wire value `"codex"`). Picked per thread via the coding-agent chip on the *compose destination* picker (default: *Claude Code*, remembered per workspace via the `coding_agent_default` preference); the choice is locked at the thread's first message — an existing thread can never switch backends. Codex sessions run inside an OS sandbox scoped to the thread's *worktree*, plus two deliberate extras: the workspace's `data/` tree (so `lucidos data write` works) and the worktree's shared git dir (so `git commit` works). Nothing else in the *workspace* is writable — not `.lucidos/`, not a sibling worktree. User questions work the same as for Claude Code (Codex asks via the `ask_user_question` tool and the answer renders as the usual question card); permission cards appear when a Codex command or file change needs to escalate past the sandbox (default protocol — the `exec` escape-hatch protocol instead runs non-interactively with the sandbox as the only guard). The Apply / Discard flow, *changes*, and *hardening* work the same as for Claude Code.

### Coding agent
Role: a subprocess driving a *thread* to make code changes inside an isolated git *worktree* (dev). Lucidos integrates two coding agents: *Claude Code* (default) and *Codex*. Modeled in code as `CodingAgent` (enum). The thread it drives is a *coding-agent thread*; which agent drives it is chosen at the thread's first message and locked thereafter.

### Coding-agent branch
The git branch a *coding-agent thread* does its work on, named after the thread so `git branch -a` reads as a list of work: `lucidos-<coding-agent>-<app|repo>-<name>-<slug>`, e.g. `lucidos-claude-code-repo-lucidos-fix-auth-timeout`, `lucidos-claude-code-app-habit-tracker-add-streaks`, `lucidos-codex-repo-example-repo-fix-auth`. The `lucidos-` prefix marks it as one Lucidos created, which matters most in an *external repo* where it sits among your own branches. Two threads with the same name get `-2`, `-3`. The name is fixed when the thread's branch is created, so renaming the thread afterwards does not move it, and a thread you continue keeps the branch its work is on. *Apply* merges this branch into `main`. Branches from before this naming (`claude-code/…`) keep their old names and keep working.

### Coding-agent permission card
The approval card a *coding-agent thread* shows when its agent wants to do something the engine won't wave through: Deny, Allow once, Allow for this thread, or Always allow. Until answered, the thread waits on the user. Most of the agent's work never reaches a card — anything it writes **inside its own worktree** is allowed automatically, because that worktree is disposable and you review every change in the diff before you Apply it. A card appears for a shell command the agent's own gate escalates, a write **outside** the worktree (somewhere else on your machine), or a write into the worktree's hidden `.git` folder — the one in-worktree place whose contents don't show up in the diff you review. **"Allow for this thread" is remembered for the life of that thread**, including across an Apply that restarts Lucidos; "Always allow" is remembered for every future thread, in an editable list under **Settings → Permissions**. A *trigger* fires unattended, so it never shows this card — see *side-effect grant*.

### Coding-agent thread
A *thread* driven by a *coding agent* (Claude Code or Codex) inside an isolated git worktree. Distinguished by `is_coding_agent = true` on `thread_summaries` — set when a `SessionStarted` opens an agent session on the thread (or by another event on the `claude_code` channel), never by a *resume boundary* alone: `ContinuationStarted` fires on chat and trigger threads too, so it confers no thread type. The persisted `source` value is `"claude_code"` for every coding-agent thread (historical channel name, backend-agnostic); public source filters should use `coding-agent`, with `claude_code` accepted only as a legacy alias. The `coding_agent` column identifies which product (`'claude-code' | 'codex'`, NULL = legacy Claude Code row); the `coding_agent_kind` column discriminates the worktree flavor (`'lucidos' | 'app' | 'external'`). Emits `CodingAgent*` events instead of chat `Response*` events. Three flavors:

- **Lucidos-internal coding-agent thread** — works on the Lucidos workspace repo itself. Produces *changes* surfaced via the Apply / Discard UI on completion.
- **App coding-agent thread** — works on a single app folder under the user's workspace (`data/apps/<id>/`) via a sparse-checkout *worktree* of the workspace git. Produces *changes* with the same Apply / Discard UI; Apply does **not** restart the engine and Lucidos's `/harden` does **not** run.
- **External-repo coding-agent thread** — works on a user-registered external git *repository*. Uses a different worktree-creation path and a minimal system prompt; **skips** the Lucidos change-proposal flow on session end.

See also: `system-knowhow/coding-agent-events.md`.

### External-repo coding-agent thread
A *coding-agent thread* (see) running against a user-registered external git *repository* rather than the Lucidos workspace itself. No Apply / Discard surface — the user reviews diffs via the external-repo diff viewer. Worktree creation and system prompt differ from the Lucidos-internal variant; documented in `docs/plans/2026-03-17-external-repos-plan.md`.

### App coding-agent thread
A *coding-agent thread* whose isolated *worktree* sparse-checks out the workspace git on a single `data/apps/<id>/` folder. Same machinery as a Lucidos-internal coding-agent thread (worktree, branch, *change*, *Apply* ff-merge) but on the user's workspace git rather than the Lucidos source repo. No engine restart on *Apply*; Lucidos's `/harden` does not run (apps own their hardening). On *Apply*, the engine emits a transient `AppUiRefreshRequested { app_id }` if any iframe-bundled file changed so open iframes reload with the merged content. The *WIP app preview* surface lets the user see the in-flight app from the worktree while the thread is still open. Branch name shape: `lucidos-<coding-agent>-app-<app_id>-<slug>`, e.g. `lucidos-claude-code-app-habit-tracker-add-streaks` (ADR 0041).
See also: `docs/plans/2026-05-27-app-coding-agent-threads-design.md`.

### WIP app preview
The in-progress rendering of an *app* served from an open *app coding-agent thread*'s *worktree* instead of from the workspace's main copy. Reachable by adding `?thread_id=<id>` to the app UI URL — the panel-overlay slot swaps from the live app (served from `<workspace>/data/apps/<id>/`) to the WIP (served from `<worktree>/data/apps/<id>/`). The WIP iframe loads its HTML/CSS/JS from the worktree, but its SDK calls (`lucidos.data.*`, `lucidos.events.*`) still hit the live workspace endpoints — data-coupled UI edits show their full effect only after *Apply*. The toggle reverts to live when the user navigates away from the thread or after *Apply* removes the worktree.

### Hardening
The quality gate every *coding-agent thread* must run via `/harden` before handing back to the user. Reviews the diff against project rules, runs relevant test suites (Rust + TS + e2e, auto-skipping irrelevant layers), and verifies system-knowhow drift. If the hardening marker is missing when the user clicks *Apply*, Apply runs `/harden` synchronously and the user waits.
See also: `.claude/commands/harden.md`, the playbook the agent actually runs. The requirement to run it is stated to every session by the engine system prompt, which owns it.

### Repository
A user-registered external git repository (row in the `repositories` table) that an *external-repo coding-agent thread* can run against. Distinct from `data/imported/` *imported* repos, which are flattened to plain files as *artifacts*.

## When to add a term

Add a term here when a new concept appears in the user-facing surface — UI strings, chat prose, app/trigger intents, knowhow file frontmatter, or `system-knowhow/*.md` content. If the new concept is dev-internal only (engine plumbing, DB schema, event-bus mechanics), put it in `docs/glossary.md` instead. Coding-agent-only concepts go in the **Advanced — coding agents** section.

## When a term changes

If a term is renamed, retired, or its meaning shifts, update this file in the same commit. Per `.claude/rules/system-knowhow.md`, every `system-knowhow/*.md` file that uses the term must be updated alongside. The `/harden` check enforces this — drift between code/UI and the glossary is a hardening failure.
