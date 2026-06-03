# Glossary (dev)

Internal terms used in the Lucidos codebase, PRs, design docs, and CC sessions. Extends [`system-knowhow/glossary.md`](../system-knowhow/glossary.md) — read that first for user-facing terms (app, artifact, intent, knowhow, plugin, trigger, thread, event, …) and the user-facing advanced-coding-agent terms (Apply, change, Claude Code, coding agent, coding-agent thread, external-repo coding-agent thread, hardening). This file only adds dev-internal concepts the workspace LLM doesn't need.

One term, one definition. When prose anywhere in the repo names a term below, it uses *this* meaning — never a synonym. If you're tempted to write *child thread* for the transitive *sub-thread* concept, *coding agent session* for *agent session*, or *cmd thread* for *coding-agent thread*, use the canonical word.

If a needed concept genuinely isn't in either glossary, add it here (if dev-only) or to `system-knowhow/glossary.md` (if user-facing) in the same change.

## Terms

### Actor
The originator of an event — who or what caused it. Captured on persisted events as `actor: Option<MessageOrigin>`. Set by the mutating HTTP handler via `api::actor::user_actor_resolved(&headers, &state.pool, device_id_override).await`; engine-internal emits pass `None`. The frontend reads it raw to render the actor-chip popover; *ActorMode* derived from `MessageOrigin` drives the UI label (Lucidos Agent / Lucidos Engine / device name).
See also: `.claude/rules/rust.md` § "Mutating endpoints stamp the actor".

### ActorMode
The request-initiation mode — `Human` / `Agent` / `Engine`. Drives the UI actor-chip label via `mcp_client_name(ActorMode)`: `Human` → device label, `Agent` → `"Lucidos Agent"`, `Engine` → `"Lucidos Engine"`. Carried on the `Api { mode }` and `Workspace { mode }` `MessageOrigin` variants so SDK callers and cross-workspace requests can declare themselves explicitly; defaults to `Human` for back-compat. Distinct from *MessageOrigin* itself: ActorMode is the live-request classification; MessageOrigin is the persisted origin record.

### Agent session
The subprocess + worktree pairing managed under `crates/lucidos-engine/src/engine/agent_session/`. Generic across *CodingAgent* variants (the runtime map in `engine_impl.rs:655` is keyed by `CodingAgent`). When the subprocess is *Claude Code* specifically, call it a *Claude Code session* (see entry; or just *CC session* in conversation). Auto-resumes on engine restart unless the branch is already merged. Replaces the older "CC session" term, which baked the product name into the concept.

### Aggregate
The grouping a persisted event belongs to — a `text` column on the `events` table. Determines how the EventBus routes the event and which projection (if any) materializes from it. Today's values: `"thread"`, `"app"`, `"trigger"`, `"artifact"`, `"plugin"`, `"change"`, `"notification"`, `"preference"`, `"presence"`, `"device_presence"`, `"device"`, `"repository"`, `"credential"`, `"oauth_account"`, `"data_file"`, `"pinned_app"`, `"ops"`, `"domain"`. **Not** a per-event-type discriminator — multiple variants share the same aggregate. Set by `SystemEvent::aggregate(&self)` in `event_bus_system_event.rs`.

### Aggregate_id
The `text` column on `events` that ties events together inside an aggregate. For threads, this is the thread UUID as text. **Always cast when joining**: `aggregate_id = c.thread_id::text`. The older `thread_id` column on `events` is legacy; prefer `aggregate_id`.
See also: `.claude/rules/db.md`.

### Blocking descendant
Internal name for the predicate in `is_blocking` (`engine/thread_lifecycle.rs`): a thread whose state currently makes it unsafe to cascade-archive its ancestor. Materialized as `thread_summaries.blocking_descendant_count` and maintained by `propagate_blocking_change` (`engine/event_bus_projection.rs`). User-facing definition lives under *Blocking descendant* in `system-knowhow/glossary.md`.

### blocking_descendant_count
`thread_summaries` column (`INT`, default `0`) holding the rolled-up count of a thread's *blocking descendants* (transitive). Maintained by EventBus; consumed by `available_thread_actions(..., descendants_block_archive: bool)` via `count > 0`.

### Attention-needing descendant
Internal name for the predicate in `is_attention_needing` (`engine/thread_lifecycle.rs`): a thread whose state requires user attention to progress — `WaitingForUserAnswer`, or an in-workspace *coding-agent thread* with pending *changes*. Strict subset of *blocking descendants* — drops the `Running` arm. Running threads are delegated work (Active section); attention-needing threads are pending user action (Review section). Materialized as `thread_summaries.attention_descendant_count`, maintained by the same `propagate_blocking_change` walk that maintains the blocking count. Relationship: `is_blocking = is_attention_needing OR status == Running`.

### attention_descendant_count
`thread_summaries` column (`INT`, default `0`) holding the rolled-up count of a thread's *attention-needing descendants* (transitive). Maintained by EventBus alongside `blocking_descendant_count`; consumed by `display_section(..., has_attention_descendants: bool)` via `count > 0` to bubble the ancestor chain to REVIEW when any descendant needs user attention, even if sibling descendants are still running.

### available_thread_actions
The DB-derivable per-thread action-availability core in `engine/thread_lifecycle.rs` (renamed from `resolve_actions`). Pure function over `thread_summaries` facts — `thread_type`, `status`, `stored_section`, `has_pending_changes`, `descendants_block_archive`, `has_unsent_draft`, `is_saved` — returning the available `Action`s in cascade order: `[DiscardDraft?, Discard?, Apply?, Archive?, Unsave|Save]`. Single source of truth on BOTH sides: codegen'd to TS (`generated/thread-lifecycle.ts`, contract-tested) so the frontend buttons derive from it, AND called server-side via `api::threads::available_thread_actions_for` to guard the mutating HTTP handlers (archive / apply / discard / save / unsave) — a stale frontend or raw API caller can't invoke an action the user couldn't currently take. Verb seam: `available_*` reads only DB-derivable facts; the frontend's *resolveThreadActions* layers in client/UI state.

### Bg-bash wait (retired)
A retired concept. The engine used to refuse `ChangeProposed` at idle while a CC background bash task was still running (`should_propose_change_at_idle`'s bg-bash gate), surfacing a `thread_summaries.coding_agent_bg_bash_pending` column → a "CC waiting on background tasks" banner with no Apply button until a ~5-minute `BgBashWakeRequested` nudge fired. **Removed** (2026-05-29): the wait was worse than the rare wasted re-harden it prevented, and it wedged permanently when CC verified bash completion via a shell `ps` check instead of `TaskOutput` (the in-memory tracker never saw the drain). The change now proposes the instant CC idles regardless of background bash; correctness is covered by *harden-at-apply* (an un-hardened change re-runs `/harden`, tests included, before it can merge), and app/external changes accept the small risk. The gate, the `CCInternalBgBashTracker`, the timer, the `BgBashWakeRequested` event, the column, and the `display_section`/`available_thread_actions` (then `resolve_actions`) `bg_bash_pending` params are all gone. The `CodingAgentIdled.bg_bash_pending` event field is kept as recorded history but no longer projected. Threads wedged by the old gate auto-recover at boot via `propose_held_back_changes_on_startup` (re-proposes any idle CC thread with a committed diff but no proposal).

### BusEvent
The wrapper enum the EventBus routes. Two variants: `BusEvent::Thread { .. }` (per-thread, becomes a *ThreadEvent*) and `BusEvent::System(SystemEvent)` (workspace-wide). Every emit goes through one of these — never bypass to write directly to the `events` table.

### Claude Code session
The *agent session* when its *coding agent* is *Claude Code* — the only variant today; a future *CodingAgent* like Codex would have its own session term (e.g. *Codex session*). Often shortened to *CC session* in conversation, comments, and commit messages. Prefer *agent session* when the discussion is agent-agnostic (the runtime map, the spawn dispatcher, recovery semantics that apply to any *CodingAgent*); reach for *Claude Code session* (or *CC session*) only when the behavior is CC-specific — `--resume` flag semantics, the `/harden` gate, the stop-reminder hook, the `cc_session_id` field on interception events. No extra mechanics beyond *agent session*; same subprocess + worktree pairing.

### Close cascade
The progressive-close behaviour: each invocation resolves EXACTLY ONE close *layer* for the focused thread — draft (confirm-discard the unsent compose draft) → change (Apply / Discard / Cancel choice) → archive — re-running *resolveThreadActions* each time so resolving one layer surfaces the next (stateless re-eval; no cursor, no "cascade in progress" flag). `nextCloseLayer` picks the front-most layer, `runCloseCascade` invokes it; in-flight apply/discard/archive gate it to a no-op (the async bridge that keeps stateless re-eval honest). Driven by the per-thread buttons (each click resolves its own layer) and the `closeThread` keybinding (default `Ctrl/Cmd+Shift+W`) — a *keybinding registry* entry dispatched generically via `matchShortcut` / `SHORTCUT_ACTIONS` in `useKeyboardShortcuts.ts` and rebindable in Settings → Keyboard Shortcuts. Shift-modified so plain `Cmd/Ctrl+W`, the browser's reserved tab-close, is left alone (sibling of the `Ctrl+Shift+O` new-thread chord). Distinct from *Escape*, which is non-destructive and never triggers it.

### CodingAgent (enum)
The Rust enum representing which *coding agent* (the user-facing role, see system-knowhow glossary) is driving a session. Today one variant: `CodingAgent::ClaudeCode`. A future Codex integration would add `CodingAgent::Codex`. Replaces the older `AgentKind` enum, whose name didn't make the scope explicit. The runtime map (`engine_impl.rs:655`) is keyed by this enum, so adding a coding agent is variant + runtime impl + registration.

### Contract test
A Rust↔TypeScript cross-validation generated from `crates/lucidos-engine/src/engine/agent_session/thread_lifecycle.rs`. The TS file in `crates/lucidos-app/src/generated/` is **never** hand-edited — regenerate with `cargo test -p lucidos-engine generate_typescript_file -- --ignored && cargo test -p lucidos-engine generate_cross_validation_fixture_file -- --ignored`.
See also: `.claude/rules/testing.md` § Contract Tests.

### Engine supervisor
Bash wrapper (`scripts/lib/engine_supervisor.sh`, function `run_supervised`) that runs the engine binary in a restart loop. SIGKILL / OOM / panic exits trigger an exponential-backoff respawn (1–30 s) and rewrite `engine.pid`; clean exits (0 / 130 / 138, i.e. `graceful_shutdown` or SIGUSR1/SIGINT defaults) break the loop. Traps SIGTERM/SIGINT on itself so `web-dev.sh`'s `kill_stale_processes` (which `pkill -P`s the supervisor as a direct child) forwards SIGUSR1 to the engine and exits cleanly instead of respawning the engine that's about to be rebuilt. The pid the supervisor writes is always the *live* engine, so `stop.sh`, `is_protected_host_pid`, and `kill_stale_processes` keep working across restarts. Wired in from `start_engine` (`scripts/lib/workspace.sh`); exported as `ENGINE_SUPERVISOR_PID` for `web-dev.sh`'s wait branch. Call it the **engine supervisor**, never "watchdog" or "babysitter" — the canonical noun lines up with the file/function/variable names. The "never call it watchdog or babysitter" rule applies to *this* concept (engine-binary respawn) only. The *in-loop watchdog* and *external watchdog* inside *agent session* are canonically watchdogs — they watch for stuck Lucidos *agent sessions* and emit `ContinuationRequested`, never restart the engine.

### Event store
The persistence layer for events — the `events` table plus the append semantics that write to it. Today the *write* path is inlined inside *EventBus* (raw `INSERT INTO events` at `event_bus.rs:218, 276`) — `EventBus::emit` is the only API that appends. The `EventStore` struct (`core/store/mod.rs`) still exists, but only as a **read-only query facade** (`query_events`, `count_events`, `get_event_by_id`, …); its historical `append` / `append_thread_event` write methods were removed when persistence moved to *EventBus*. The term remains valid for the concept.

### EventBus
The in-process emit/subscribe mechanism. `EventBus::emit(BusEvent)` is the **sole** entry point for event persistence — it owns the *event store* directly. Consumers (SSE, projections, scheduler matcher, side-effects) subscribe to the bus, never poll the table.

### EventChannel
Also called **channel**. The source-channel tag merged into every persisted event's payload via `EventMeta::channel`, distinguishing which thread surface emitted the event. The Rust enum `EventChannel` lives in `engine/thread_events.rs`; today's variants are `Chat`, `ClaudeCode`, and `Trigger`, serialized to the wire strings `"chat"`, `"claude_code"`, and `"trigger"` respectively. The `"claude_code"` wire string is the deliberate *Claude Code* instance identifier (not a legacy alias) and is part of the persistence + frontend contract — a future Codex coding agent would slot in as `EventChannel::Codex` with wire string `"codex"`. The user-facing *Thread* entry references the same identifiers via `thread_summaries.source`. `Trigger` is the umbrella for all trigger-driven runs (scheduled, event, hybrid); the precise invocation that fired a given run is recorded separately on `TriggerStarted.invocation`.

### EventMeta
Cross-cutting fields the EventBus merges into a persisted event's payload at write time: `request_event_id`, `channel`, `actor`, etc. Built per-variant by the helpers in `engine/thread_events.rs`. New variants take `EventMeta::with_actor(actor)` rather than reading the device header themselves.

### External watchdog
The 12-minute backstop that ticks from its own `tokio::spawn` outside every per-thread loop (`agent_session/external_watchdog.rs`, `EXTERNAL_WATCHDOG_LIMIT_MS`, tick interval `EXTERNAL_WATCHDOG_TICK_SECS = 30 s`). Scans `agent_sessions`; for any session past the limit where the gate (`is_waiting` / `tools_in_flight`) says we'd otherwise have fired, drops the entry from `agent_sessions` and emits a `ContinuationRequested { reason: AUTO_RECOVERY_AFTER_HANG_REASON }` which the spawn dispatcher consumes to issue a fresh `--resume`. Same outcome as the *in-loop watchdog*'s auto-recovery path, but reachable from outside a wedged `select!`. The 12 > 10 min gap gives the in-loop watchdog first crack; the external tick is a no-op when the in-loop succeeded. Distinct from *Engine supervisor*.

### Family
A root thread plus every transitive descendant reachable through `parentThreadId` — the unit the thread drawer renders together. Distinct from *sub-thread* (any single descendant): a family is the whole set. The drawer routes families as one (highest-priority section wins: active > review > saved > archive — running anywhere keeps the family under Active even when a sibling has a real CTA, so the CTA renders inline on the child row rather than dragging the still-working family out of Active) and sorts them by the freshest signal anywhere in the subtree, so a parent automatically rises with its descendants and `nestByParent` can put each child directly under its parent. The **family root** is the topmost ancestor present in the currently-rendered list; an orphan child (parent paginated out / filtered) is its own root. Implemented in `crates/lucidos-app/src/components/drawer/ThreadDrawer.tsx` as the `FamilyGraph` (`{ byId, rootByThread }`) and `FamilyKeys` (`{ revivedKey, recentKey, reviewTier }`) types, built by `computeFamilyGraph` and `computeFamilyKeys`.

### Family extension
The ancestor + descendant threads of a paginated set that get loaded eagerly so the drawer can render them under their parent, even when their own `last_activity` falls below the loaded window. Backend helper `EventStore::fetch_family_extension` (recursive CTE over `thread_summaries.parent_thread_id`); HTTP layer returns them in a separate `family_threads` field on `GET /threads` and `GET /threads/older`. The frontend upserts them into `threadMap` like any other thread but tracks their ids in `familyExtensionIds` (in `store/actions/thread-loading.ts`) so they're **excluded from the `loadOlderThreads` pagination cursor** — without that exclusion, a single old child would advance the cursor past every intervening thread. An id is removed from the set when natural pagination later returns it as a base thread.

### In-loop watchdog
The 10-minute inactivity timer inside `run_session`'s `select!` (`WATCHDOG_INACTIVITY_LIMIT_MS` in `crates/lucidos-engine/src/engine/agent_session/lifecycle.rs`). When an *agent session* has been silent past the limit while the watchdog gate (`is_waiting` / `tools_in_flight`) says it should have fired, the in-loop watchdog cancels the agent token and the loop emits a `ContinuationRequested` so the spawn dispatcher issues a fresh `--resume`. First line of defense against a hung subprocess — useless when the `select!` itself is wedged (e.g. an event-handler await waiting on a slow subscriber). See *External watchdog* for the floor that catches that case. Distinct from *Engine supervisor*, which operates at the engine-binary level.

### Keybinding registry
The customizable keyboard-shortcut subsystem. Pure registry + binding math in
`utils/shortcuts.ts` (`SHORTCUT_DEFS`, `Binding`, `eventToBinding`,
`matchesEvent`, `formatBinding`, `serialize`/`parseBinding`, `bindingSearchText`).
The override-aware layer is `store/actions/keybindings.ts`: it merges per-user
overrides — persisted as the workspace `keybindings` preference (a JSON map of
`shortcutId → "mod+shift+o"`, synced across devices via `PreferencesChanged`) —
over the registry defaults, exposing `bindingFor` / `setBinding` / `resetBinding`
/ `matchShortcut` / `recordChord` and the override-aware `tooltipWithShortcut`.
`useKeyboardShortcuts` dispatches by the current binding (so rebinds take effect
with no code change), Settings → Keyboard Shortcuts renders the cheat sheet +
recorder, and SearchEverywhere indexes each shortcut by its combo aliases. The
`mod` modifier matches either Cmd or Ctrl (both fire); the single-key `c`/`t`
shortcuts were dropped in favor of modifier chords only.

### Lifted family
A *family* whose root's own natural display section is lower-priority than the section the family was routed to — i.e. a descendant earned the lift. Surfaces in the drawer as two coordinated cues: the parent row renders with demoted styling (muted opacity) so it reads as "I'm here because of one of my children", and any non-root descendant whose own natural section equals the routed section ("responsible child") gets a brighter accent rail in place of the default gray. When every family member naturally belongs to the routed section, the family isn't lifted and neither cue fires. Computed by `computeFamilyDecorations` (in `crates/lucidos-app/src/components/drawer/ThreadDrawer.tsx`) as `{ routedByThread, liftedRoots }`; routed section comes from the same priority pass `categorizeThreads` uses (active > review > saved > archive).

### Loadable<T>
The four-state TypeScript type for every async-fetched value: `{ status: 'not-loaded' }` / `'loading'` / `'loaded'; data: T` / `'failed'; error: string`. Store signals **must** wrap async data in `Loadable<T>` — bare arrays are a bug because they mask loading as empty.
See also: `.claude/rules/frontend.md` § "Async Data Loading".

### Overlay stack
The central LIFO registry of dismissable overlays (`store/overlayStack.ts`) — modals (via `ModalOverlay`), pseudo-fullscreen, etc. Each overlay registers its `dismiss` on mount and removes it on unmount. Replaces the per-instance `document` Escape listeners that used to race each other and any global key handler; the one capture-phase Escape dispatcher in `useKeyboardShortcuts` (`dispatchEscape`) pops the top entry. Escape policy is non-destructive, in order: dismiss the top overlay → else, if the focused text input opts out via `data-escape-self`, leave it for the element's own Escape handler (used where a blur commits work, e.g. the thread-title editor whose blur saves a rename — blurring on Escape there would save instead of cancel) → else blur a focused text input → else no-op (it never touches the focused thread or discards work). The *close cascade* lives on a separate trigger, not Escape — so a double-Escape on a cascade confirm cancels the dialog, it can't skip a layer.

### Per-thread events bump
The frontend reactivity primitive in `crates/lucidos-app/src/store/threadActivity.ts` that splits SSE fan-out into two channels so a long-running streaming thread doesn't fire every wide `threadMap` subscriber on every token. `bumpThreadEvents(threadId)` increments a lazy-created `Signal<number>` for that thread only (RAF-coalesced); `getThreadEventsBump(threadId)` subscribes to just that thread. Streaming events (`TextStreamed`, `CodingAgentTextStreamed`, `CodingAgentToolCalled`, `ContextCaptured`, …) bump the per-thread signal and do **not** flip `threadMap`. Meta-shape changes (status, title, channel, codingAgent flags, child counts, …) flip `threadMap` and fire the wide subscribers (`attentionThreadCount`, `ThreadDrawer.ThreadList`, every `PromptInput` effect). Focused-thread views (`activeExchanges`, `activeStreamingBuffer`, `ThreadView`'s exchanges memo) subscribe to the per-thread bump and read `threadMap.peek()`. **Contract**: every code path that mutates `thread.events`, `thread.streamingBuffer`, or `thread.pendingUserMessages` MUST pair the mutation with `bumpThreadEvents(threadId)` — the SSE handler at the bottom of `handleThreadEvent` bumps unconditionally; the optimistic-message paths in `chat.ts` (`addPendingMessage`, `removePendingMessage`, `clearStalePendingMessages`, the unreachable-engine fallback) and the `CodingAgentThreadSpawned` branch in `handleGlobalEvent` bump explicitly. Wires same pattern as `composeDrafts` / `draftPresentThreadIds` for compose state.

### MessageOrigin
The Rust enum for *actor*, defined in `crates/lucidos-engine/src/engine/thread_events.rs`. Variants:

- `Device { device_id: String, label: String }` — human at a known device. `device_id` is the TEXT primary key from the `devices` table.
- `Api { user_agent: Option<String>, mode: ActorMode }` — HTTP request without a device id or cross-workspace caller. `mode` defaults to `Human`.
- `Workspace { workspace, thread_id, event_id, user_agent, mode }` — HTTP request from another Lucidos workspace, identified by the `caller_workspace` body field. `mode` carries upstream intent (defaults to `Human`).
- `ThreadLink { thread_id, title, spawning_event_id, mode, direction }` — bidirectional link to another thread in the same workspace. `direction = Parent` means the linked thread spawned the receiving thread; `direction = Child` means a child posting a callback. (Carries `#[serde(alias = "parent_thread")]` so historical rows from the old unidirectional `ParentThread` variant still deserialize; `direction` defaults to `Parent` when missing.)
- `System` — internal projection / housekeeping origin (e.g. `MessageOrigin::system()`).
- `Engine { reason: EngineReason }` — engine-internal action (e.g. `MessageOrigin::engine(EngineReason::HardenRetrigger)`).

Optional on `SystemEvent` variants (`actor: Option<MessageOrigin>`); the four legacy change-event variants carry per-variant `actor` instead. `MessageOrigin::mode()` derives the *ActorMode* used for UI labelling.

### Persisted event
A subset of *Event* (see user-facing glossary): past-tense, written to the `events` table by `EventBus::emit`, replayable, visible to projections, matched by triggers. The default flavor.

### Transient event
A subset of *Event*: past-tense, broadcast over SSE only, never persisted, never reaches projections or the trigger matcher. A trigger on a transient event can never fire — it isn't even allowlistable, because the scheduler's matcher only looks at persisted events. Routing is determined by `ThreadEvent::is_persisted()` / `SystemEvent::is_persisted()`.

### Presence (device_presence)
Transient projection that feeds the PresenceCheck protocol's candidate list. `device_presence(device_id PK, visible_at)` records devices with any visible top-level Lucidos tab; `DevicePresenceStore::candidates` returns the device_ids fresher than 2 minutes (`PRESENCE_STALE_AFTER`). An empty list short-circuits `send_push_to_all_with_app` to `push_allowed=true` without bothering to broadcast the PresenceCheck. The projection comes from transient `SystemEvent`s (`DeviceVisible` / `DeviceHidden`) — never persisted to the events table. Frontend reporter: `device-presence.ts`, heartbeating every 30s while visible. Per-thread focus used to live in a separate `thread_presence` table updated by `ThreadFocused` / `ThreadUnfocused` events; that path was retired once the PresenceCheck pong started reporting `focused_thread_id` live (see `system-knowhow/notifications.md` §3).

### PresenceTracker
The in-memory `Arc<Mutex<HashMap<Uuid, PendingSlot>>>` on `LucidosEngine` that owns in-flight PresenceCheck slots. `expect(notification_id, expected_pongs)` registers interest and returns an `Arc<Notify>`; `record(req)` appends a pong and signals via `notify_one()` once `expected_pongs` are in (so a pong that lands before the fan-out task awaits doesn't lose its wakeup); `collect(notification_id)` drains the slot. Each `expect()` also sweeps slots older than 5 s so a panicking fan-out task can't leak entries. Lives only in memory and is wiped on engine restart; transient by design. See `crates/lucidos-engine/src/api/presence_pong.rs` and `system-knowhow/notifications.md` §3.

### Projection
A cached materialized view maintained by EventBus inside `emit()`, in the same transaction as the source event INSERT — `thread_summaries` (from thread lifecycle events), `notifications` (from `NotificationCreated`), `pinned_apps` (from pinning events). The *event store* is the source of truth; projections are recomputable, and consistent with it under read-your-write. See the `EventBus` struct doc-comment in `crates/lucidos-engine/src/engine/event_bus.rs` for the full five-phase pipeline (Validate → Persist → **Project** → CaptureAggregate → PostCommit).

### Request_id
The `request_event_id` field that links a request event (e.g. `MessageReceived`) to all its terminal events (`ResponseGenerated`, `ResponseCanceled`, `ResponseAborted`, `ResponseFailed`). Carried via `EventMeta::request_event_id`. Used by the UI to group a turn and by the engine to settle stuck-running threads.

### resolveThreadActions
Frontend (`store/actions/threadActions.ts`) enrichment tier over the codegen'd `availableThreadActions`. Feeds `has_unsent_draft` from the live `composeDrafts` signal (fresh, ahead of the 250 ms compose debounce), applies the external-repo carve-out (a CC change that can't merge into a foreign repo → Archive instead of Apply/Discard), and maps each bare `Action` into a *TaggedAction* `{ kind, category, label, tooltip?, invoke }` whose `invoke` encapsulates the confirm + handler. Both the per-thread buttons (WaitingBanner close buttons, PromptInput Save/Unsave toggle) and the *close cascade* render/drive from these, so a shortcut can only invoke an action whose button is currently available (no enablement drift). The `available_*` / `resolve_*` verb seam marks the DB-vs-client boundary: see *available_thread_actions*.

### resolveGlobalActions
Frontend composer (`store/actions/threadActions.ts`): the top overlay-dismiss action (when the *overlay stack* is non-empty) followed by the focused thread's *resolveThreadActions*. The single "what can the user do right now, globally" selector — the Escape dispatcher consults its dismiss action, the close cascade its `close`-category layers.

### Scheduler blocklist
The `ThreadEvent::is_per_token_streaming` predicate (`crates/lucidos-engine/src/engine/thread_events.rs`) consumed by the scheduler subscriber's `BusEvent::Thread` arm in `crates/lucidos-engine/src/scheduler/mod.rs`. Filters out per-token streaming variants — today: `TextStreamed`, `Thinking`, `CodingAgentTextStreamed` — before they reach the trigger matcher. Every other persisted `ThreadEvent` flows through and is subscribable via `on_event:`. Triggers wired to a blocked variant validate and persist but **never fire**.
See also: `system-knowhow/thread-events.md` § "Today the scheduler uses a blocklist", `.claude/rules/system-knowhow.md`.

### Signer
A WebAssembly module that signs outbound proxy requests with credentials (HMAC, OAuth, vendor-specific). Loaded by the `WasmSignerLayer` in the proxy pipeline. Installed under `data/auth-modules/` as a `.wasm` + optional *signer manifest* sidecar — see user-facing *auth module*. Real-artifact tests live in `crates/lucidos-e2e/tests/wasm_signers.rs`; inline-WAT tests live in `crates/lucidos-engine/tests/proxy_wasm_engine.rs`.

### Source of truth
PostgreSQL (the `events` table + projections) plus the filesystem (`data/artifacts/`, `data/apps/`, `data/knowhow/`, `data/triggers/`, `data/config/`, `data/auth-modules/`, `data/scripts/`). In-memory state is **cache or active runtime only** — the engine must be restartable at any time without losing user-visible state.
See also: `CLAUDE.md` § "Engine Statelessness".

### SystemEvent
The workspace-wide event enum. Each variant is its own `SystemEvent::Variant` — grouping is provided by `aggregate()` ("trigger", "app", …), **not** by an `event_type: String` discriminator. `DomainEvent` is one variant among many; never use it as a generic wrapper. Every variant is past-tense (the events-only model — see user-facing *Event*); persistence is set per-variant via `SystemEvent::is_persisted()`.
See also: `.claude/rules/rust.md` § "System events are individual variants".

### TaggedAction
Frontend-only enriched action object produced by *resolveThreadActions*: `{ kind: Action | 'dismiss_overlay', category: 'close' | 'primary' | 'save' | 'dismiss', label, tooltip?, invoke }`. `category` drives button styling and lets the *close cascade* target only `close`-layer actions (so it can't fire an unrelated action by accident); `invoke` runs the action, opening a confirm where the action warrants one. The UI shell of the `Action` enum that *available_thread_actions* returns bare.

### ThreadEvent
The per-thread event enum (`crates/lucidos-engine/src/engine/thread_events.rs`). Every variant is past-tense (the events-only model — see user-facing *Event*); the `is_persisted()` method routes between *persisted event* and *transient event* on emit. Variant added/removed/renamed, payload changed, persistence flipped, or alias added → MUST update `system-knowhow/thread-events.md` in the same change (and `coding-agent-events.md` if the change touches `CodingAgent*` / `UserQuestion*` / `CodingAgentPermission*`).

### Worktree
An isolated git worktree under `<workspace>/.lucidos/worktrees/` where an *agent session* runs. All edits, builds, and test runs stay inside it; scripts resolve paths via `SCRIPT_DIR` so they pick up the worktree's code automatically. Cleaned up by Apply or by the `ExitWorktree` CC tool. For *app coding-agent threads*, the worktree is a sparse-checkout (cone mode) of the workspace git narrowed to `data/apps/<id>/` (see *app worktree*); for Lucidos-internal and external-repo sessions, it's a full checkout of the relevant git.

### App worktree
The sparse-checkout *worktree* that backs an *app coding-agent thread*. Created by `git worktree add --no-checkout <path> -b <branch> main` against the workspace git, then `git sparse-checkout init --cone` + `git sparse-checkout set data/apps/<id>` materialises only that app folder plus top-level files (workspace `.gitignore`). Path: `<workspace>/.lucidos/worktrees/thread-<short>/`. Branch: `claude-code/app/<id>/<ts>-<uuid>`. *Apply* ff-merges the branch into the workspace git's `main` and emits `AppUiRefreshRequested` if any iframe-bundled file changed; no push to any remote (workspace git is local by default). Lifecycle helper: `git_ops::create_sparse_app_worktree`.

### Stranded worktree
A *worktree* on disk whose git admin dir (`<repo>/.git/worktrees/<name>`, pointed at by the worktree's `.git` link file) no longer exists — typically from an interrupted `git worktree remove` or a `git worktree prune` that dropped the admin entry while the working dir survived. Every `git` invocation from it then fails with `fatal: not a git repository`, so the cleanup worker's git-based tiers (which lean on `git status` / `git rev-parse`) can't act and `remove_worktree_and_optionally_delete_branch` early-returns. Detected by `worktree_cleanup::worktree_git_admin_missing` (conservative: only `true` when the link target is positively gone, resolving relative gitdirs against the worktree) and removed by `remove_stranded_worktree` (subpath-guarded plain `remove_dir_all`, never touches branch refs — committed work survives in the main repo's branch) after a fixed `STRANDED_GRACE` that does not accelerate under disk pressure. Distinct from an *orphaned temp worktree* (`harden-`/`apply-`/`merge-` left by a crashed apply/harden/merge flow), which is swept separately gated on its change row's status.

### CodingAgentKind
Rust enum (`Lucidos | App | External`) discriminating the *coding-agent thread* flavor. Serialized as snake_case on `SessionStarted.coding_agent_kind` and persisted in `thread_summaries.coding_agent_kind`. Distinct from `CodingAgent` (which picks the backend product — today only `ClaudeCode`). The `apply_change` path reads it to decide whether to run the `/harden` gate, whether to gate engine restart via `files_require_restart`, and whether to emit `AppUiRefreshRequested` after a successful merge.

## When to add a term

Add it here if it's dev-only — engine plumbing, DB schema, test infrastructure, build tooling, CC mechanics. If users (or the workspace LLM) would ever encounter it, add it to `system-knowhow/glossary.md` instead (under **Core** if it's general, or under **Advanced — coding agents** if it's only relevant to coding-agent workflows).

## When a term changes

Update this file in the same commit that renames or retires the term — same rule as `system-knowhow/glossary.md`. `/harden` flags drift.
