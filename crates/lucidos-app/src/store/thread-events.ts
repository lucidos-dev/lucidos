import { EVENT_CLASSIFICATION, LAST_ACTIVITY_EVENTS, SESSION_END_REASONS } from '../generated/thread-lifecycle';
import type { EventChannel, SessionEndReason } from '../generated/thread-lifecycle';
import { MODELS, REASONING_LEVELS } from './models';
import type { ExchangeStatus } from './exchange-status';
import { mergeAdjacentTextEvents, isMeaningfulText } from './event-rendering';
import type { Step, ResponseEvent, ContextSection, ContextAssembledData, ContextCapture } from './types';

/** Who started a thread: user-initiated or system-initiated (e.g. scheduled task). */
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

/** Mirrors the Rust `MessageOrigin` enum (serde tag = "kind", snake_case). */
export type MessageOrigin =
  | { kind: 'device'; device_id: string; label: string }
  | { kind: 'api'; user_agent?: string; mode?: ActorMode }
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

/** Map a `MessageOrigin` to its display icon + label, driven by mode (not
 *  origin kind). The chip answers "who decided this": a human at the keyboard,
 *  an LLM acting on behalf of the user, or deterministic engine code. The
 *  origin variant (device / api / workspace / thread_link) is metadata for
 *  the popover and does not affect the chip.
 *
 *  Special case: the `system` origin renders as "System", distinct from the
 *  generic engine label. The engine acts deliberately (hardening, scheduler);
 *  the system just kills processes (shutdown, OS signal, crash). */
export function actorInitiator(actor: MessageOrigin | undefined): { icon: string; label: string } {
  if (actor?.kind === 'system') return { icon: '⚙', label: SYSTEM_LABEL };
  switch (originMode(actor)) {
    case 'human':  return { icon: '\u{1F464}', label: 'You' };
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
 *  Otherwise: device actor = `/api/restart` pre-emit ("You — Restarted");
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
  | { type: 'Thinking'; text: string; context_tokens?: number; context_messages?: number; trimmed?: boolean }
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
      sections: ContextSection[];
      tools?: string[];
      estimated_total_tokens: number;
      usage?: { input_tokens: number; output_tokens: number; cache_read_tokens: number; cache_creation_tokens: number };
      trimmed?: boolean;
    }
  | { type: 'MemorySearched'; results?: number; queries?: string[] }
  | { type: 'ToolCalled'; name: string; args: unknown; description?: string }
  | { type: 'ToolResult'; name: string; result: string; images?: string[] }
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
  | { type: 'CodingAgentSettingsChanged'; model?: string; reasoning_effort?: string; permission_mode?: string }
  | { type: 'UserQuestionAsked'; tool_use_id: string; cc_session_id: string; question: string; options?: QuestionOption[]; multi_select?: boolean }
  | { type: 'UserQuestionAnswered'; tool_use_id: string; answer: AnswerKind; actor?: MessageOrigin }
  | { type: 'CodingAgentPermissionRequest'; request_id: string; tool_use_id: string; tool_name: string; input: Record<string, unknown>; summary: string }
  | { type: 'CodingAgentPermissionResolved'; request_id: string; allowed: boolean; reason?: string; persist_scope?: PersistScope; actor?: MessageOrigin }
  | { type: 'ChildThreadCompleted'; child_thread_id: string; child_thread_title?: string; status: ChildCompletionStatus; summary: string; pending_change_ids?: string[] };

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

// Transient events — live SSE only, never stored
export type TransientEvent =
  // Streaming state (present participle)
  | { type: 'TextStreaming'; text: string }
  | { type: 'Retrying'; reason: string }
  | { type: 'PreambleCompleting' }
  // Side-effect commands — trigger frontend modals/actions
  | { type: 'CredentialRequest'; payload: string }
  | { type: 'PluginInstallRequest'; payload: string }
  | { type: 'PluginUninstallRequest'; payload: string }
  | { type: 'EmailConfirmRequest'; payload: string }
  | { type: 'PushNotificationRequest' }
  | { type: 'McpConsentRequest'; data: string }
  | { type: 'RefreshFile'; path: string }
  | { type: 'RefreshAppUI'; app_id: string }
  | { type: 'CaptureAppUI'; app_id: string; request_id: string }
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
 *  'archived' = history/saved, 'inbox' = needs user attention (both chat and CC).
 *  Wire JSON field name stays `section` for backwards-compat. */
export type ThreadSection = 'archived' | 'inbox';

/** Post-event projection snapshot carried on persisted SSE thread events
 *  (`data.aggregate`) and on `fetchThreadEvents` HTTP responses
 *  (`currentAggregate`). Mirrors the backend `ThreadAggregate` struct.
 *  Compose fields are intentionally excluded — they have their own
 *  broadcast cadence and including them would clobber local typing.
 *  Frontend overlays this onto `thread.meta` so it never has to derive
 *  thread state from event-type lookups (SECTION_TRANSITIONS / STATUS_TRANSITIONS). */
export type ThreadAggregate = {
  threadId: string;
  title: string;
  channel: string;
  initiator: ThreadInitiator;
  createdAt: string;
  lastActivity: string;
  messageCount: number;
  section: ThreadSection;
  status: ThreadStatus;
  activeChildrenCount: number;
  totalChildrenCount: number;
  ccHasChanges: boolean;
  ccRequiresRestart: boolean;
  ccIsExternalRepo: boolean;
  ccApplying: boolean;
  isSaved: boolean;
  hasResponse: boolean;
  lastRevivedAt: string | null;
  parentThreadId: string | null;
  parentThreadTitle: string | null;
  triggerId?: string;
  triggerName?: string;
  ccRepoId?: string;
  ccRepoName?: string;
  state: ThreadComposeState;
};

/** Apply an aggregate snapshot to a thread's meta. Used by live SSE (per-event
 *  aggregate) and historical replay (fetchThreadEvents.currentAggregate).
 *  Nullable fields propagate cleared values; trigger/repo fields are omitted
 *  by the backend when not applicable, so absence preserves prior values. */
export function applyAggregateToMeta(meta: ThreadMeta, agg: ThreadAggregate): void {
  meta.section = agg.section;
  meta.status = agg.status;
  meta.activeChildrenCount = agg.activeChildrenCount;
  meta.totalChildrenCount = agg.totalChildrenCount;
  meta.ccHasChanges = agg.ccHasChanges;
  meta.ccRequiresRestart = agg.ccRequiresRestart;
  meta.ccIsExternalRepo = agg.ccIsExternalRepo;
  meta.ccApplying = agg.ccApplying;
  meta.saved = agg.isSaved;
  meta.messageCount = agg.messageCount;
  meta.updatedAt = agg.lastActivity;
  meta.lastRevivedAt = agg.lastRevivedAt ?? '';
  meta.state = agg.state;
  meta.parentThreadId = agg.parentThreadId ?? undefined;
  meta.parentThreadTitle = agg.parentThreadTitle ?? undefined;
  if (agg.triggerId) meta.triggerId = agg.triggerId;
  if (agg.triggerName) meta.triggerName = agg.triggerName;
  if (agg.ccRepoId) meta.repoId = agg.ccRepoId;
  if (agg.ccRepoName) meta.repoName = agg.ccRepoName;
}

/** Placeholder shown in the drawer while a thread waits for its first
 *  LLM-generated title. Treated as "no title" by anything that displays it. */
export const PENDING_TITLE_PLACEHOLDER = '...';

export type ThreadMeta = {
  id: string;
  title: string;
  channel: EventChannel | 'error_unknown_channel';
  initiator: ThreadInitiator;
  saved: boolean;
  createdAt: string;
  updatedAt: string;
  /** Thread status computed by the backend: 'idle', 'running', or 'waiting'. */
  status: ThreadStatus;
  /** Server-computed exchange count (MESSAGE_COUNT_EVENTS in thread_lifecycle.rs). */
  messageCount: number;
  /** Section from backend DB projection — used as initial section before events load. */
  section: ThreadSection;
  /** Number of active child threads (non-zero means parent is "in progress"). */
  activeChildrenCount: number;
  /** Total number of child threads (active + finished). */
  totalChildrenCount: number;
  /** Whether the CC session has proposed changes. */
  ccHasChanges: boolean;
  /** Whether the proposed changes require an engine restart. */
  ccRequiresRestart: boolean;
  /** Whether the CC session is working on an external repo. */
  ccIsExternalRepo: boolean;
  /** Whether a merge conflict is being resolved. */
  ccApplying: boolean;
  /** When the thread last entered 'running' state (for IN PROGRESS sort order). */
  lastRevivedAt: string;
  /** Set when mode != 'human' on the initial MessageReceived. */
  parentThreadId?: string;
  parentThreadTitle?: string;
  /** Trigger that fired this thread (only for `channel === 'trigger'`). */
  triggerId?: string;
  /** Trigger name at fire-time (snapshot — falls back when the trigger is renamed/deleted). */
  triggerName?: string;
  /** Repository the CC session bound to (only for `channel === 'claude_code'`). */
  repoId?: string;
  /** Current repo name from the registry — undefined when the repo was deleted. */
  repoName?: string;
  /** Compose state machine. Server is the source of truth; events flow via
   *  ThreadStarted, MessageReceived, ThreadDiscarded, ThreadArchived.
   *
   *  Draft text / images / mode pick live in the sibling `composeDrafts`
   *  signal (see `store/composeDrafts.ts`). They are NOT on ThreadMeta:
   *  per-keystroke draft writes would otherwise re-render every component
   *  subscribed to threadMap (most expensively ChatExchange, which calls
   *  marked.parse per render). */
  state: ThreadComposeState;
};

export type ThreadComposeState = 'composing' | 'active' | 'discarded' | 'archived';
export type ComposeChannelMode = 'lucidos' | 'claude_code' | null;

export type ThreadState = {
  meta: ThreadMeta;
  events: Map<number, StoredEvent>;
  streamingBuffer: string;
  eventsLoaded: boolean;
  /** True when loadThreadEvents exhausted retries. The UI shows an error
   *  state instead of a spinner. On next resume, runResumeSync retries
   *  failed threads via loadThreadEvents (which resets this flag). */
  eventsLoadFailed: boolean;
  /** SSE events may arrive out of order during reconnect, so we track
   *  the max DB-loaded sequence separately to avoid skipping gap events. */
  lastDbSeq: number;
  /** Optimistic user messages shown before real SSE events arrive.
   *  Each entry is removed when its corresponding MessageReceived event arrives
   *  from SSE, matched by the client-generated event_id UUID. */
  pendingUserMessages: Array<{ text: string; eventId: string; created: string; image_hashes?: string[] }>;
};

export type ThreadStatus = 'idle' | 'running' | 'waiting' | 'waiting_for_user_answer' | 'failed';

/** Build a fresh `ThreadState` for optimistic / SSE-bootstrapped threads.
 *  All CC/changes flags default to false and counts to 0; callers override
 *  what they actually know. Centralised so adding a `ThreadMeta` field is a
 *  one-line change instead of a four-place audit.
 *
 *  Compose draft (text/images/mode) lives in the sibling `composeDrafts`
 *  signal — callers that bootstrap a `composing` thread (compose.ts,
 *  thread-sync.ts ThreadStarted skeleton) seed the draft entry separately
 *  via `setDraft`. Keeps this builder free of signal side effects. */
export function makeOptimisticThreadState(opts: {
  id: string;
  title: string;
  channel: ThreadMeta['channel'];
  initiator: ThreadInitiator;
  eventsLoaded: boolean;
  timestamp?: string;
  pendingUserMessages?: ThreadState['pendingUserMessages'];
  triggerId?: string;
  triggerName?: string;
  repoId?: string;
  repoName?: string;
  /** Override compose state — defaults to 'active'. Set 'composing' for
   *  optimistic draft creation (compose.ts, ThreadStarted SSE). */
  state?: ThreadComposeState;
  /** Override status — defaults to 'running'. Composing rows want 'idle'. */
  status?: ThreadStatus;
}): ThreadState {
  const ts = opts.timestamp ?? new Date().toISOString();
  return {
    meta: {
      id: opts.id,
      title: opts.title,
      channel: opts.channel,
      initiator: opts.initiator,
      saved: false,
      createdAt: ts,
      updatedAt: ts,
      status: opts.status ?? 'running',
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: ts,
      triggerId: opts.triggerId,
      triggerName: opts.triggerName,
      repoId: opts.repoId,
      repoName: opts.repoName,
      state: opts.state ?? 'active',
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: opts.eventsLoaded,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: opts.pendingUserMessages ?? [],
  };
}

/** Sort threads by updatedAt descending (most recent first). */
export const byRecent = (a: ThreadState, b: ThreadState): number =>
  b.meta.updatedAt.localeCompare(a.meta.updatedAt);

/** Sort review threads in two tiers, then by recency within each tier:
 *
 *   Tier 1 (system has identified a CTA): ccHasChanges, WaitingForUserAnswer,
 *   or Failed.
 *   Tier 2: everything else (idle threads in the inbox).
 *
 *   Within each tier: most-recently-updated first.
 */
export const byReviewOrder = (a: ThreadState, b: ThreadState): number => {
  const tier = (t: ThreadState) =>
    t.meta.ccHasChanges
      || t.meta.status === 'waiting_for_user_answer'
      || t.meta.status === 'failed'
        ? 0
        : 1;
  const ta = tier(a);
  const tb = tier(b);
  if (ta !== tb) return ta - tb;
  return byRecent(a, b);
};

/** Whether this event type updates the thread's last_activity in the backend
 *  projection (event_bus.rs). Generated from thread_lifecycle.rs. */
function updatesLastActivity(type: string): boolean {
  return LAST_ACTIVITY_EVENTS.has(type);
}

/** CC activity event types — tool calls, text streaming, and tool results.
 *  Used to detect active CC work after mid-session completion events.
 *  Derived from the generated thread lifecycle contract. */
const CC_ACTIVITY_EVENTS = new Set(
  Object.entries(EVENT_CLASSIFICATION)
    .filter(([evt, cls]) => cls === 'activity' && evt.startsWith('CodingAgent'))
    .map(([evt]) => evt)
);

/** CC waiting info — now sourced from backend thread_summaries projection. */
export type CCWaitingInfo = {
  hasChanges: boolean;
  isExternalRepo: boolean;
  requiresRestart: boolean;
  applying: boolean;
};

/** Get CC waiting info from thread meta (backend-computed). */
export function getCCWaitingInfo(meta: ThreadMeta): CCWaitingInfo | null {
  if (meta.status !== 'waiting') return null;
  if (meta.channel !== 'claude_code') return null;
  return {
    hasChanges: meta.ccHasChanges,
    isExternalRepo: meta.ccIsExternalRepo,
    requiresRestart: meta.ccRequiresRestart,
    applying: meta.ccApplying,
  };
}

export type Exchange = {
  userEvent: StoredEvent;
  userSeq: number;
  steps: SequencedEvent[];
};

/** The narrowed `UserQuestionAnswered` variant — exposed so call sites that
 *  walk an Exchange's steps can read the question's resolution (answer + actor)
 *  without redeclaring the shape. */
export type AnsweredQuestion = Extract<ThreadEvent, { type: 'UserQuestionAnswered' }>;

/** The narrowed `CodingAgentPermissionResolved` variant — same purpose as
 *  `AnsweredQuestion`, for permission-prompt resolutions. */
export type ResolvedPermission = Extract<ThreadEvent, { type: 'CodingAgentPermissionResolved' }>;

/** Find the matching `UserQuestionAnswered` step in a divider exchange.
 *  Returns the typed event (with `answer` narrowed and the optional `actor`
 *  stamped by `EventMeta`) or undefined when the question is still pending. */
export function findQuestionAnswer(exchange: Exchange, toolUseId: string): AnsweredQuestion | undefined {
  for (const { event } of exchange.steps) {
    if (event.type === 'UserQuestionAnswered' && event.tool_use_id === toolUseId) return event;
  }
  return undefined;
}

/** Find the matching `CodingAgentPermissionResolved` step in a permission
 *  divider exchange. Returns the typed event or undefined when the request
 *  is still pending. */
export function findPermissionResolution(exchange: Exchange, requestId: string): ResolvedPermission | undefined {
  for (const { event } of exchange.steps) {
    if (event.type === 'CodingAgentPermissionResolved' && event.request_id === requestId) return event;
  }
  return undefined;
}

// ---------------------------------------------------------------------------
// Exchange-level derived data — standalone functions on Exchange.
// ---------------------------------------------------------------------------

/** Derive the user message text from an exchange. */
export function exchangeUserMessage(exchange: Exchange): string {
  const ev = exchange.userEvent;
  if (ev.type === 'TriggerStarted') {
    return ev.prompt || ev.trigger_name || '';
  }
  if (ev.type === 'ContinuationStarted') {
    return 'Resumed after engine restart';
  }
  if (ev.type === 'ResponseAborted') {
    return responseAbortedSummary(ev.actor, ev.cause);
  }
  if (ev.type === 'ResponseCanceled') {
    return RESPONSE_CANCELED_SUMMARY;
  }
  if (ev.type === 'MissingHardeningDetected') {
    return `${ENGINE_LABEL} — Hardening`;
  }
  if (ev.type === 'MergeConflictDetected') {
    const files = ev.files ?? [];
    const suffix = files.length > 0 ? ` (${files.length} file${files.length === 1 ? '' : 's'})` : '';
    return `${ENGINE_LABEL} — Merging changes from main${suffix}`;
  }
  if (isChangeLifecycleEvent(ev)) return '';
  if ('text' in ev) return (ev as { text: string }).text;
  return '';
}

/** Derive the user channel from the exchange's user event.
 *  Reads the `channel` field from MessageReceived, or infers from event type. */
export function exchangeUserChannel(exchange: Exchange): string | undefined {
  const t = exchange.userEvent.type;
  if (t === 'TriggerStarted') return 'trigger';
  if (t === 'ContinuationStarted' || t === 'MissingHardeningDetected' || t === 'MergeConflictDetected') {
    return 'claude_code';
  }
  if (t === 'ResponseAborted' || t === 'ResponseCanceled') {
    // Boundary event — channel is the original thread's channel; leaving it
    // undefined lets the caller fall back to thread meta when needed.
    return undefined;
  }
  if (exchange.userEvent.type === 'MessageReceived') return exchange.userEvent.channel;
  return undefined;
}

/** Change lifecycle event types — render as terminal initiator-only panels. */
export type ChangeLifecycleType =
  | 'ChangeApplied' | 'ChangeDiscarded' | 'ChangeReverted' | 'ChangeApplyFailed';

export type ChangeLifecycleEvent = Extract<ThreadEvent, { type: ChangeLifecycleType }>;

const CHANGE_LIFECYCLE_TYPES: ReadonlySet<string> = new Set([
  'ChangeApplied', 'ChangeDiscarded', 'ChangeReverted', 'ChangeApplyFailed',
]);

export function isChangeLifecycleEvent(event: { type: string }): event is ChangeLifecycleEvent {
  return CHANGE_LIFECYCLE_TYPES.has(event.type);
}

/** Who sent the user event. Maps `MessageReceived.mode` to the UI's binary
 *  user-vs-system distinction: `human` → user, `agent`/`engine` → system. */
export function exchangeUserSource(exchange: Exchange): ThreadInitiator {
  const ev = exchange.userEvent;
  if (ev.type === 'MessageReceived') return modeToInitiator(ev.mode);
  return isSystemExchange(exchange) ? 'system' : 'user';
}

/** Map an `ActorMode` to the UI's binary user-vs-system label.
 *  Undefined defaults to `'user'` (mirrors the engine's `default_mode_human`
 *  for old DB rows persisted before the `mode` field existed). */
export function modeToInitiator(mode: ActorMode | undefined): ThreadInitiator {
  return mode === 'agent' || mode === 'engine' ? 'system' : 'user';
}

/** Whether this exchange was system-initiated (auto-recovery, auto-hardening,
 *  auto-merge, scheduled trigger, change lifecycle, abort/resume boundary)
 *  rather than user-initiated. */
function isSystemExchange(exchange: Exchange): boolean {
  const ev = exchange.userEvent;
  return ev.type === 'ContinuationStarted' || ev.type === 'TriggerStarted'
    || ev.type === 'MissingHardeningDetected' || ev.type === 'MergeConflictDetected'
    || ev.type === 'ResponseAborted'
    || isChangeLifecycleEvent(ev);
}

/** Extract user-pasted image hashes from the exchange's MessageReceived event.
 *  Post-Phase-3b the event payload carries `user_image_hashes: string[]` only;
 *  the bytes live in the content-addressed blob store and are loaded by the
 *  renderer via `<img src="/api/v1/blobs/<hash>">`. */
export function exchangeUserImageHashes(exchange: Exchange): string[] {
  if (exchange.userEvent.type !== 'MessageReceived') return [];
  const raw = (exchange.userEvent as { user_image_hashes?: unknown }).user_image_hashes;
  if (!Array.isArray(raw)) return [];
  return raw.filter((h): h is string => typeof h === 'string');
}

/** Extract a field from the response completion event or CodingAgentSettingsChanged fallback.
 *  Walks steps backward, skipping terminal events that omit the field (recovery
 *  paths emit ResponseAborted with model=null). CC sessions fall back to
 *  CodingAgentSettingsChanged (emitted at session start). Chat sessions fall back to the
 *  request metadata stamped on MessageReceived so the route tooltip shows
 *  model/effort while the response is still streaming. */
type ResponseField = 'model' | 'reasoning_effort';
function extractResponseField(exchange: Exchange, field: ResponseField): string | undefined {
  let ccFallback: string | undefined;
  for (let i = exchange.steps.length - 1; i >= 0; i--) {
    const event = exchange.steps[i].event;
    if (event.type === 'ResponseGenerated' || event.type === 'ResponseCanceled' || event.type === 'ResponseAborted') {
      const v = event[field];
      if (v) return v;
    }
    if (!ccFallback && event.type === 'CodingAgentSettingsChanged' && event[field]) {
      ccFallback = event[field];
    }
  }
  if (ccFallback) return ccFallback;
  if (exchange.userEvent.type === 'MessageReceived') {
    const v = exchange.userEvent[field];
    if (v) return v;
  }
  return undefined;
}

export function exchangeResponseModel(exchange: Exchange): string | undefined {
  return extractResponseField(exchange, 'model');
}

export function exchangeReasoningEffort(exchange: Exchange): string | undefined {
  return extractResponseField(exchange, 'reasoning_effort');
}

const MODEL_LABELS: Record<string, string> = Object.fromEntries([
  ...MODELS.map(m => [m.value, m.label]),
  ['claude-opus-4-1', 'Opus 4.1'],
  ['claude-haiku-4-5-20251001', 'Haiku 4.5'],
  ['claude-haiku-4-5@20251001', 'Haiku 4.5'],
  // CC subprocess short aliases — `CodingAgentSettingsChanged.model` carries
  // these verbatim, so without an explicit label the popover renders the bare
  // alias (e.g. `opus[1m]`).
  ['opus', 'Opus 4.6'],
  ['opus[1m]', 'Opus 4.6 (1M)'],
  ['sonnet', 'Sonnet 4.6'],
  ['sonnet[1m]', 'Sonnet 4.6 (1M)'],
  ['haiku', 'Haiku 4.5'],
]);

export function displayModelName(modelId: string): string {
  return MODEL_LABELS[modelId] ?? modelId;
}

const EFFORT_LABELS: Record<string, string> = Object.fromEntries(
  REASONING_LEVELS.map(l => [l.value, l.label]),
);

export function displayReasoningEffort(effort: string): string {
  return EFFORT_LABELS[effort] ?? effort;
}

/** Derive the user timestamp from an exchange. */
export function exchangeTimestamp(exchange: Exchange): string {
  return exchange.userEvent.created
    || exchange.userEvent._displayCreated
    || new Date().toISOString();
}

/** Derive the response timestamp — the latest step event's `created` timestamp.
 *  Returns undefined if there are no step events (no response yet). */
export function exchangeResponseTimestamp(exchange: Exchange): string | undefined {
  for (let i = exchange.steps.length - 1; i >= 0; i--) {
    if (exchange.steps[i].event.created) return exchange.steps[i].event.created;
  }
  return undefined;
}

/** Check if the exchange has actual CC content (tools/text, not just SessionStarted). */
function exchangeHasCCContent(exchange: Exchange): boolean {
  return exchange.steps.some(({ event }) => CC_ACTIVITY_EVENTS.has(event.type));
}

/** Build the response text by concatenating all TextStreamed/CodingAgentTextStreamed events. */
export function exchangeResponseText(exchange: Exchange): string {
  let text = '';
  for (const { event } of exchange.steps) {
    if (event.type === 'TextStreamed' || event.type === 'CodingAgentTextStreamed') {
      text += (event as { text: string }).text;
    }
  }
  return text;
}

/** Format a multi-line code/command string as "Run <first line>" (truncated to 60 chars). */
function describeRun(text: string): string {
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    if (trimmed) return trimmed.length > 60 ? `Run ${trimmed.slice(0, 57)}...` : `Run ${trimmed}`;
  }
  return 'Run command';
}

/** Full primary-arg value for an engine tool call — used as a hover tooltip when
 *  the rendered description elides it (Rust `describe_tool()` truncates commands,
 *  paths, prompts, and URLs to ~60 chars). Returns whichever single arg the
 *  description actually clips so the tooltip mirrors the un-elided form.
 *  Undefined when nothing useful would differ from the description. */
export function fullCommandForEngineTool(name: string, args: unknown): string | undefined {
  const a = args as Record<string, unknown> | null | undefined;
  if (!a) return undefined;
  const s = (k: string) => (typeof a[k] === 'string' ? (a[k] as string) : undefined);

  switch (name) {
    case 'run_bash': return s('command');
    case 'run_python': return s('code');
    case 'read_file':
    case 'write_file':
    case 'edit_file':
    case 'delete_file':
    case 'refresh_file': return s('path');
    case 'copy_file': return s('destination');
    case 'import_file': return s('source_path');
    case 'browser_open':
    case 'http_request': return s('url');
    case 'web_search': return s('query');
    case 'execute_intent': return s('intent_id');
    case 'emit_event':
    case 'query_events': return s('event_type');
    case 'send_notification': return s('title');
    case 'send_email': return s('subject');
    case 'generate_image':
    case 'run_thread': return s('prompt');
    default: return undefined;
  }
}

/** Full primary-arg value for a Claude Code tool call — used as a hover tooltip
 *  when the rendered description elides it (Rust `describe_cc_tool()` in
 *  `crates/lucidos-engine/src/core/mod.rs` shows basenames for paths,
 *  truncates Bash commands to 57 chars + first line, and shows only the URL
 *  origin for WebFetch). `Agent` returns `prompt` rather than the short
 *  `description` field Rust uses, since the prompt is the actual hidden detail.
 *  Undefined when no primary arg is defined for the tool. */
export function fullCommandForCCTool(name: string, args: unknown): string | undefined {
  const a = args as Record<string, unknown> | null | undefined;
  if (!a) return undefined;
  const s = (k: string) => (typeof a[k] === 'string' ? (a[k] as string) : undefined);

  switch (name) {
    case 'Read':
    case 'Edit':
    case 'MultiEdit':
    case 'Write':
    case 'NotebookEdit': return s('file_path');
    case 'Bash': return s('command');
    case 'WebFetch': return s('url');
    case 'Glob':
    case 'Grep': return s('pattern');
    case 'WebSearch': return s('query');
    case 'Agent': return s('prompt');
    case 'Skill': return s('skill');
    case 'TodoWrite': {
      const todos = a.todos;
      if (!Array.isArray(todos) || todos.length === 0) return undefined;
      const MARKERS: Record<string, string> = { completed: '[x]', in_progress: '[~]', pending: '[ ]' };
      return todos.map((t) => {
        const { content, activeForm, status } = t as { content?: string; activeForm?: string; status?: string };
        const marker = MARKERS[status ?? ''] ?? '[?]';
        const text = (status === 'in_progress' && activeForm) ? activeForm : (content ?? '');
        return `${marker} ${text}`;
      }).join('\n');
    }
    default: return undefined;
  }
}

/** @deprecated Fallback for old events without a stored description. New descriptions come from Rust `describe_tool()`. */
function describeEngineTool(name: string, args: unknown): string {
  const a = args as Record<string, unknown> | null | undefined;
  const str = (key: string) => (a && typeof a[key] === 'string' ? a[key] as string : '');
  const basename = (p: string) => p.split('/').pop() || p;

  switch (name) {
    case 'read_file': return str('path') ? `Read ${basename(str('path'))}` : 'Read file';
    case 'write_file': return str('path') ? `Write ${basename(str('path'))}` : 'Write file';
    case 'edit_file': return str('path') ? `Edit ${basename(str('path'))}` : 'Edit file';
    case 'list_files': return str('path') ? `List ${str('path')}` : 'List files';
    case 'copy_file': return str('destination') ? `Copy to ${basename(str('destination'))}` : 'Copy file';
    case 'delete_file': return str('path') ? `Delete ${basename(str('path'))}` : 'Delete file';
    case 'import_file': return str('url') ? `Import ${basename(str('url'))}` : 'Import file';
    case 'run_bash': return str('command') ? describeRun(str('command')) : 'Run bash';
    case 'run_python': return str('code') ? describeRun(str('code')) : (str('description') || 'Run Python');
    case 'execute_intent': return str('intent_id') ? `Run intent: ${str('intent_id')}` : 'Run intent';
    case 'emit_event': return str('event_type') ? `Emit ${str('event_type')}` : 'Emit event';
    case 'query_events': return str('event_type') ? `Query ${str('event_type')}` : 'Query events';
    case 'web_search': return str('query') ? `Search "${str('query')}"` : 'Web search';
    case 'http_request': return str('url') ? `HTTP ${str('method') || 'GET'} ${str('url').split('/').slice(0, 3).join('/')}` : 'HTTP request';
    case 'send_notification': return str('title') ? `Notify: ${str('title')}` : 'Send notification';
    case 'send_email': return str('subject') ? `Email: ${str('subject')}` : 'Send email';
    case 'read_emails': return 'Read emails';
    case 'read_email': return 'Read email';
    case 'fetch_news': return str('query') ? `News: ${str('query')}` : 'Fetch news';
    case 'browser_open': return str('url') ? `Open ${str('url').split('/').slice(0, 3).join('/')}` : 'Open browser';
    case 'browser_extract': return 'Extract page content';
    case 'browser_click': return str('selector') ? `Click ${str('selector')}` : 'Click element';
    case 'browser_type': return 'Type text';
    case 'browser_eval': return 'Run browser script';
    case 'browser_screenshot': return 'Take screenshot';
    case 'browser_close': return 'Close browser';
    case 'git_clone': return str('url') ? `Clone ${basename(str('url'))}` : 'Clone repo';
    case 'create_app': return str('name') ? `Create app: ${str('name')}` : 'Create app';
    case 'create_trigger': return str('name') ? `Schedule: ${str('name')}` : 'Create task';
    case 'run_claude': return 'Run Claude Code';
    case 'correct_memory': return 'Correct memory';
    case 'set_language': return str('language') ? `Set language: ${str('language')}` : 'Set language';
    case 'set_timezone': return str('timezone') ? `Set timezone: ${str('timezone')}` : 'Set timezone';
    case 'refresh_file': return str('path') ? `Refresh ${basename(str('path'))}` : 'Refresh file';
    case 'refresh_app': { const n = str('app_name') || str('app_id'); return n ? `Refresh ${n}` : 'Refresh app'; }
    case 'capture_app': { const n = str('app_name') || str('app_id'); return n ? `Capture ${n}` : 'Capture app'; }
    case 'request_credential': return str('provider') ? `Request ${str('provider')} credential` : 'Request credential';
    case 'configure_email': return 'Configure email';
    case 'connect_oauth_account': return str('provider') ? `Connect ${str('provider')}` : 'Connect account';
    case 'navigate_ui': {
      const target = str('target');
      if (target === 'app' || target === 'app-ui') { const n = str('app_name') || str('app_id'); return n ? `Open ${n}` : 'Open app'; }
      if (target === 'file') return str('path') ? `Open ${basename(str('path'))}` : 'Open file';
      if (target === 'url') return str('url') ? `Open ${str('url').split('/').slice(0, 3).join('/')}` : 'Open URL';
      return target ? `Open ${target}` : 'Navigate UI';
    }
    case 'read_notifications': return 'Read notifications';
    case 'enable_push_notifications': return 'Enable push notifications';
    case 'setup_mcp_server': return str('name') ? `Setup MCP: ${str('name')}` : 'Setup MCP server';
    case 'list_mcp_servers': return 'List MCP servers';
    case 'start_mcp_server': return str('name') ? `Start MCP: ${str('name')}` : 'Start MCP server';
    case 'stop_mcp_server': return str('name') ? `Stop MCP: ${str('name')}` : 'Stop MCP server';
    case 'remove_mcp_server': return str('name') ? `Remove MCP: ${str('name')}` : 'Remove MCP server';
    case 'list_apps': return 'List apps';
    case 'list_triggers': return 'List tasks';
    case 'update_trigger': return str('name') ? `Update task: ${str('name')}` : 'Update task';
    case 'delete_trigger': return 'Delete task';
    case 'browser_forget_login': return 'Forget browser login';
    case 'browser_clear_data': return 'Clear browser data';
    case 'run_thread': return str('prompt') ? `Run thread: ${str('prompt').slice(0, 50)}` : 'Run thread';
    case 'generate_image': return str('prompt') ? `Generate image: ${str('prompt').slice(0, 44)}` : 'Generate image';
    case 'manage_repositories': return 'Manage repositories';
    default: { const s = name.replace(/_/g, ' '); return s.charAt(0).toUpperCase() + s.slice(1); }
  }
}

/** @deprecated Fallback for old events without a stored description. New descriptions come from Rust `describe_cc_tool()`. */
function describeCCTool(name: string, args: unknown): string {
  const a = args as Record<string, unknown> | null | undefined;
  const str = (key: string) => (a && typeof a[key] === 'string' ? a[key] as string : '');
  const basename = (p: string) => p.split('/').pop() || p;

  switch (name) {
    case 'Read': return str('file_path') ? `Read ${basename(str('file_path'))}` : 'Read file';
    case 'Edit': return str('file_path') ? `Edit ${basename(str('file_path'))}` : 'Edit file';
    case 'Write': return str('file_path') ? `Write ${basename(str('file_path'))}` : 'Write file';
    case 'MultiEdit': return str('file_path') ? `Edit ${basename(str('file_path'))}` : 'Edit file';
    case 'Glob': return str('pattern') ? `Find ${str('pattern')}` : 'Find files';
    case 'Grep': return str('pattern') ? `Search '${str('pattern')}'` : 'Search code';
    case 'Bash': return str('command') ? describeRun(str('command')) : 'Run command';
    case 'WebFetch': return str('url') ? `Fetch ${str('url').split('/').slice(0, 3).join('/')}` : 'Fetch URL';
    case 'WebSearch': return str('query') ? `Search '${str('query')}'` : 'Web search';
    case 'Agent': return str('description') || 'Run agent';
    case 'Skill': return str('skill') ? `Run skill: ${str('skill')}` : 'Run skill';
    case 'NotebookEdit': return str('file_path') ? `Edit ${basename(str('file_path'))}` : 'Edit notebook';
    default: return name;
  }
}

/** Mark the last pending step (success === null) as completed.
 *  Walks backwards so parallel tool calls resolve in LIFO order as results arrive.
 *  Optional `pred` narrows which pending step to resolve (e.g. only "Thinking" steps). */
function resolveLastPendingStep(
  steps: { success: boolean | null; description?: string }[],
  pred?: (s: { description?: string }) => boolean,
): void {
  for (let i = steps.length - 1; i >= 0; i--) {
    if (steps[i].success === null && (!pred || pred(steps[i]))) {
      steps[i].success = true;
      return;
    }
  }
}

/** Force ALL pending steps to completed.
 *  Called after a completion event so spinners don't persist on finished exchanges. */
function resolvePendingSteps(steps: { success: boolean | null }[]): void {
  for (const step of steps) {
    if (step.success === null) step.success = true;
  }
}

const isThinking = (s: { description?: string }) => s.description === 'Thinking';
const isNotThinking = (s: { description?: string }) => !isThinking(s);

/** Bag of legacy events. All optional — `synthesizeContextCapture`
 *  produces something useful from any subset (Thinking-only is the
 *  oldest case). */
export interface LegacyContextEvents {
  thinking?: { text?: string; context_tokens?: number; context_messages?: number; trimmed?: boolean };
  tokensMeasured?: { input_tokens?: number };
  assembled?: { sections?: ContextSection[]; tools?: string[]; model?: string; total_chars?: number };
}

/** Default context_window for legacy rows: 200k. Pre-ContextCaptured
 *  events never persisted the budget; under-reporting on the 1M-context
 *  Opus fork is preferable to faking headroom. */
const LEGACY_CONTEXT_WINDOW = 200_000;

export function synthesizeContextCapture(legacy: LegacyContextEvents): ContextCapture {
  const usage = legacy.tokensMeasured?.input_tokens != null
    ? {
        input_tokens: legacy.tokensMeasured.input_tokens,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
      }
    : undefined;
  return {
    producer: 'main_llm',
    model: legacy.assembled?.model ?? '',
    context_window: LEGACY_CONTEXT_WINDOW,
    sections: legacy.assembled?.sections ?? [],
    tools: legacy.assembled?.tools ?? [],
    estimated_total_tokens: legacy.thinking?.context_tokens ?? legacy.assembled?.total_chars ?? 0,
    usage,
    trimmed: legacy.thinking?.trimmed ?? false,
    legacy: true,
  };
}

/** Convert a `ContextCaptured` ThreadEvent into the store-side
 *  `ContextCapture` shape (mostly identity — clamps optional fields). */
function capturedEventToData(
  snap: Extract<ThreadEvent, { type: 'ContextCaptured' }>,
): ContextCapture {
  return {
    producer: snap.producer,
    model: snap.model,
    context_window: snap.context_window,
    sections: snap.sections,
    tools: snap.tools ?? [],
    estimated_total_tokens: snap.estimated_total_tokens,
    usage: snap.usage,
    trimmed: snap.trimmed ?? false,
  };
}

/** Pick which step a ContextCaptured snapshot binds to. Main-LLM emits
 *  fire after a `Thinking` step, so they bind there — the inline
 *  `tokens / window (pct%)` chip then renders next to the request. CC
 *  has no per-API-call Thinking step (CC manages its own loop), so a
 *  CC snapshot binds to whichever step is on top of the stack —
 *  typically the tool that just finished. Used by both `exchangeSteps`
 *  and `exchangeResponseEvents` so the inline chip and summary
 *  projection agree on which step owns each snapshot. The caller
 *  supplies `assign` because Step and the ResponseEvent step variant
 *  share the `contextCapture` field but live in different unions. */
function bindSnapshotToStep<T>(
  data: ContextCapture,
  items: T[],
  isStep: (item: T) => boolean,
  isThinking: (item: T) => boolean,
  assign: (item: T, snap: ContextCapture) => void,
): void {
  const acceptable = data.producer === 'claude_code' ? isStep : isThinking;
  for (let i = items.length - 1; i >= 0; i--) {
    if (acceptable(items[i])) {
      assign(items[i], data);
      return;
    }
  }
}

/** Build Step[] from exchange events (tool calls with success tracking).
 *  @param _isLast — kept for caller compatibility; spinners are no longer resolved
 *  on `!isLast` alone. A non-last exchange can still be the one the agentic loop
 *  is actively processing (chat mid-flight injection — the parent's
 *  request_event_id keeps attracting events even after the follow-up MR lands),
 *  so resolution waits for either an in-exchange completion event or `threadIdle`.
 *  @param threadIdle — true when CC is not producing output (see
 *  `isThreadQuiescent` in store.ts). Combined with the in-exchange completion
 *  flag to finalize pending steps. */
export function exchangeSteps(exchange: Exchange, _isLast = true, threadIdle = false): Step[] {
  const steps: Step[] = [];
  let isComplete = false;
  let legacyAcc: LegacyContextEvents = {};
  let lastThinkingIdx = -1;
  const refreshLegacySnapshot = () => {
    if (lastThinkingIdx < 0) return;
    steps[lastThinkingIdx].contextCapture = synthesizeContextCapture(legacyAcc);
  };
  for (const { event } of exchange.steps) {
    switch (event.type) {
      case 'MemorySearched': {
        const results = (event as { results?: number }).results ?? 0;
        steps.push({ description: results > 0 ? 'Memory searched' : 'Memory: no results', success: true });
        break;
      }
      case 'Thinking': {
        const ctx = event as { context_tokens?: number; context_messages?: number; trimmed?: boolean; text?: string };
        legacyAcc = { thinking: ctx };
        steps.push({
          description: 'Thinking',
          success: true,
          context_tokens: ctx.context_tokens,
          context_messages: ctx.context_messages,
          trimmed: ctx.trimmed,
        });
        lastThinkingIdx = steps.length - 1;
        if (ctx.context_tokens != null || ctx.context_messages != null) {
          refreshLegacySnapshot();
        }
        break;
      }
      case 'ContextTokensMeasured': {
        const measured = event as { input_tokens: number };
        legacyAcc.tokensMeasured = measured;
        for (let i = steps.length - 1; i >= 0; i--) {
          if (steps[i].description === 'Thinking') {
            steps[i].context_tokens = measured.input_tokens;
            break;
          }
        }
        refreshLegacySnapshot();
        break;
      }
      case 'ContextAssembled': {
        const ctx = event as { sections: ContextSection[]; tools: string[]; model: string; total_chars: number };
        legacyAcc.assembled = ctx;
        refreshLegacySnapshot();
        break;
      }
      case 'ContextCaptured': {
        bindSnapshotToStep(
          capturedEventToData(event as Extract<ThreadEvent, { type: 'ContextCaptured' }>),
          steps,
          () => true,
          (s) => s.description === 'Thinking',
          (s, snap) => { s.contextCapture = snap; },
        );
        break;
      }
      case 'ToolCalled': {
        const e = event as { name: string; args: unknown; description?: string };
        steps.push({ description: e.description || describeEngineTool(e.name, e.args), success: null });
        break;
      }
      case 'ToolResult':
        resolveLastPendingStep(steps);
        break;
      case 'CodingAgentPromptSent':
        steps.push({ description: 'Thinking', success: null });
        break;
      case 'CodingAgentToolCalled': {
        resolveLastPendingStep(steps, isThinking);
        const e = event as { name: string; args: unknown; description?: string };
        steps.push({ description: e.description || describeCCTool(e.name, e.args), success: null, tool_use_id: toolUseIdOf(event) });
        isComplete = false; // CC resumed — not finished yet
        break;
      }
      case 'CodingAgentToolResult': {
        // tool_use_id is unique per call; description is ambiguous for parallel
        // calls (two `Read SKILL.md` of different files share a row label).
        // Fallback handles AskUserQuestion: its CodingAgentToolCalled is
        // suppressed (run_session.rs) so no step carries its id, and the
        // ToolResult must not resolve the resume-marker Thinking spinner
        // queued by agent_question.rs — hence isNotThinking on the walker.
        const id = toolUseIdOf(event);
        let resolved = false;
        if (id) {
          for (const step of steps) {
            if (step.success === null && step.tool_use_id === id) {
              step.success = true;
              resolved = true;
              break;
            }
          }
        }
        if (!resolved) resolveLastPendingStep(steps, isNotThinking);
        break;
      }
      case 'ResponseGenerated': case 'ResponseCanceled': case 'ResponseAborted': case 'ResponseFailed':
      case 'CodingAgentIdled':
        isComplete = true;
        break;
      case 'CodingAgentTextStreamed':
        resolveLastPendingStep(steps, isThinking);
        isComplete = false; // CC resumed — not finished yet
        break;
    }
  }
  if (isComplete || threadIdle) resolvePendingSteps(steps);
  return steps;
}

/** Count images in an exchange (user-pasted + generated) for thread:N offset computation. */
export function exchangeImageCount(exchange: Exchange): number {
  let count = exchangeUserImageHashes(exchange).length;
  for (const { event } of exchange.steps) {
    if (event.type === 'ToolResult') {
      const imgs = (event as { images?: string[] }).images;
      if (imgs?.length) count += imgs.length;
    }
  }
  return count;
}

/** Mark the last pending step in a ResponseEvent[] as completed and return it
 *  so callers can attach extra payload (tool result text, images). Optional
 *  `pred` narrows which pending step to resolve. */
function resolveLastPendingResponseStep(
  events: ResponseEvent[],
  pred?: (s: { description?: string }) => boolean,
): Extract<ResponseEvent, { type: 'step' }> | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.type === 'step' && e.success === null && (!pred || pred(e))) {
      e.success = true;
      return e;
    }
  }
  return null;
}

/** Build ResponseEvent[] from exchange events (interleaved text + steps for rendering).
 *  `imageOffset` is the number of images in all previous exchanges (for thread:N numbering).
 *  @param _isLast — kept for caller compatibility; no longer drives spinner resolution
 *  on its own. See `threadIdle`.
 *  @param threadIdle — true when CC is not producing output (see
 *  `isThreadQuiescent` in store.ts). Combined with the in-exchange completion
 *  flag to finalize pending steps. A non-last exchange can still be the one
 *  the engine is actively processing (chat mid-flight injection), so
 *  resolution must not trigger purely on `!isLast`. */
export function exchangeResponseEvents(exchange: Exchange, imageOffset = 0, _isLast = true, threadIdle = false): ResponseEvent[] {
  const events: ResponseEvent[] = [];
  const hasCCContent = exchangeHasCCContent(exchange);
  // Count images across the thread for thread:N numbering — starts after user images in this exchange
  let imageCounter = imageOffset + exchangeUserImageHashes(exchange).length;
  let isComplete = false;
  // One ContextAssembled per exchange; attach to every step pushed after it.
  let currentContext: ContextAssembledData | undefined;
  let legacyAcc: LegacyContextEvents = {};
  const attachLegacyToLastThinking = () => {
    for (let i = events.length - 1; i >= 0; i--) {
      const e = events[i];
      if (e.type === 'step' && e.description === 'Thinking') {
        e.contextCapture = synthesizeContextCapture(legacyAcc);
        return;
      }
    }
  };
  const pushStep = (step: Extract<ResponseEvent, { type: 'step' }>) => {
    if (currentContext) step.context = currentContext;
    events.push(step);
  };

  for (const { event } of exchange.steps) {
    const created = event.created;
    switch (event.type) {
      case 'MemorySearched': {
        const ms = event as { results?: number; queries?: string[] };
        const results = ms.results ?? 0;
        const detail = ms.queries?.length ? ms.queries.join(', ') : undefined;
        pushStep({ type: 'step', description: results > 0 ? 'Memory searched' : 'Memory: no results', success: true, detail, created });
        break;
      }
      case 'Thinking': {
        const ctx = event as { context_tokens?: number; context_messages?: number; trimmed?: boolean };
        legacyAcc = { thinking: ctx };
        pushStep({
          type: 'step',
          description: 'Thinking',
          success: true,
          context_tokens: ctx.context_tokens,
          context_messages: ctx.context_messages,
          trimmed: ctx.trimmed,
          created,
        });
        if (ctx.context_tokens != null || ctx.context_messages != null) {
          attachLegacyToLastThinking();
        }
        break;
      }
      case 'ContextTokensMeasured': {
        const measured = event as { input_tokens: number };
        legacyAcc.tokensMeasured = measured;
        for (let i = events.length - 1; i >= 0; i--) {
          const e = events[i];
          if (e.type === 'step' && e.description === 'Thinking') {
            e.context_tokens = measured.input_tokens;
            break;
          }
        }
        attachLegacyToLastThinking();
        break;
      }
      case 'ContextAssembled': {
        const ctx = event as { sections: ContextSection[]; tools: string[]; model: string; total_chars: number };
        legacyAcc.assembled = ctx;
        currentContext = {
          sections: ctx.sections,
          tools: ctx.tools,
          model: ctx.model,
          total_chars: ctx.total_chars,
        };
        attachLegacyToLastThinking();
        break;
      }
      case 'ContextCaptured': {
        bindSnapshotToStep(
          capturedEventToData(event as Extract<ThreadEvent, { type: 'ContextCaptured' }>),
          events,
          (e) => e.type === 'step',
          (e) => e.type === 'step' && e.description === 'Thinking',
          (e, snap) => { if (e.type === 'step') e.contextCapture = snap; },
        );
        break;
      }
      case 'ToolCalled': {
        const e = event as { name: string; args: unknown; description?: string };
        const description = e.description || describeEngineTool(e.name, e.args);
        const full = fullCommandForEngineTool(e.name, e.args);
        pushStep({ type: 'step', description, tool_name: e.name, success: null, full, created });
        break;
      }
      case 'ToolResult': {
        const toolResult = event as { result?: string; images?: string[] };
        const resolved = resolveLastPendingResponseStep(events);
        if (resolved) {
          if (toolResult.result !== undefined) resolved.result = toolResult.result;
          if (toolResult.images?.length) resolved.result_images = toolResult.images;
        }
        // Render generated images inline
        if (toolResult.images?.length) {
          for (const b64 of toolResult.images) {
            imageCounter++;
            events.push({ type: 'image', base64: b64, mime_type: 'image/jpeg', index: imageCounter });
          }
        }
        break;
      }
      case 'TextStreamed':
        events.push({ type: 'text', md: (event as { text: string }).text });
        break;
      case 'SessionStarted':
        if (hasCCContent) events.push({ type: 'section_break', channel: 'claude_code' });
        break;
      case 'CodingAgentPromptSent':
        pushStep({ type: 'step', description: 'Thinking', success: null, created });
        break;
      case 'CodingAgentToolCalled': {
        resolveLastPendingResponseStep(events, isThinking);
        const e = event as { name: string; args: unknown; description?: string };
        const description = e.description || describeCCTool(e.name, e.args);
        const full = fullCommandForCCTool(e.name, e.args);
        pushStep({ type: 'step', description, tool_name: e.name, success: null, tool_use_id: toolUseIdOf(event), full, created });
        isComplete = false; // CC resumed — not finished yet
        break;
      }
      case 'CodingAgentToolResult': {
        // See exchangeSteps for the pairing rationale.
        const id = toolUseIdOf(event);
        const ccResult = (event as { result?: string }).result;
        let resolved = false;
        if (id) {
          for (const e of events) {
            if (e.type === 'step' && e.success === null && e.tool_use_id === id) {
              e.success = true;
              if (ccResult !== undefined) e.result = ccResult;
              resolved = true;
              break;
            }
          }
        }
        if (!resolved) {
          const fallback = resolveLastPendingResponseStep(events, isNotThinking);
          if (fallback && ccResult !== undefined) fallback.result = ccResult;
        }
        break;
      }
      case 'CodingAgentTextStreamed':
        resolveLastPendingResponseStep(events, isThinking);
        events.push({ type: 'text', md: (event as { text: string }).text });
        isComplete = false; // CC resumed — not finished yet
        break;
      case 'CodingAgentUserMessageSent':
        // Legacy event — now an exchange boundary in groupIntoExchanges, never a step
        break;
      case 'ResponseGenerated': case 'ResponseCanceled': case 'ResponseAborted': case 'ResponseFailed':
      case 'CodingAgentIdled':
        isComplete = true;
        break;
      // ChangeApplied/Discarded/Reverted/ApplyFailed and UserQuestionAsked/
      // CodingAgentPermissionRequest/CredentialRequested/McpConsentRequested
      // are exchange-STARTERS (see EXCHANGE_START_TYPES) — they render as their
      // own initiator panels and never reach this loop as steps. The matching
      // resolution events (UserQuestionAnswered, CodingAgentPermissionResolved)
      // become steps of the divider exchange and are handled by describeInitiator
      // from the userEvent's exchange — no per-step ResponseEvent synthesis here.
      case 'SessionEnded':
        break;
    }
  }
  // Resolve pending spinners on finished exchanges (missing ToolResult from
  // killed sessions, parallel tool calls with lost results, or non-last
  // exchanges that were genuinely abandoned). Mid-flight chat injection means
  // a non-last exchange can still be the one the agentic loop is actively
  // processing, so we DON'T resolve purely on `!isLast` — wait for the
  // exchange's terminator OR for the thread to go idle.
  if (isComplete || threadIdle) {
    const stepEvents = events.filter(e => e.type === 'step') as { success: boolean | null }[];
    resolvePendingSteps(stepEvents);
    // Strip trailing Thinking steps — noise from CC processing notifications
    // (e.g., post-ChangeApplied) without producing output. Keep at least one
    // event so canceled/aborted exchanges still show .response-content.
    while (events.length > 1) {
      const last = events[events.length - 1];
      if (last.type === 'step' && isThinking(last)) {
        events.pop();
      } else {
        break;
      }
    }
  }
  return mergeAdjacentTextEvents(events);
}

/** Tri-state mapping for a step's `success` field: null = pending, true =
 *  succeeded, false = failed. Both the inline-step row and the detail modal
 *  consume this — the class drives the icon color, the label is user-facing. */
export function stepStatus(success: boolean | null): { label: string; className: 'pending' | 'success' | 'error' } {
  if (success === null) return { label: 'In progress', className: 'pending' };
  if (success) return { label: 'Completed', className: 'success' };
  return { label: 'Failed', className: 'error' };
}

/** Whether a non-last exchange's response panel should be hidden as visual
 *  noise. The next exchange's user message implies the chronological flow,
 *  so a panel that produced no real output isn't worth a "Continued below ↳"
 *  placeholder.
 *
 *  An exchange counts as empty if it has no response text and every event is
 *  either a bare 'Thinking' step or a text event that contributes no visible
 *  output. CC follow-ups race the user: the loop emits a Thinking marker
 *  (and sometimes a whitespace-only text header) before producing any tool
 *  call or text, leaving an interrupted exchange with stray steps that say
 *  nothing the status indicator doesn't already. */
export function isEmptyContinuedExchange(
  status: ExchangeStatus,
  hasResponse: boolean,
  events: ResponseEvent[],
  isLast: boolean,
): boolean {
  if (isLast) return false;
  if (status !== 'done' && status !== 'interrupted') return false;
  if (hasResponse) return false;
  return events.every(e =>
    (e.type === 'step' && isThinking(e)) || (e.type === 'text' && !isMeaningfulText(e))
  );
}

/** Get the error message from a failed exchange. */
export function exchangeError(exchange: Exchange): string {
  for (const { event } of exchange.steps) {
    if (event.type === 'ResponseFailed') return event.error;
  }
  return '';
}

/** Index of the last (newest) ResponseAborted exchange that has NO later
 *  ContinuationStarted exchange anywhere in the thread. Used by AbortPanel to
 *  decide whether to render the Continue button — only the unresumed abort
 *  shows it; older aborts that the user already continued past are inert.
 *
 *  Stale-settle aborts (engine cleanup of a stuck-but-already-gone process,
 *  fired by the user's Stop/Apply/Discard/Archive/Interrupt click) are
 *  treated like ContinuationStarted: they terminate the scan with `null`.
 *  Clicking Continue would re-run work the user just deliberately stopped. */
export function unresumedAbortIndex(exchanges: Exchange[]): number | null {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const ev = exchanges[i].userEvent;
    if (ev.type === 'ContinuationStarted') return null;
    if (ev.type === 'ResponseAborted') {
      if (ev.cause === 'stale_settle') return null;
      return i;
    }
  }
  return null;
}

/** Read the engine note (UserPromptInjected step) from a ContinuationStarted
 *  exchange. Returns the full text and a coarse count of bullet entries for
 *  the subline ("Reminded the model about N prior tool calls"). Returns null
 *  when no engine note is present (e.g., CC resume path). */
export function resumeEngineNote(exchange: Exchange): { text: string; toolCount: number } | null {
  for (const { event } of exchange.steps) {
    if (event.type === 'UserPromptInjected' && (event as { mode?: ActorMode }).mode === 'engine') {
      const text = (event as { text: string }).text || '';
      // Count bullet lines that look like "- name(args) → result" — the engine
      // note format from chat/rerun.rs::build_side_effect_summary.
      let toolCount = 0;
      for (const line of text.split('\n')) {
        const trimmed = line.trim();
        if (trimmed.startsWith('- ') && trimmed.includes(' → ')) toolCount++;
      }
      return { text, toolCount };
    }
  }
  return null;
}

/** SessionEnded reasons that represent deliberate lifecycle events, NOT system
 *  interruptions. Derived from the generated contract — `shutdown` and `panic`
 *  are system interruptions; `closed` is the user closing a thread (deliberate
 *  but terminal). Removed pre-Phase-4 reasons (`completed`, `changes_proposed`,
 *  `changes_applied`, `auto_ended`, `user_ended`, `stale_resume`, `discarded`)
 *  still appear on legacy DB rows and were considered normal lifecycle ends —
 *  preserved here as plain strings so historical exchanges render the same as
 *  before. */
const NORMAL_SESSION_END_REASONS: ReadonlySet<string> = new Set<string>([
  ...SESSION_END_REASONS.filter(r => r !== 'shutdown' && r !== 'panic'),
  'completed',
  'changes_proposed',
  'changes_applied',
  'auto_ended',
  'user_ended',
  'stale_resume',
  'discarded',
]);

/** Identify ResponseAborted events that have been superseded by a later
 *  same-request_event_id terminal (ResponseGenerated / ResponseFailed). This
 *  models the engine-restart-then-recovered turn: recovery emits an abort,
 *  the rerun re-uses the original request_event_id, and the eventual success
 *  or definitive failure should win the exchange's verdict.
 *
 *  Strict matching: only events with the SAME non-null request_event_id are
 *  paired. Two different ids in the same exchange (or one event missing the
 *  field) do NOT merge — preserving the no-recovery case unchanged. */
function supersededAbortIndices(steps: SequencedEvent[]): Set<number> {
  const superseded = new Set<number>();
  for (let i = 0; i < steps.length; i++) {
    const aborted = steps[i].event;
    if (aborted.type !== 'ResponseAborted') continue;
    const abortReqId = aborted.request_event_id;
    if (!abortReqId) continue;
    for (let j = i + 1; j < steps.length; j++) {
      const later = steps[j].event;
      if (later.type !== 'ResponseGenerated' && later.type !== 'ResponseFailed') continue;
      if (later.request_event_id === abortReqId) {
        superseded.add(i);
        break;
      }
    }
  }
  return superseded;
}

/** Derive ExchangeStatus for an exchange.
 *  @param isLast — true if this is the last (newest) exchange in the thread
 *  @param hasPriorActive — true if a prior exchange is still active (pending/streaming/cc-working),
 *         meaning this exchange is queued behind it
 *  @param threadIdle — true when CC is not producing output (see
 *         `isThreadQuiescent` in store.ts). When true and the exchange has no
 *         terminal event, the exchange was interrupted by an engine crash/lid
 *         close and should show as 'aborted', not 'streaming'. */
export function exchangeStatus(exchange: Exchange, streamingBuffer: string, isLast: boolean, hasPriorActive?: boolean, threadIsCC?: boolean, threadIdle = false): ExchangeStatus {
  let isComplete = false;
  let isCanceled = false;
  let isAborted = false;
  let isFailed = false;
  let isCC = false;
  let isCCWaiting = false;
  let isSessionEnded = false;
  // SessionEnded with a normal lifecycle reason (changes_proposed, completed, etc.)
  // — terminal for CC exchanges even when CodingAgentIdled was skipped (e.g. the
  // engine's auto-harden `continue` path bailed out before emitting it).
  let isSessionEndedNormally = false;
  let isShutdown = false;
  // CC paused on AskUserQuestion. The QuestionCard owns the action surface;
  // the exchange itself reads as "done" so it doesn't show a "Working" spinner
  // while the user thinks. Resume (UserQuestionAnswered followed by CC text)
  // clears this flag and the exchange falls back to cc-working.
  let isWaitingForAnswer = false;
  // Track whether the exchange reached a "completed" state BEFORE any
  // abort/shutdown event. When true, the abort is from a system-injected
  // prompt crash (e.g., auto-harden) and the user's work was already done.
  // This distinguishes "CC completed → auto-harden crashed → ResponseAborted"
  // (should be 'done') from "CC crashed mid-work → ResponseAborted" (should
  // be 'aborted').
  let wasCompleted = false;
  let completedBeforeAbort = false;

  const supersededAborts = supersededAbortIndices(exchange.steps);

  // Divider exchanges (UserQuestionAsked / CodingAgentPermissionRequest as
  // userEvent) start in awaiting-answer until a matching resolution lands as
  // a step. Without seeding here, the steps loop sees only the resolution and
  // never the request, so isWaitingForAnswer stays false for pending dividers.
  const userEventType = exchange.userEvent.type;
  if (userEventType === 'UserQuestionAsked' || userEventType === 'CodingAgentPermissionRequest') {
    isWaitingForAnswer = true;
  }

  for (let i = 0; i < exchange.steps.length; i++) {
    const event = exchange.steps[i].event;
    switch (event.type) {
      case 'ResponseGenerated': isComplete = true; wasCompleted = true; break;
      case 'ResponseCanceled': isCanceled = true; isComplete = true; break;
      case 'ResponseAborted':
        if (supersededAborts.has(i)) break; // superseded by a later same-id terminal
        if (wasCompleted) completedBeforeAbort = true;
        isAborted = true; isComplete = true; break;
      case 'ResponseFailed': isFailed = true; isComplete = true; break;
      case 'SessionStarted':
        isCC = true; isSessionEnded = false; isSessionEndedNormally = false; isShutdown = false;
        break;
      // SessionEnded: deliberate lifecycle endings must NOT flash the
      // "engine restarted" aborted banner, even if isCCWaiting was
      // transiently cleared by a CodingAgentPromptSent (e.g., hardening
      // follow-ups during apply_now). Only `shutdown`/`panic` are system
      // interruptions; everything else (including missing reason from
      // legacy DB rows, coalesced to `completed`) is a normal lifecycle end.
      case 'SessionEnded': {
        const reason = event.reason ?? 'completed';
        if (reason === 'shutdown') {
          if (wasCompleted) completedBeforeAbort = true;
          isShutdown = true;
        }
        if (!NORMAL_SESSION_END_REASONS.has(reason)) {
          isSessionEnded = true;
        } else if (reason !== 'stale_resume') {
          // stale_resume is mid-flight (a fresh SessionStarted follows) — not terminal.
          isSessionEndedNormally = true;
        }
        break;
      }
      case 'CodingAgentIdled': isCCWaiting = true; wasCompleted = true; break;
      // CC work events after waiting → CC resumed, no longer waiting/complete.
      // CodingAgentUserMessageSent resets wasCompleted — a user follow-up in the
      // same exchange (legacy data) means new work was requested.
      case 'CodingAgentUserMessageSent':
        isCCWaiting = false; isComplete = false; wasCompleted = false; break;
      case 'CodingAgentToolCalled':
      case 'CodingAgentTextStreamed':
      case 'CodingAgentPromptSent':
        isCCWaiting = false; isComplete = false; isWaitingForAnswer = false; break;
      case 'UserQuestionAsked':
      case 'CodingAgentPermissionRequest':
        isWaitingForAnswer = true; break;
      case 'UserQuestionAnswered':
      case 'CodingAgentPermissionResolved':
        isWaitingForAnswer = false; break;
    }
  }

  // Follow-up exchanges in a CC thread inherit CC context even without
  // their own SessionStarted event (the session is shared across exchanges).
  if (threadIsCC) isCC = true;

  const hasSteps = exchange.steps.length > 0;
  // Absorbed-UPI placeholder: the engine emitted a UPI carrying this
  // exchange's MR via injected_message_id, so the response actually lives in
  // the prior exchange (req_id-routed there). The placeholder reads as 'done'
  // and is excluded from the 'interrupted' carve-out below ("Continued below"
  // is wrong — the answer is above, not below).
  const onlyStep = exchange.steps.length === 1 ? exchange.steps[0].event : undefined;
  const isAbsorbedUpiPlaceholder = onlyStep?.type === 'UserPromptInjected' && !!onlyStep.injected_message_id;

  if (isFailed) return 'error';
  // Abort/shutdown AFTER the exchange was already completed (e.g., auto-harden
  // crash after CodingAgentIdled/ResponseGenerated) — the user's work was done.
  // System-level crashes after completion don't undo that.
  if ((isAborted || isShutdown) && completedBeforeAbort) return 'done';
  // ResponseAborted event — system-initiated interruption (crash, shutdown, etc.)
  if (isAborted) return 'aborted';
  // Engine shutdown — system-initiated interruption, not user cancel.
  if (isShutdown) return 'aborted';
  if (isCanceled) return 'canceled';
  // Session ended without a proper response = aborted.
  // Chat: no ResponseGenerated. CC: no CodingAgentIdled (was mid-work when killed).
  if (isSessionEnded && !isComplete && !isCCWaiting) return 'aborted';
  // If a prior exchange is still active and this exchange has no events yet,
  // it's queued (waiting for the prior to finish). Must check BEFORE the
  // !isLast→done fallthrough to avoid showing "No response generated".
  // CC threads don't queue — messages go to CC's stdin, not engine queue.
  // Only the LAST queued exchange shows "Queued" — earlier ones were superseded
  // by a newer message and are handled by the empty-non-last rule below
  // (→ 'done', renders as "Continued below ↳").
  if (hasPriorActive && !hasSteps && !isCC && isLast) return 'queued';
  // CC idle → done. WaitingBanner handles the "can interact" state separately.
  if (isCCWaiting) return 'done';
  // CC session ended with a normal reason (changes_proposed, completed, etc.) —
  // terminal even when CodingAgentIdled was missing.
  if (isCC && isSessionEndedNormally) return 'done';
  // CC paused on a user question or permission prompt — render as
  // 'awaiting-answer' so the surrounding spinner stops AND the header reads
  // "Awaiting answer" (not the misleading "Done ✓"). The QuestionCard /
  // PermissionCard inside the exchange shows the action surface.
  if (isWaitingForAnswer) return 'awaiting-answer';
  // Non-last with steps but no terminator: the user moved past this exchange
  // (chat fast-path injects the follow-up via UPI under the parent's
  // request_event_id and redirects later events to the new exchange; CC
  // shares one session across exchanges). Render as 'interrupted' so only
  // the last panel reads "Working".
  if (!isLast && !isComplete && hasSteps && !isAbsorbedUpiPlaceholder) return 'interrupted';
  if (isComplete) return 'done';
  // Non-last CC exchange without a terminator was skipped by CC's msg_tx queue
  // — safely 'done'.
  if (!isLast && (isCC || threadIdle)) return 'done';
  // Empty chat exchange when the engine has gone idle — extends the !isLast
  // empty-→-done rule to the isLast case, so an MR whose response landed in
  // a sibling exchange (off-by-one in the orphan re-process chain — see
  // thread 9b5a05aa) doesn't spin "Requesting" forever. Without `threadIdle`
  // the isLast branch must keep falling through so a freshly-sent MR before
  // the loop has emitted anything still reads as 'pending'/'Requesting'.
  // Relies on chat's request_event_id serialization invariant: by the time an
  // exchange is non-last, the loop has already moved past it (mid-flight
  // injection routes new events back to the parent's request_event_id, so a
  // non-last exchange the loop is still actively processing has steps).
  if (!hasSteps && !isCC && (!isLast || threadIdle)) return 'done';
  // CC exchanges are 'cc-working' once they have steps, 'pending' before.
  if (isCC) return hasSteps ? 'cc-working' : 'pending';
  if (streamingBuffer) return 'streaming';

  // Absorbed-UPI placeholder: handled here for the isLast case (the
  // !isLast branch above bypasses 'interrupted' for it). Must run before
  // the threadIdle stale-detector to avoid a false 'aborted'.
  if (isAbsorbedUpiPlaceholder) return 'done';

  // Stale exchange: thread DB says idle but exchange has no terminal event and
  // no live streaming buffer. This happens when the engine crashed or lid closed
  // mid-response — the agentic loop died without emitting ResponseGenerated or
  // ResponseAborted. Detect this BEFORE the streaming fallbacks so we show
  // "Aborted" instead of an eternal "Working" spinner.
  // hasSteps covers both tool calls AND TextStreamed events (both are in exchange.steps).
  if (threadIdle && isLast && !isComplete && hasSteps) return 'aborted';

  // Persisted response text (TextStreamed events) without a completion event
  // means the response is still in progress — the streaming buffer was just
  // cleared by a persisted event arrival. Show 'streaming', not 'done'.
  const responseText = exchangeResponseText(exchange);
  if (responseText) return 'streaming';

  const steps = exchangeSteps(exchange, isLast, threadIdle);
  const events = exchangeResponseEvents(exchange, 0, isLast, threadIdle);
  if (steps.length > 0 || events.length > 0) return 'streaming';

  return 'pending';
}

// ---------------------------------------------------------------------------
// Exchange grouping
// ---------------------------------------------------------------------------

/** Compute exchanges for a thread, merging any pending user messages as
 *  synthetic MessageReceived events. Pure function — no signal dependencies. */
export function computeExchanges(thread: ThreadState): Exchange[] {
  if (thread.pendingUserMessages.length === 0) {
    return groupIntoExchanges(thread.events);
  }
  // Merge pending messages as synthetic MessageReceived events so they act as
  // proper exchange boundaries. MAX_SAFE_INTEGER seqs sort them after all real events.
  //
  // CHAT threads: Don't set `created` — chat messages are queued, so events after
  // the pending timestamp are still from the CURRENT request. Without `created`, sort
  // falls through to seq comparison. Use `_displayCreated` for display timestamps.
  //
  // CC threads: Keep `created` — follow-ups are delivered immediately, so events
  // after the follow-up ARE responses to it. Timestamp-based sorting correctly
  // splits events between old and new exchanges.
  const augmented = new Map(thread.events);
  const isCC = thread.meta.channel === 'claude_code';
  for (let i = 0; i < thread.pendingUserMessages.length; i++) {
    const pending = thread.pendingUserMessages[i];
    const syntheticSeq = Number.MAX_SAFE_INTEGER - thread.pendingUserMessages.length + i;
    augmented.set(syntheticSeq, {
      type: 'MessageReceived' as const,
      text: pending.text,
      channel: thread.meta.channel,
      ...(isCC ? { created: pending.created } : { _displayCreated: pending.created }),
      ...(pending.image_hashes?.length ? { user_image_hashes: pending.image_hashes } : {}),
    } as StoredEvent);
  }
  return groupIntoExchanges(augmented);
}

/** Event types that begin a new exchange in the timeline. Includes user-initiated
 *  events (MessageReceived, UserPromptInjected), system-initiated events that
 *  spawn a fresh round of work (engine restart, auto-hardening, auto-merge),
 *  the abort/resume boundary pair, change lifecycle events
 *  (apply/discard/revert/fail), the ActionRequired family
 *  (UserQuestionAsked, CodingAgentPermissionRequest, CredentialRequested,
 *  McpConsentRequested) — each agent pause is its own auditable boundary with
 *  an actor, not a step inside the prior agent response — and ChildThreadCompleted
 *  where a sidequest result lands in the parent as a rich card. */
const EXCHANGE_START_TYPES: ReadonlySet<string> = new Set([
  'MessageReceived',
  'TriggerStarted',
  'ResponseAborted',
  'ResponseCanceled',
  'ContinuationStarted',
  'UserPromptInjected',
  'MissingHardeningDetected',
  'MergeConflictDetected',
  'ChangeApplied',
  'ChangeDiscarded',
  'ChangeReverted',
  'ChangeApplyFailed',
  'UserQuestionAsked',
  'CodingAgentPermissionRequest',
  'CredentialRequested',
  'McpConsentRequested',
  'ChildThreadCompleted',
]);

export function isExchangeStartEvent(type: string): boolean {
  return EXCHANGE_START_TYPES.has(type);
}

/** Pure thread-level metadata events that don't belong to any exchange.
 *  Without this filter, an event arriving after a follow-up MR has started a
 *  new (still-empty) exchange leaks into that exchange's steps via the
 *  `current.steps.push` fallthrough — breaking the absorbed-UPI single-step
 *  shape that exchangeStatus relies on to short-circuit to 'done'.
 *  ThreadArchived is excluded automatically: it's classified terminal in the
 *  generated contract, not metadata. Derived from EVENT_CLASSIFICATION so a
 *  new Thread* metadata event added in Rust is picked up without an edit. */
const THREAD_LEVEL_METADATA_EVENTS: ReadonlySet<string> = new Set(
  Object.entries(EVENT_CLASSIFICATION)
    .filter(([evt, cls]) => cls === 'metadata' && evt.startsWith('Thread'))
    .map(([evt]) => evt)
);

/** True if the thread contains at least one event that could contribute to
 *  rendered content. Used to distinguish a legitimately empty thread (only
 *  lifecycle metadata) from a thread with content events that failed to form
 *  exchanges (true corruption). Sourced from the Rust-generated
 *  `EVENT_CLASSIFICATION`: anything not classified as 'metadata' (or unknown
 *  to the contract) counts as content. */
export function hasContentEvents(events: Map<number, StoredEvent>): boolean {
  for (const event of events.values()) {
    if (EVENT_CLASSIFICATION[event.type] !== 'metadata') return true;
  }
  return false;
}

/** Find an existing exchange to absorb `event` into instead of starting a new one.
 *
 *  Two convergent paths:
 *  1. Engine resume note — UPI emitted by chat/rerun.rs right after ContinuationStarted
 *     belongs as a step under the resume initiator. A Human-mode UPI in the same
 *     position is a real correction and stays its own exchange.
 *  2. Mid-flight injection — chat fast-path emits MessageReceived first (with the
 *     client UUID) then sends the injection; the agentic loop later emits UPI
 *     carrying that UUID in `injected_message_id`. Without absorption the user
 *     sees a duplicate "Auto-prompt sent" panel below their own message.
 *
 *  Returns null when the event is not absorbable, or when an injection's partner
 *  is missing — the caller falls back to starting a new exchange so the UPI still
 *  renders rather than vanishing. */
function findAbsorbTarget(
  current: Exchange | null,
  exchanges: Exchange[],
  event: StoredEvent,
): Exchange | null {
  if (event.type !== 'UserPromptInjected') return null;
  if (event.mode === 'engine'
      && current
      && current.userEvent.type === 'ContinuationStarted') {
    return current;
  }
  if (event.injected_message_id) {
    return exchanges.find(ex =>
      ex.userEvent.type === 'MessageReceived' && ex.userEvent._eventId === event.injected_message_id,
    ) ?? null;
  }
  return null;
}

/** Chat-loop events whose `request_event_id` should route them to their
 *  originating exchange. Excludes `CodingAgent*` because CC reuses one session
 *  across many follow-ups and never re-anchors the field — routing CC events
 *  by request id would push every follow-up's work back into the first MR.
 *
 *  Response* events are dual-purpose (chat AND CC emit them). For CC they
 *  carry the session's persistent req_id (same reason CodingAgent* events do),
 *  so they're filtered out by `shouldRouteByRequestId` when channel is CC.
 *
 *  Every event the chat agentic loop stamps with `meta.request_event_id` must
 *  appear here — anything missing falls through to the `current` pointer in
 *  `groupIntoExchanges` and silently leaks into a follow-up MR's empty
 *  exchange when the loop's events arrive after the follow-up was emitted.
 *  That leak flipped `exchangeStatus` to 'aborted' for the follow-up via the
 *  `threadIdle && hasSteps && !isComplete` branch — observed on real thread
 *  9b5a05aa where a stray ContextAssembled landed in the empty MR exchange. */
const REQUEST_ID_ROUTED_TYPES: ReadonlySet<string> = new Set([
  'Thinking',
  'MemorySearched',
  'ContextAssembled',
  'ContextTokensMeasured',
  'ToolCalled',
  'ToolResult',
  'TextStreamed',
  'ResponseGenerated',
  'ResponseCanceled',
  'ResponseAborted',
  'ResponseFailed',
]);

/** Skip req_id routing for Response* terminals when their channel is CC: the
 *  session's persistent meta carries the original MR's req_id for the entire
 *  session, so routing back by id would push a mid-flight cancel/abort to the
 *  original exchange instead of terminating the active follow-up. */
function shouldRouteByRequestId(event: StoredEvent): boolean {
  if (!REQUEST_ID_ROUTED_TYPES.has(event.type)) return false;
  switch (event.type) {
    case 'ResponseGenerated':
    case 'ResponseCanceled':
    case 'ResponseAborted':
    case 'ResponseFailed':
      return event.channel !== 'claude_code';
    default:
      return true;
  }
}

/** Read `request_event_id` from any event payload. The field is added to the
 *  wire payload by Rust's `EventMeta::apply()` regardless of the event type,
 *  so the cast is honest about what arrives at runtime. */
function requestEventIdOf(event: { type: string }): string | undefined {
  return (event as { request_event_id?: string }).request_event_id;
}

/** Read `tool_use_id` from a CodingAgentTool* event payload. Empty string in
 *  legacy DB rows from before the field existed — normalize to `undefined`. */
function toolUseIdOf(event: { type: string }): string | undefined {
  const id = (event as { tool_use_id?: string }).tool_use_id;
  return id ? id : undefined;
}

/** Find an exchange by its anchor `_eventId`. Backward walk so an id collision
 *  resolves to the most recent owner. */
function findExchangeByAnchorId(exchanges: Exchange[], anchorId: string): Exchange | null {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    if (exchanges[i].userEvent._eventId === anchorId) return exchanges[i];
  }
  return null;
}

/** Sort events chronologically by `created` timestamp, falling back to seq for events
 *  missing timestamps. The fallback exists because the global BIGSERIAL sequence is
 *  not guaranteed to match wall-clock order across concurrent writes. */
export function sortEventsChronologically(
  events: Map<number, StoredEvent>,
): SequencedEvent[] {
  return [...events.entries()]
    .sort(([aSeq, aEvt], [bSeq, bEvt]) => {
      if (aEvt.created && bEvt.created) {
        const cmp = aEvt.created.localeCompare(bEvt.created);
        if (cmp !== 0) return cmp;
      }
      return aSeq - bSeq;
    })
    .map(([seq, event]) => ({ seq, event }));
}

export function groupIntoExchanges(events: Map<number, StoredEvent>): Exchange[] {
  const sorted = sortEventsChronologically(events);

  // Legacy rerun-in-place: when a ResponseAborted shares request_event_id
  // with a later ResponseGenerated/ResponseFailed in the same thread, the
  // rerun re-used the original exchange (pre-Phase-5.3 behavior). Don't split
  // at those aborts — supersededAbortIndices in exchangeStatus deflates the
  // verdict to the later success.
  //
  // Single forward pass: record request_event_ids of every later resolving
  // terminal first, then mark aborts that match. O(N) instead of O(N²).
  const resolvedReqIds = new Set<string>();
  for (const { event } of sorted) {
    if (event.type !== 'ResponseGenerated' && event.type !== 'ResponseFailed') continue;
    const reqId = requestEventIdOf(event);
    if (reqId) resolvedReqIds.add(reqId);
  }
  const legacySupersededAbortSeqs = new Set<number>();
  for (const { seq, event } of sorted) {
    if (event.type !== 'ResponseAborted') continue;
    const reqId = requestEventIdOf(event);
    if (reqId && resolvedReqIds.has(reqId)) legacySupersededAbortSeqs.add(seq);
  }

  const exchanges: Exchange[] = [];
  let current: Exchange | null = null;
  // tool_use_id → exchange that owns the matching CodingAgentToolCalled step.
  // Populated as calls are appended (always via the default `current.steps.push`
  // branch — CodingAgent* events aren't absorbed or request-id routed) and
  // queried when a CodingAgentToolResult lands so we can re-route it to the
  // call's exchange even if a permission request boundary intervened.
  const toolCallOwners = new Map<string, Exchange>();
  // request_event_id → redirect target exchange. Set when a UPI is absorbed
  // mid-flight: the loop emits the UPI when it actually ingests the queued
  // follow-up, so every event after that point is part of the answer to the
  // absorbed prompt — not the original request. Without redirecting, the
  // post-injection tools and the final ResponseGenerated all stay in the
  // original exchange and the follow-up panel renders as an empty stub.
  const reqIdRedirect = new Map<string, Exchange>();

  for (const { seq, event } of sorted) {
    if (THREAD_LEVEL_METADATA_EVENTS.has(event.type)) continue;

    // Legacy rerun-in-place aborts stay in the originating exchange as a
    // step so supersededAbortIndices in exchangeStatus can deflate the
    // verdict and the rerun's TextStreamed/ResponseGenerated render in
    // the same response panel — never split, never start a fresh boundary.
    const isLegacySupersededAbort =
      event.type === 'ResponseAborted' && legacySupersededAbortSeqs.has(seq);

    const reqId = shouldRouteByRequestId(event) ? requestEventIdOf(event) : undefined;
    const owner = reqId
      ? (reqIdRedirect.get(reqId) ?? findExchangeByAnchorId(exchanges, reqId))
      : null;

    // ResponseAborted is dual-purpose: it terminates the originating exchange
    // (so the partial-response panel reads 'Aborted ⚠') AND opens a new
    // boundary exchange whose userEvent is the abort itself, rendered as the
    // AbortPanel. The boundary always sits chronologically last so the panel
    // appears below any newer MessageReceived in the timeline.
    if (event.type === 'ResponseAborted' && !isLegacySupersededAbort) {
      const target = owner ?? current;
      if (target && target.userEvent.type !== 'ResponseAborted') {
        target.steps.push({ seq, event });
        current = { userEvent: event, userSeq: seq, steps: [] };
        exchanges.push(current);
        continue;
      }
    }
    // ResponseCanceled mirrors the abort dual-purpose pattern: keep the
    // cancel as a step on the originating exchange (so its response panel
    // reads 'Canceled ✕') AND open a new boundary exchange so a separate
    // 'You — Canceled the response' panel renders below the truncated reply.
    if (event.type === 'ResponseCanceled') {
      const target = owner ?? current;
      if (target && target.userEvent.type !== 'ResponseCanceled') {
        target.steps.push({ seq, event });
        current = { userEvent: event, userSeq: seq, steps: [] };
        exchanges.push(current);
        continue;
      }
    }
    // Re-route ToolResult by tool_use_id when a permission boundary stranded
    // it from its call's exchange. Legacy events (no id) fall through.
    if (event.type === 'CodingAgentToolResult') {
      const id = toolUseIdOf(event);
      const callOwner = id ? toolCallOwners.get(id) : undefined;
      if (callOwner && callOwner !== current) {
        callOwner.steps.push({ seq, event });
        continue;
      }
    }
    const absorbTarget = findAbsorbTarget(current, exchanges, event);
    if (absorbTarget) {
      absorbTarget.steps.push({ seq, event });
      current = absorbTarget;
      if (event.type === 'UserPromptInjected') {
        const absorbedReqId = requestEventIdOf(event);
        if (absorbedReqId) reqIdRedirect.set(absorbedReqId, absorbTarget);
      }
    } else if (isExchangeStartEvent(event.type) && !isLegacySupersededAbort) {
      current = { userEvent: event, userSeq: seq, steps: [] };
      exchanges.push(current);
    } else if (event.type === 'CodingAgentUserMessageSent') {
      // Legacy: old data has this instead of MessageReceived for CC follow-ups.
      // New data emits both MessageReceived and CodingAgentUserMessageSent for the same
      // user message — skip creating a duplicate exchange if one already exists.
      if (current && current.userEvent.type === 'MessageReceived' && current.steps.length === 0) {
        // MessageReceived already started this exchange — skip the duplicate
        continue;
      }
      const text = (event as { text: string }).text;
      current = { userEvent: { type: 'MessageReceived', text } as StoredEvent, userSeq: seq, steps: [] };
      exchanges.push(current);
    } else if (event.type === 'CodingAgentPromptSent' && !current) {
      // Legacy engine-spawned CC threads (merge-conflict, hardening) created
      // before MergeConflictDetected/MissingHardeningDetected boundary events
      // existed emit a bare CodingAgentPromptSent as the first content event.
      // Promote it to a synthetic boundary so the panel renders — without this
      // every following step is dropped and the thread shows the "Messages
      // could not be displayed" empty state. Modern threads always have a
      // proper boundary first, so `current` is non-null and we fall through
      // to the step branch below.
      current = { userEvent: event, userSeq: seq, steps: [] };
      exchanges.push(current);
    } else if (owner) {
      owner.steps.push({ seq, event });
    } else if (current) {
      current.steps.push({ seq, event });
      if (event.type === 'CodingAgentToolCalled') {
        const id = toolUseIdOf(event);
        if (id) toolCallOwners.set(id, current);
      }
    }
  }
  return exchanges;
}

export function handleEvent(
  threadMap: Map<string, ThreadState>,
  threadId: string,
  seq: number | null,
  event: ThreadEvent | TransientEvent,
  created?: string,
  eventId?: string,
  aggregate?: ThreadAggregate,
): boolean {
  const thread = threadMap.get(threadId);
  if (!thread) return false;

  // Backend-computed snapshot is the source of truth for thread.meta. Live
  // SSE attaches a per-event aggregate on persisted events; transient events
  // (e.g. ChildrenCountChanged from fanout) may also carry one when the
  // backend updated other projection fields out-of-band. fetchThreadEvents
  // replay applies a single currentAggregate after the loop (in
  // applyEventRows), so per-row calls here legitimately have no aggregate.
  if (aggregate) {
    const prevStatus = thread.meta.status;
    applyAggregateToMeta(thread.meta, aggregate);
    if (thread.meta.status === 'running' && prevStatus !== 'running' && created) {
      thread.meta.lastRevivedAt = created;
    }
  }

  if (seq !== null) {
    if (thread.events.has(seq)) return false;
    if (!created) {
      console.warn(`[handleEvent] persisted event ${event.type} (seq=${seq}) missing created timestamp — this indicates a backend bug`);
    }
    const stored: StoredEvent = { ...(event as ThreadEvent), created, ...(eventId ? { _eventId: eventId } : {}) };
    thread.events.set(seq, stored);
    thread.streamingBuffer = '';
    // Update updatedAt only for events that the backend updates last_activity for.
    // Must stay in sync with update_thread_projection() in event_bus.rs.
    if (created && updatesLastActivity(event.type)) thread.meta.updatedAt = created;
    // When a real MessageReceived event arrives from the backend,
    // remove the matching optimistic pending message by event_id (UUID).

    if ((event.type === 'MessageReceived' || event.type === 'UserPromptInjected') && thread.pendingUserMessages.length > 0) {
      if (eventId) {
        const idx = thread.pendingUserMessages.findIndex(p => p.eventId === eventId);
        if (idx !== -1) thread.pendingUserMessages.splice(idx, 1);
      } else {
        // Fallback for events without event_id (e.g. scheduled tasks, old data):
        // remove the oldest pending message (FIFO order)
        thread.pendingUserMessages.shift();
      }
    }
    // FreeText answers don't emit a MessageReceived (the backend routes typed
    // text straight to UserQuestionAnswered), so the optimistic pending message
    // added by sendMessage() must be cleared here too. Match by text — the
    // backend forwards user input verbatim. A non-match indicates drift; let
    // the safety timer clean it up rather than silently shifting the wrong one.
    if (event.type === 'UserQuestionAnswered' && event.answer.kind === 'FreeText' && thread.pendingUserMessages.length > 0) {
      const text = event.answer.text;
      const idx = thread.pendingUserMessages.findIndex(p => p.text === text);
      if (idx !== -1) thread.pendingUserMessages.splice(idx, 1);
    }
  } else {
    if ('text' in event) {
      thread.streamingBuffer += event.text;
    }
    // Transient events (streaming text, tool calls) represent active work —
    // update updatedAt so the thread list timestamp stays current during
    // long-running CC sessions. No flicker risk: transient events are never
    // metadata (ThreadTitleGenerated etc.), and on reload CodingAgentIdled
    // (a persisted activity event) provides the correct final timestamp.
    if (created) thread.meta.updatedAt = created;
  }
  return true;
}

/** Synthesize a `MessageOrigin` for older DB rows that don't have one stamped.
 *  Returns undefined when the event has neither device_id nor parent_thread_id
 *  (the panel then falls back to a minimal "Unknown" line). New events written
 *  after this feature shipped always carry an explicit `origin`; this helper
 *  exists so the panel can render coherent content for historical exchanges. */
export function legacyOrigin(
  event: Extract<ThreadEvent, { type: 'MessageReceived' }>,
): MessageOrigin | undefined {
  if (event.origin) return event.origin;
  const initiator = modeToInitiator(event.mode);
  if (initiator === 'system') {
    return event.parent_thread_id
      ? { kind: 'thread_link', thread_id: event.parent_thread_id, spawning_event_id: event.spawning_event_id, mode: event.mode === 'engine' ? 'engine' : 'agent', direction: 'parent' }
      : undefined;
  }
  if (event.device_id) {
    return { kind: 'device', device_id: event.device_id, label: event.device ?? 'Unknown device' };
  }
  return undefined;
}
