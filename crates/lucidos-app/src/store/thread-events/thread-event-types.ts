import type { EventChannel, SessionEndReason } from '../../generated/thread-lifecycle';
import type { ContextSection } from '../types';

export type ThreadInitiator = 'user' | 'system';

/** Mirrors the Rust `ActorMode` enum (lowercase strings). */
export type ActorMode = 'human' | 'agent' | 'engine';

/** Mirrors the Rust `EngineReason` enum (serde tag = "kind", snake_case).
 *  `session_recovered` is the legacy name for `continuation_started` — kept
 *  on the union so legacy DB rows still typecheck during the migration window
 *  (the Rust side has the matching `#[serde(alias = "session_recovered")]`,
 *  and the migration rewrites historical rows to the new name). */
export type EngineReason =
  | { kind: 'continuation_started' }
  | { kind: 'session_recovered' }
  | { kind: 'orphan_recovery' }
  | { kind: 'scheduler'; trigger_id: string; trigger_name?: string }
  | { kind: 'harden_retrigger' }
  | { kind: 'stale_session' }
  | { kind: 'merge_conflict' }
  | { kind: 'missing_hardening' };

/** Direction of a `thread_link` origin: which end of the parent⇄child
 *  relationship the linked thread sits on relative to the receiving thread. */
export type ThreadDirection = 'parent' | 'child';

/** Mirrors the Rust `MessageOrigin` enum (serde tag = "kind", snake_case).
 *  `api.source_thread_id` populated when the request came from a
 *  Lucidos-spawned subprocess (CC, `run_bash`, `run_python`, scheduled
 *  script, `lucidos` CLI) — engine sets it from the
 *  `x-lucidos-source-thread-id` header after validating the
 *  `x-lucidos-agent-origin-token` against the per-engine-startup token.
 *  When set, `mode === 'agent'` (subprocesses are never human) and the
 *  popover renders a deep-link back to the spawning thread. */
export type MessageOrigin =
  | { kind: 'device'; device_id: string; label: string }
  | { kind: 'api'; user_agent?: string; mode?: ActorMode; source_thread_id?: string }
  | {
      kind: 'workspace';
      workspace: string;
      thread_id?: string;
      event_id?: string;
      user_agent?: string;
      mode?: ActorMode;
    }
  | {
      kind: 'thread_link';
      thread_id: string;
      title?: string;
      spawning_event_id?: string;
      mode?: ActorMode;
      direction?: ThreadDirection;
    }
  | { kind: 'engine'; reason: EngineReason }
  /** The host system killed the underlying process (engine shutdown, OS signal,
   *  crash, safety-net catch). Distinct from `engine` which represents
   *  engine-deliberate actions (hardening retrigger, scheduler, merge conflict). */
  | { kind: 'system' };

/** Display label for engine-deliberate work (hardening, merging, scheduler). */
export const ENGINE_LABEL = 'Lucidos Engine';

/** Display label for process killed by the host system (engine shutdown,
 *  safety-net catch, OS signal). Distinct from `ENGINE_LABEL`: the engine
 *  acts deliberately; the system just kills processes. */
export const SYSTEM_LABEL = 'System';

/** Display label for work kicked off by a Lucidos LLM agent in another thread
 *  (parent_thread origin) — distinct from the engine, which only owns events
 *  it literally raises on its own (recovery, hardening, scheduler, …). */
export const LUCIDOS_AGENT_LABEL = 'Lucidos Agent';

/** Icon paired with `LUCIDOS_AGENT_LABEL` — used by both the initiator chip
 *  (who started the work) and the executor chip (who produced the response).
 *  Single source so the two panels stay in sync. */
export const LUCIDOS_AGENT_ICON = '✨';

/** Display label for an external HTTP caller that did NOT self-identify as a
 *  known actor (no device id, no agent-origin token, no cross-workspace
 *  caller). The popover surfaces the User-Agent for forensics; the chip just
 *  says "API caller" so the user never sees an anonymous mutation rendered as
 *  "You". */
export const API_CALLER_LABEL = 'API caller';

/** Icon paired with `API_CALLER_LABEL` — plug signals "external integration
 *  plugging into the API", deliberately distinct from the 👤 person icon
 *  reserved for `kind: device` (the only origin that is unambiguously "You"). */
export const API_CALLER_ICON = '🔌';

/** Derive the ActorMode from a MessageOrigin. Mirrors the Rust
 *  `MessageOrigin::mode()` impl: device is intrinsic Human, engine and system
 *  are intrinsic Engine, the others read from the carried `mode` field
 *  (defaulting to the same defaults the backend uses for old DB rows). */
export function originMode(origin: MessageOrigin | undefined): ActorMode {
  if (!origin) return 'engine'; // unknown origin → engine acted on its own
  switch (origin.kind) {
    case 'device':      return 'human';
    case 'api':         return origin.mode ?? 'human';
    case 'workspace':   return origin.mode ?? 'human';
    case 'thread_link': return origin.mode ?? 'agent';
    case 'engine':      return 'engine';
    case 'system':      return 'engine';
  }
}

/** Map a `MessageOrigin` to its display icon + label. The chip answers "who
 *  decided this" with one of the closed set of known actors: **You** (a real
 *  browser device), **Lucidos Agent** (LLM acting on behalf of the user),
 *  **Lucidos Engine** (deterministic engine work), **System** (host killed the
 *  process), or **API caller** (external HTTP caller that did NOT self-identify
 *  as one of the above).
 *
 *  "You" is reserved for `kind: device` — a browser session bound to a known
 *  device row. Any other human-mode origin (anonymous API client, cross-workspace
 *  human, …) renders as "API caller" so an unauthenticated POST can never
 *  impersonate the user in the timeline. The popover still discloses the
 *  origin kind, user-agent, and workspace name underneath. */
export function actorInitiator(actor: MessageOrigin | undefined): { icon: string; label: string } {
  if (actor?.kind === 'system') return { icon: '⚙', label: SYSTEM_LABEL };
  if (actor?.kind === 'device') return { icon: '\u{1F464}', label: 'You' };
  switch (originMode(actor)) {
    case 'human':  return { icon: API_CALLER_ICON, label: API_CALLER_LABEL };
    case 'agent':  return { icon: LUCIDOS_AGENT_ICON, label: LUCIDOS_AGENT_LABEL };
    case 'engine': return { icon: '⚙', label: ENGINE_LABEL };
  }
}

/** Mirrors Rust's `CancelCause` (snake_case). User-driven termination of a
 *  *real* in-flight response — `EventMeta.actor` identifies the user, this
 *  enum identifies what they did. `unknown` covers legacy DB rows persisted
 *  before the field existed and any retired cause string (e.g. `stale_settle`,
 *  which moved to `AbortCause`). */
export type CancelCause = 'user_stop' | 'user_action' | 'unknown';

/** Mirrors Rust's `AbortCause` (snake_case). System-driven cleanup — the
 *  engine or OS terminated the process, or the engine settled a projection
 *  whose live process was already gone (`stale_settle`). `unknown` only
 *  appears on legacy DB rows persisted before the field existed. */
export type AbortCause =
  | 'engine_shutdown'
  | 'safety_net'
  | 'recovery_after_restart'
  | 'process_killed'
  | 'stale_settle'
  | 'unknown';

/** Summary text for a `ResponseAborted` event. `stale_settle` (engine cleanup
 *  of a stuck projection on a user button click) reads "Settled stuck
 *  response" — distinct from a real abort because no live response existed.
 *  Otherwise: device actor = `/api/v1/restart` pre-emit ("You — Restarted");
 *  anything else is the host system killing the process ("System — Response
 *  interrupted"). */
export function responseAbortedSummary(
  actor: MessageOrigin | undefined,
  cause: AbortCause | undefined,
): string {
  if (cause === 'stale_settle') return 'Settled stuck response';
  return actor?.kind === 'device' ? 'Restarted' : 'Response interrupted';
}

/** Summary text for a `ResponseCanceled` event — always a user-driven stop
 *  on a real in-flight response, so no cause-dependent branching is needed. */
export const RESPONSE_CANCELED_SUMMARY = 'Canceled the response';

// Persisted thread events — stored in DB, appear in snapshots.
// Optional fields (`?`) allow older DB rows (before the field was added) to deserialize safely.
/** Mirrors the Rust `ChildCompletionStatus` (serde rename_all = "snake_case").
 *  Drives the status badge on the child-completion card. */
export type ChildCompletionStatus = 'success' | 'failure' | 'no_changes' | 'canceled';

/** Mirrors the Rust `AllowScope` (serde rename_all = "snake_case"). Carried on
 *  `CodingAgentPermissionResolved` so the answered card can render the same
 *  button layout as the prompt with a check on the chosen scope and
 *  strike-through on the rest. `undefined` covers Allow-once, Deny, and
 *  recovery-emitted orphan resolutions (no scope was picked). */
export type PersistScope = 'narrow' | 'broad' | 'session';

export type ThreadEvent =
  | { type: 'MessageReceived'; text: string; channel?: EventChannel; user_image_hashes?: string[]; device_id?: string; device?: string; image_description?: string; mode?: ActorMode; model?: string; reasoning_effort?: string; parent_thread_id?: string; spawning_event_id?: string; origin?: MessageOrigin }
  | { type: 'TextStreamed'; text: string }
  | { type: 'ThoughtStreamed'; text: string; context_tokens?: number; context_messages?: number; trimmed?: boolean }
  // ContextTokensMeasured / ContextAssembled are legacy event types — old DB
  // rows still surface them; new emissions use ContextCaptured below. Kept on
  // the union so the projection's switch can branch on them without `as`.
  | { type: 'ContextTokensMeasured'; input_tokens: number }
  | { type: 'ContextAssembled'; sections: ContextSection[]; tools: string[]; model: string; total_chars: number }
  | {
      type: 'ContextCaptured';
      producer: 'main_llm' | 'claude_code';
      model: string;
      context_window: number;
      /** Absent on snapshot rows (server strips for size — see
       *  `strip_context_capture_sections` in `api/threads.rs`). Live SSE
       *  emissions carry the full array; lazy-fetch covers the snapshot
       *  case via `GET /events/:event_id/context`. */
      sections?: ContextSection[];
      tools?: string[];
      estimated_total_tokens: number;
      usage?: { input_tokens: number; output_tokens: number; cache_read_tokens: number; cache_creation_tokens: number };
      trimmed?: boolean;
      /** Stamped by the snapshot endpoint when `sections` + `tools` were dropped. */
      sections_stripped?: boolean;
    }
  | { type: 'MemorySearched'; results?: number; queries?: string[] }
  | { type: 'ToolCalled'; name: string; args: unknown; description?: string }
  | {
      type: 'ToolResult';
      name: string;
      /** Absent on snapshot rows (server strips for size — see
       *  `strip_tool_result_content` in `api/threads.rs`). Live SSE emissions
       *  carry the full text; lazy-fetch covers the snapshot case via
       *  `GET /events/:event_id/tool-result`. */
      result?: string;
      images?: string[];
      tool_called_event_id?: string;
      /** Stamped by the snapshot endpoint when `result` was dropped. */
      result_stripped?: boolean;
    }
  | { type: 'TodoListWritten'; items: TodoItem[] }
  | { type: 'ResponseGenerated'; text?: string; images?: string[]; model?: string; reasoning_effort?: string; request_event_id?: string; channel?: EventChannel }
  | { type: 'ResponseCanceled'; text?: string; images?: string[]; model?: string; reasoning_effort?: string; actor?: MessageOrigin; channel?: EventChannel; cause?: CancelCause }
  | { type: 'ResponseAborted'; text?: string; images?: string[]; model?: string; reasoning_effort?: string; request_event_id?: string; actor?: MessageOrigin; channel?: EventChannel; cause?: AbortCause }
  | { type: 'ResponseFailed'; error: string; request_event_id?: string; channel?: EventChannel }
  | { type: 'SessionStarted'; session_id: string; branch?: string; repo_id?: string }
  | { type: 'ContinuationStarted'; branch?: string; origin?: MessageOrigin; actor?: MessageOrigin }
  // SessionEnded.reason is loosely typed to tolerate legacy DB rows whose
  // payloads carry removed values like 'completed' / 'changes_proposed' /
  // 'auto_ended' / 'user_ended' / 'stale_resume' / 'discarded'. The current
  // (Phase 4) terminal-only reasons are 'shutdown' / 'panic' / 'closed' plus
  // the engine's 'legacy_non_terminal' catch-all; everything else is a
  // historical row and should be treated as a harmless terminal end.
  | { type: 'SessionEnded'; reason?: SessionEndReason | string }
  | { type: 'CodingAgentTextStreamed'; text: string }
  | { type: 'CodingAgentToolCalled'; name: string; args: unknown; description?: string; tool_use_id?: string }
  | { type: 'CodingAgentToolResult'; name: string; result: string; tool_use_id?: string }
  | { type: 'CodingAgentUserMessageSent'; text: string }
  | { type: 'CodingAgentPromptSent'; text: string; origin?: MessageOrigin }
  | { type: 'MissingHardeningDetected'; origin?: MessageOrigin }
  | { type: 'CodingAgentIdled'; has_changes?: boolean; requires_restart?: boolean; is_external_repo?: boolean; cc_session_id?: string }
  | { type: 'ThreadTitleGenerated'; title: string }
  | { type: 'ThreadTitleRenamed'; title: string; actor?: MessageOrigin }
  | { type: 'ThreadSaved'; actor?: MessageOrigin }
  | { type: 'ThreadUnsaved'; actor?: MessageOrigin }
  | { type: 'ThreadArchived'; actor?: MessageOrigin }
  | { type: 'ThreadStarted'; mode: string; actor?: MessageOrigin }
  | { type: 'ThreadDiscarded'; actor?: MessageOrigin; discarded_at?: string }
  | { type: 'TriggerStarted'; trigger_id: string; trigger_name?: string; prompt?: string; invocation?: TriggerInvocation; origin?: MessageOrigin }
  | { type: 'TriggerCompleted'; trigger_id: string; trigger_name?: string; result_summary?: string }
  | { type: 'ChangeProposed'; change_id?: string; description?: string; files?: string[]; requires_restart?: boolean; path?: string; diff?: string; origin?: MessageOrigin; commit_sha?: string; incomplete?: boolean }
  | { type: 'ChangeApplied'; change_id?: string; requires_restart?: boolean; client_update?: boolean; commits?: string[]; thread_title?: string; actor?: MessageOrigin; path?: string }
  | { type: 'ChangeDiscarded'; change_id?: string; actor?: MessageOrigin; path?: string }
  | { type: 'ChangeReverted'; change_id?: string; actor?: MessageOrigin; path?: string }
  | { type: 'ChangeApplyFailed'; change_id?: string; error?: string; actor?: MessageOrigin }
  | { type: 'MergeConflictDetected'; change_id?: string; files?: string[]; origin?: MessageOrigin }
  | { type: 'UserPromptInjected'; text: string; mode?: ActorMode; origin?: MessageOrigin; injected_message_id?: string }
  | { type: 'CredentialRequested'; provider: string }
  | { type: 'McpConsentRequested'; tool: string; args: unknown }
  | { type: 'CodingAgentSettingsChanged'; model?: string; reasoning_effort?: string; permission_mode?: string; cc_session_id?: string }
  | { type: 'UserQuestionAsked'; tool_use_id: string; cc_session_id: string; question: string; options?: QuestionOption[]; multi_select?: boolean }
  | { type: 'UserQuestionAnswered'; tool_use_id: string; answer: AnswerKind; actor?: MessageOrigin }
  | { type: 'CodingAgentPermissionRequest'; request_id: string; tool_use_id: string; tool_name: string; input: Record<string, unknown>; summary: string }
  | { type: 'CodingAgentPermissionResolved'; request_id: string; allowed: boolean; reason?: string; persist_scope?: PersistScope; actor?: MessageOrigin }
  | { type: 'ChildThreadCompleted'; child_thread_id: string; child_thread_title?: string; status: ChildCompletionStatus; summary: string; pending_change_ids?: string[] }
  // Background Flash enrichment of a prior MessageReceived's attached images
  // (one event per attached hash, all carrying the same description text).
  // Replaces the deprecated `image_description` field on MessageReceived; new
  // emissions of MessageReceived no longer carry it. The frontend has no UI
  // consumer today — the description only matters to the backend's history /
  // title-generation paths — but the type belongs on the union so projections
  // can pattern-match without `as` casts when the SSE stream delivers one.
  | { type: 'ImageDescribed'; source_event_id: string; hash: string; description: string; model: string };

/** See `system-knowhow/glossary.md` § Todo item. `abandoned` is engine-only:
 *  the engine flips any pending/in_progress item to `abandoned` at response
 *  termination so the user can see the agent walked away from it. The LLM
 *  cannot write that status via `todo_write`. */
export type TodoStatus = 'pending' | 'in_progress' | 'completed' | 'abandoned';

/** See `system-knowhow/glossary.md` § Todo item. `active_form` is shown only
 *  while `status === 'in_progress'`; otherwise the row renders `content`. */
export interface TodoItem {
  content: string;
  active_form: string;
  status: TodoStatus;
}

/** Mirrors the Rust `QuestionOption` in thread_events.rs. */
export interface QuestionOption {
  id: string;
  label: string;
  description?: string;
}

/** Mirrors the Rust `TriggerInvocation` (serde tag = "kind") — records which
 *  path fired a particular trigger run. The popover panel uses this to label
 *  the run as "Scheduled" vs "Event triggered" and, for the latter, to render
 *  the matched event details. */
export type TriggerInvocation =
  | { kind: 'Schedule' }
  | { kind: 'Event'; event_type: string; event_id?: string };

/** Mirrors the Rust `AnswerKind` (serde tag = "kind"). MultiSelected's
 *  optional `text` carries freetext typed in the prompt textarea while the
 *  question was on screen — backend joins it with the resolved labels when
 *  relaying to CC. */
export type AnswerKind =
  | { kind: 'Selected'; option_id: string }
  | { kind: 'FreeText'; text: string }
  | { kind: 'MultiSelected'; option_ids: string[]; text?: string }
  | { kind: 'Canceled' };

// Transient events — live SSE only, never stored. All names are past tense
// (events-only model; T7 renamed every imperative / present-participle variant).
// Old wire names (e.g. RefreshAppUI, CredentialRequest) only appear on legacy
// persisted rows, never on live SSE — old clients won't see them once the
// engine restarts, so this union only enumerates the canonical names.
export type TransientEvent =
  // Streaming state
  | { type: 'CumulativeTextUpdated'; text: string }
  | { type: 'LlmCallRetried'; reason: string }
  | { type: 'PreambleCompleted' }
  // Request events — trigger frontend modals/actions
  | { type: 'CredentialPromptRequested'; payload: string }
  | { type: 'PluginInstallRequested'; payload: string }
  | { type: 'PluginUninstallRequested'; payload: string }
  | { type: 'EmailConfirmRequested'; payload: string }
  | { type: 'PushNotificationRequested' }
  | { type: 'McpConsentPromptRequested'; data: string }
  | { type: 'FileRefreshRequested'; path: string }
  | { type: 'AppUiRefreshRequested'; app_id: string }
  | { type: 'AppUiCaptureRequested'; app_id: string; request_id: string }
  | { type: 'NavigationRequested'; payload: string }
  | { type: 'CodingAgentThreadSpawned'; cc_thread_id: string; title: string }
  | { type: 'ChildrenCountChanged'; active: number; total: number };

export type StoredEvent = ThreadEvent & { created?: string; _displayCreated?: string; _eventId?: string };

/** Events that define (or redefine) a thread's channel/source. */
export function isChannelDefiningEvent(eventType: string): boolean {
  return eventType === 'SessionStarted'
    || eventType === 'ContinuationStarted'
    || eventType === 'TriggerStarted';
}

export type SequencedEvent = {
  seq: number;
  event: StoredEvent;
};

/** Thread section as stored in the DB projection (thread_summaries.archive_state).
 *  'archived' = archive/saved, 'inbox' = needs user attention (both chat and CC).
 *  Wire JSON field name stays `section` for backwards-compat. */
export type ThreadSection = 'archived' | 'inbox';
