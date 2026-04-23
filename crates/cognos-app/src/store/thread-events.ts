import { EVENT_CLASSIFICATION, LAST_ACTIVITY_EVENTS, SECTION_TRANSITIONS, SESSION_END_REASONS, STATUS_TRANSITIONS } from '../generated/thread-lifecycle';
import type { EventChannel, SessionEndReason, StatusTransition } from '../generated/thread-lifecycle';
import { MODELS, REASONING_LEVELS } from './models';

/** Who started a thread: user-initiated or system-initiated (e.g. scheduled task). */
export type ThreadInitiator = 'user' | 'system';

/** Mirrors the Rust `ActorMode` enum (lowercase strings). */
export type ActorMode = 'human' | 'agent' | 'engine';

/** Mirrors the Rust `EngineReason` enum (serde tag = "kind", snake_case). */
export type EngineReason =
  | { kind: 'session_recovered' }
  | { kind: 'orphan_recovery' }
  | { kind: 'scheduler'; trigger_id: string; trigger_name?: string }
  | { kind: 'harden_retrigger' }
  | { kind: 'stale_session' };

/** Mirrors the Rust `MessageOrigin` enum (serde tag = "kind", snake_case). */
export type MessageOrigin =
  | { kind: 'device'; device_id: string; label: string }
  | { kind: 'api'; user_agent?: string }
  | {
      kind: 'workspace';
      workspace: string;
      thread_id?: string;
      event_id?: string;
      user_agent?: string;
      mode?: ActorMode;
    }
  | {
      kind: 'parent_thread';
      thread_id: string;
      title?: string;
      spawning_event_id?: string;
      mode?: ActorMode;
    }
  | { kind: 'engine'; reason: EngineReason };

/** Display label for system-initiated work attributed to the engine. */
export const ENGINE_LABEL = 'Lucidos Engine';

// Persisted thread events — stored in DB, appear in snapshots.
// Optional fields (`?`) allow older DB rows (before the field was added) to deserialize safely.
export type ThreadEvent =
  | { type: 'MessageReceived'; text: string; channel?: EventChannel; images?: unknown[]; device_id?: string; device?: string; image_description?: string; sender?: ThreadInitiator; source?: ThreadInitiator; model?: string; reasoning_effort?: string; parent_thread_id?: string; spawning_event_id?: string; origin?: MessageOrigin }
  | { type: 'TextStreamed'; text: string }
  | { type: 'Thinking'; text: string; context_tokens?: number; context_messages?: number; trimmed?: boolean }
  | { type: 'MemorySearched'; results?: number; queries?: string[] }
  | { type: 'ToolCalled'; name: string; args: unknown; description?: string }
  | { type: 'ToolResult'; name: string; result: string; images?: string[] }
  | { type: 'ResponseGenerated'; text?: string; images?: string[]; model?: string; reasoning_effort?: string }
  | { type: 'ResponseCanceled'; text?: string; images?: string[]; model?: string; reasoning_effort?: string }
  | { type: 'ResponseAborted'; text?: string; images?: string[]; model?: string; reasoning_effort?: string }
  | { type: 'ResponseFailed'; error: string }
  | { type: 'SessionStarted'; session_id: string; branch?: string }
  | { type: 'SessionRecovered'; branch?: string; origin?: MessageOrigin }
  | { type: 'SessionEnded'; reason?: SessionEndReason }
  | { type: 'CodingAgentTextStreamed'; text: string }
  | { type: 'CodingAgentToolCalled'; name: string; args: unknown; description?: string }
  | { type: 'CodingAgentToolResult'; name: string; result: string }
  | { type: 'CodingAgentUserMessageSent'; text: string }
  | { type: 'CodingAgentPromptSent'; text: string; origin?: MessageOrigin }
  | { type: 'MissingHardeningDetected' }
  | { type: 'CodingAgentIdled'; has_changes?: boolean; requires_restart?: boolean; is_external_repo?: boolean; cc_session_id?: string }
  | { type: 'ThreadTitleGenerated'; title: string }
  | { type: 'ThreadTitleRenamed'; title: string; actor?: MessageOrigin }
  | { type: 'ThreadPinned'; actor?: MessageOrigin }
  | { type: 'ThreadUnpinned'; actor?: MessageOrigin }
  | { type: 'ThreadMarkedRead' }
  | { type: 'ThreadMarkedUnread' }
  | { type: 'ThreadDismissed'; actor?: MessageOrigin }
  | { type: 'TriggerStarted'; trigger_id: string; trigger_name?: string; prompt?: string; invocation?: TriggerInvocation; origin?: MessageOrigin }
  | { type: 'TriggerCompleted'; trigger_id: string; trigger_name?: string; result_summary?: string }
  | { type: 'ChangeProposed'; change_id?: string; description?: string; files?: string[]; requires_restart?: boolean; path?: string; diff?: string; origin?: MessageOrigin }
  | { type: 'ChangeApplied'; change_id?: string; requires_restart?: boolean; client_update?: boolean; commits?: string[]; thread_title?: string; actor?: MessageOrigin; path?: string }
  | { type: 'ChangeDiscarded'; change_id?: string; actor?: MessageOrigin; path?: string }
  | { type: 'ChangeReverted'; change_id?: string; actor?: MessageOrigin; path?: string }
  | { type: 'ChangeApplyFailed'; change_id?: string; error?: string; actor?: MessageOrigin }
  | { type: 'MergeConflictDetected'; change_id?: string; files?: string[] }
  | { type: 'UserPromptInjected'; text: string }
  | { type: 'CredentialRequested'; provider: string }
  | { type: 'McpConsentRequested'; tool: string; args: unknown }
  | { type: 'CodingAgentSettingsChanged'; model?: string; reasoning_effort?: string; permission_mode?: string }
  | { type: 'UserQuestionAsked'; tool_use_id: string; cc_session_id: string; question: string; options?: QuestionOption[] }
  | { type: 'UserQuestionAnswered'; tool_use_id: string; answer: AnswerKind }
  | { type: 'CodingAgentPermissionRequest'; request_id: string; tool_use_id: string; tool_name: string; input: Record<string, unknown>; summary: string }
  | { type: 'CodingAgentPermissionResolved'; request_id: string; allowed: boolean; reason?: string };

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

/** Mirrors the Rust `AnswerKind` (serde tag = "kind"). */
export type AnswerKind =
  | { kind: 'Selected'; option_id: string }
  | { kind: 'FreeText'; text: string }
  | { kind: 'Canceled' };

// Transient events — live SSE only, never stored
export type TransientEvent =
  // Streaming state (present participle)
  | { type: 'TextStreaming'; text: string }
  | { type: 'Retrying'; reason: string }
  | { type: 'PreambleCompleting' }
  // Side-effect commands — trigger frontend modals/actions
  | { type: 'CredentialRequest'; payload: string }
  | { type: 'EmailConfirmRequest'; payload: string }
  | { type: 'PushNotificationRequest' }
  | { type: 'McpConsentRequest'; data: string }
  | { type: 'RefreshFile'; path: string }
  | { type: 'RefreshAppUI'; app_id: string }
  | { type: 'CaptureAppUI'; app_id: string; request_id: string }
  | { type: 'NavigationRequested'; payload: string }
  | { type: 'CodingAgentThreadSpawned'; cc_thread_id: string; title: string }
  | { type: 'ChildrenCountChanged'; active: number; total: number };

export type StoredEvent = ThreadEvent & { created?: string; _displayCreated?: string };

/** Events that define (or redefine) a thread's channel/source. */
export function isChannelDefiningEvent(eventType: string): boolean {
  return eventType === 'SessionStarted'
    || eventType === 'SessionRecovered'
    || eventType === 'TriggerStarted';
}

export type SequencedEvent = {
  seq: number;
  event: StoredEvent;
};

/** Thread section as stored in the DB projection (thread_summaries.section).
 *  'default' = history/pinned, 'unread' = needs user attention (both chat and CC). */
export type ThreadSection = 'default' | 'unread';

export type ThreadMeta = {
  id: string;
  title: string;
  channel: EventChannel | 'error_unknown_channel';
  initiator: ThreadInitiator;
  pinned: boolean;
  createdAt: string;
  updatedAt: string;
  unread: boolean;
  /** Thread status computed by the backend: 'idle', 'running', or 'waiting'. */
  status: ThreadStatus;
  /** Exchange count from the API (message_count). Used for drawer display
   *  before events are lazy-loaded. Once eventsLoaded is true, the real
   *  count from the events map takes precedence. */
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
  /** Set when sender=System on the initial MessageReceived. */
  parentThreadId?: string;
  parentThreadTitle?: string;
};

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
  pendingUserMessages: Array<{ text: string; eventId: string; created: string; images?: Array<{ base64: string; mime_type: string }> }>;
};

export type ThreadStatus = 'idle' | 'running' | 'waiting' | 'waiting_for_user_answer' | 'failed';

/** Sort threads by updatedAt descending (most recent first). */
export const byRecent = (a: ThreadState, b: ThreadState): number =>
  b.meta.updatedAt.localeCompare(a.meta.updatedAt);

/** Sort review threads: ccHasChanges first, then most recent. */
export const byReviewOrder = (a: ThreadState, b: ThreadState): number => {
  const aHas = a.meta.ccHasChanges ? 1 : 0;
  const bHas = b.meta.ccHasChanges ? 1 : 0;
  if (bHas !== aHas) return bHas - aHas;
  return byRecent(a, b);
};

/** Whether this event type updates the thread's last_activity in the backend
 *  projection (event_bus.rs). Generated from thread_lifecycle.rs. */
function updatesLastActivity(type: string): boolean {
  return LAST_ACTIVITY_EVENTS.has(type);
}

/** Update thread meta.status and CC fields from a persisted SSE event.
 *  Generated transitions from thread_lifecycle.rs — matches event_bus.rs. */
function updateStatusFromEvent(thread: ThreadState, event: ThreadEvent | TransientEvent): void {
  // Stale resume is an internal retry — the user's message is still being
  // processed in a fresh session, so keep status as-is (running).
  // Must match event_bus.rs:1006 which skips the DB status update.
  if (event.type === 'SessionEnded' && event.reason === 'stale_resume') return;

  const transition: StatusTransition | undefined = STATUS_TRANSITIONS[event.type];
  if (!transition) return;

  const meta = thread.meta;

  // Apply CC flag rule FIRST — conditional_cc status checks depend on updated flags.
  // The SQL backend SETs cc_has_changes from the payload and uses the same $2 param
  // in the status CASE. We must update flags before the status check to match.
  if (transition.ccFlags) {
    switch (transition.ccFlags.kind) {
      case 'clear_all':
        meta.ccHasChanges = false;
        meta.ccRequiresRestart = false;
        meta.ccIsExternalRepo = false;
        meta.ccApplying = false;
        break;
      case 'set_changes':
        meta.ccHasChanges = true;
        break;
      case 'set_applying':
        meta.ccApplying = true;
        break;
      case 'clear_applying':
        meta.ccApplying = false;
        break;
      case 'from_payload':
        // SET (not OR) — CodingAgentIdled is the authoritative snapshot.
        // After apply/discard, has_changes=false clears the flag.
        meta.ccHasChanges = !!(event as { has_changes?: boolean }).has_changes;
        meta.ccRequiresRestart = !!(event as { requires_restart?: boolean }).requires_restart;
        meta.ccIsExternalRepo = !!(event as { is_external_repo?: boolean }).is_external_repo;
        break;
    }
  }

  // Apply status rule (after CC flags so conditional_cc sees the correct state)
  switch (transition.status.kind) {
    case 'set':
      meta.status = transition.status.status;
      break;
    case 'conditional_cc':
      meta.status = meta.ccHasChanges ? transition.status.withChanges : transition.status.withoutChanges;
      break;
    case 'no_change':
      break;
  }

  // Special case: SessionEnded with reason='discarded' clears all CC flags.
  // This depends on payload content, not just event type, so it can't be in the generated data.
  if (event.type === 'SessionEnded' && event.reason === 'discarded') {
    meta.ccHasChanges = false;
    meta.ccRequiresRestart = false;
    meta.ccIsExternalRepo = false;
    meta.ccApplying = false;
    meta.status = 'idle';
  }
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

// ---------------------------------------------------------------------------
// Exchange-level derived data — standalone functions on Exchange.
// These extract rendering data from an Exchange's events.
// Used directly by components and tests.
// ---------------------------------------------------------------------------

import type { ExchangeStatus } from './exchange-status';
import { mergeAdjacentTextEvents } from './event-rendering';
import type { Step, ResponseEvent } from './types';

/** Derive the user message text from an exchange. */
export function exchangeUserMessage(exchange: Exchange): string {
  const ev = exchange.userEvent;
  if (ev.type === 'TriggerStarted') {
    return ev.prompt || ev.trigger_name || '';
  }
  if (ev.type === 'SessionRecovered') {
    return 'Session resumed after engine restart';
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
  if (t === 'SessionRecovered' || t === 'MissingHardeningDetected' || t === 'MergeConflictDetected') {
    return 'claude_code';
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

/** Who sent the user event. `source` is the legacy wire name kept for old DB rows. */
export function exchangeUserSource(exchange: Exchange): ThreadInitiator {
  const ev = exchange.userEvent;
  if (ev.type === 'MessageReceived') return ev.sender ?? ev.source ?? 'user';
  return isSystemExchange(exchange) ? 'system' : 'user';
}

/** Whether this exchange was system-initiated (auto-recovery, auto-hardening,
 *  auto-merge, scheduled trigger, change lifecycle) rather than user-initiated. */
export function isSystemExchange(exchange: Exchange): boolean {
  const ev = exchange.userEvent;
  return ev.type === 'SessionRecovered' || ev.type === 'TriggerStarted'
    || ev.type === 'MissingHardeningDetected' || ev.type === 'MergeConflictDetected'
    || isChangeLifecycleEvent(ev);
}

export interface UserImage {
  base64: string;
  mimeType: string;
}

/** Extract user-pasted images from the exchange's MessageReceived event. */
export function exchangeUserImages(exchange: Exchange): UserImage[] {
  if (exchange.userEvent.type !== 'MessageReceived') return [];
  const raw = (exchange.userEvent as { images?: unknown[] }).images;
  if (!Array.isArray(raw)) return [];
  return raw
    .filter((img): img is { base64: string; mime_type: string } =>
      typeof img === 'object' && img !== null && 'base64' in img && 'mime_type' in img
    )
    .map(img => ({ base64: img.base64, mimeType: img.mime_type }));
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
  // Legacy short aliases from events stored before the migration.
  ['opus', 'Opus 4.6'],
  ['sonnet', 'Sonnet 4.6'],
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
export function exchangeHasCCContent(exchange: Exchange): boolean {
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

/** Build Step[] from exchange events (tool calls with success tracking).
 *  @param isLast — true if this is the last (newest) exchange. Non-last exchanges
 *  resolve pending spinners even without a completion event, since they were
 *  implicitly interrupted by the next user message.
 *  @param threadIdle — true if the thread's DB status is 'idle'. Forces resolution
 *  of pending steps since the exchange is no longer actively processing. */
export function exchangeSteps(exchange: Exchange, isLast = true, threadIdle = false): Step[] {
  const steps: Step[] = [];
  let isComplete = false;
  for (const { event } of exchange.steps) {
    switch (event.type) {
      case 'MemorySearched': {
        const results = (event as { results?: number }).results ?? 0;
        steps.push({ description: results > 0 ? `Memory: ${results} results` : 'Memory: no results', success: true });
        break;
      }
      case 'Thinking': {
        const ctx = event as { context_tokens?: number; context_messages?: number; trimmed?: boolean };
        steps.push({
          description: 'Thinking',
          success: true,
          context_tokens: ctx.context_tokens,
          context_messages: ctx.context_messages,
          trimmed: ctx.trimmed,
        });
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
        steps.push({ description: e.description || describeCCTool(e.name, e.args), success: null });
        isComplete = false; // CC resumed — not finished yet
        break;
      }
      case 'CodingAgentToolResult':
        resolveLastPendingStep(steps);
        break;
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
  if (isComplete || !isLast || threadIdle) resolvePendingSteps(steps);
  return steps;
}

/** Count images in an exchange (user-pasted + generated) for thread:N offset computation. */
export function exchangeImageCount(exchange: Exchange): number {
  let count = exchangeUserImages(exchange).length;
  for (const { event } of exchange.steps) {
    if (event.type === 'ToolResult') {
      const imgs = (event as { images?: string[] }).images;
      if (imgs?.length) count += imgs.length;
    }
  }
  return count;
}

/** Mark the last pending step in a ResponseEvent[] as completed.
 *  Optional `pred` narrows which pending step to resolve. */
function resolveLastPendingResponseStep(
  events: ResponseEvent[],
  pred?: (s: { description?: string }) => boolean,
): void {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.type === 'step' && e.success === null && (!pred || pred(e))) {
      e.success = true;
      return;
    }
  }
}

/** Build ResponseEvent[] from exchange events (interleaved text + steps for rendering).
 *  `imageOffset` is the number of images in all previous exchanges (for thread:N numbering).
 *  @param isLast — true if this is the last (newest) exchange. Non-last exchanges
 *  resolve pending spinners even without a completion event. */
export function exchangeResponseEvents(exchange: Exchange, imageOffset = 0, isLast = true): ResponseEvent[] {
  const events: ResponseEvent[] = [];
  const hasCCContent = exchangeHasCCContent(exchange);
  // Count images across the thread for thread:N numbering — starts after user images in this exchange
  let imageCounter = imageOffset + exchangeUserImages(exchange).length;
  let isComplete = false;

  for (const { event } of exchange.steps) {
    switch (event.type) {
      case 'MemorySearched': {
        const ms = event as { results?: number; queries?: string[] };
        const results = ms.results ?? 0;
        const detail = ms.queries?.length ? ms.queries.join(', ') : undefined;
        events.push({ type: 'step', description: results > 0 ? `Memory: ${results} results` : 'Memory: no results', success: true, detail });
        break;
      }
      case 'Thinking': {
        const ctx = event as { context_tokens?: number; context_messages?: number; trimmed?: boolean };
        events.push({
          type: 'step',
          description: 'Thinking',
          success: true,
          context_tokens: ctx.context_tokens,
          context_messages: ctx.context_messages,
          trimmed: ctx.trimmed,
        });
        break;
      }
      case 'ToolCalled': {
        const e = event as { name: string; args: unknown; description?: string };
        events.push({ type: 'step', description: e.description || describeEngineTool(e.name, e.args), tool_name: e.name, success: null });
        break;
      }
      case 'ToolResult': {
        resolveLastPendingResponseStep(events);
        // Render generated images inline
        const toolImages = (event as { images?: string[] }).images;
        if (toolImages?.length) {
          for (const b64 of toolImages) {
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
        events.push({ type: 'step', description: 'Thinking', success: null });
        break;
      case 'CodingAgentToolCalled': {
        resolveLastPendingResponseStep(events, isThinking);
        const e = event as { name: string; args: unknown; description?: string };
        events.push({ type: 'step', description: e.description || describeCCTool(e.name, e.args), tool_name: e.name, success: null });
        isComplete = false; // CC resumed — not finished yet
        break;
      }
      case 'CodingAgentToolResult':
        resolveLastPendingResponseStep(events);
        break;
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
      // ChangeApplied/Discarded/Reverted/ApplyFailed are exchange-STARTERS
      // (see EXCHANGE_START_TYPES) — they render as their own initiator panels
      // and never reach this loop as steps.
      case 'UserQuestionAsked': {
        const e = event as { tool_use_id: string; question: string; options?: QuestionOption[] };
        events.push({
          type: 'question',
          tool_use_id: e.tool_use_id,
          question: e.question,
          options: e.options ?? [],
        });
        // CC was killed waiting for the answer — exchange is not "done" yet.
        isComplete = false;
        break;
      }
      case 'UserQuestionAnswered': {
        // Find the matching question card in this exchange and mark it resolved.
        const e = event as { tool_use_id: string; answer: AnswerKind };
        for (let i = events.length - 1; i >= 0; i--) {
          const ev = events[i];
          if (ev.type === 'question' && ev.tool_use_id === e.tool_use_id) {
            (ev as { resolved?: AnswerKind }).resolved = e.answer;
            break;
          }
        }
        break;
      }
      case 'CodingAgentPermissionRequest': {
        const e = event as { request_id: string; tool_use_id: string; tool_name: string; input: Record<string, unknown>; summary: string };
        events.push({
          type: 'permission',
          request_id: e.request_id,
          tool_use_id: e.tool_use_id,
          tool_name: e.tool_name,
          input: e.input,
          summary: e.summary,
        });
        // CC's tool call is blocking on the engine — exchange isn't done yet.
        isComplete = false;
        break;
      }
      case 'CodingAgentPermissionResolved': {
        const e = event as { request_id: string; allowed: boolean; reason?: string };
        for (let i = events.length - 1; i >= 0; i--) {
          const ev = events[i];
          if (ev.type === 'permission' && ev.request_id === e.request_id) {
            (ev as { resolved?: { allowed: boolean; reason?: string } }).resolved = {
              allowed: e.allowed,
              reason: e.reason,
            };
            break;
          }
        }
        break;
      }
      case 'SessionEnded':
        break;
    }
  }
  // Resolve pending spinners on finished exchanges (missing ToolResult from
  // killed sessions, parallel tool calls with lost results, or non-last
  // exchanges where the user sent a new message mid-tool-call).
  if (isComplete || !isLast) {
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

/** Get the error message from a failed exchange. */
export function exchangeError(exchange: Exchange): string {
  for (const { event } of exchange.steps) {
    if (event.type === 'ResponseFailed') return event.error;
  }
  return '';
}

/** Check whether an aborted exchange was caused by an engine restart/shutdown.
 *  Only returns true when SessionEnded with reason 'shutdown' is present.
 *  ResponseAborted alone (CC process crash, stdin write failure, EOF race)
 *  is NOT an engine restart — it's a CC session interruption. */
export function isAbortedByRestart(exchange: Exchange): boolean {
  for (const { event } of exchange.steps) {
    if (event.type === 'SessionEnded' && event.reason === 'shutdown') {
      return true;
    }
  }
  return false;
}

/** SessionEnded reasons that represent deliberate lifecycle events, NOT system
 *  interruptions. These must not trigger the "engine restarted" aborted banner.
 *  Derived from the generated contract — only 'shutdown' and 'panic' are
 *  system interruptions; everything else is a normal lifecycle transition.
 *  Unknown/missing reasons default to potentially-aborted for safety. */
const NORMAL_SESSION_END_REASONS: ReadonlySet<SessionEndReason> = new Set(
  SESSION_END_REASONS.filter(r => r !== 'shutdown' && r !== 'panic'),
);

/** Derive ExchangeStatus for an exchange.
 *  @param isLast — true if this is the last (newest) exchange in the thread
 *  @param hasPriorActive — true if a prior exchange is still active (pending/streaming/cc-working),
 *         meaning this exchange is queued behind it
 *  @param threadIdle — true if the thread's DB status is 'idle' (no active processing).
 *         When true and the exchange has no terminal event, the exchange was interrupted
 *         by an engine crash/lid close and should show as 'aborted', not 'streaming'. */
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

  for (const { event } of exchange.steps) {
    switch (event.type) {
      case 'ResponseGenerated': isComplete = true; wasCompleted = true; break;
      case 'ResponseCanceled': isCanceled = true; isComplete = true; break;
      case 'ResponseAborted':
        if (wasCompleted) completedBeforeAbort = true;
        isAborted = true; isComplete = true; break;
      case 'ResponseFailed': isFailed = true; isComplete = true; break;
      case 'SessionStarted':
        isCC = true; isSessionEnded = false; isSessionEndedNormally = false; isShutdown = false;
        break;
      // SessionEnded: deliberate lifecycle endings must NOT flash the
      // "engine restarted" aborted banner, even if isCCWaiting was
      // transiently cleared by a CodingAgentPromptSent (e.g., hardening
      // follow-ups during apply_now). Only genuine system interruptions
      // and unknown/missing reasons set isSessionEnded.
      case 'SessionEnded': {
        if (event.reason === 'shutdown') {
          if (wasCompleted) completedBeforeAbort = true;
          isShutdown = true;
        }
        if (!event.reason || !NORMAL_SESSION_END_REASONS.has(event.reason)) {
          isSessionEnded = true;
        } else if (event.reason !== 'stale_resume') {
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
      case 'UserQuestionAsked': isWaitingForAnswer = true; break;
      case 'UserQuestionAnswered': isWaitingForAnswer = false; break;
      // Permission prompts mirror UserQuestionAsked — CC's tool call is
      // blocking on user input, so the exchange should read as "done"
      // (PermissionCard owns the action surface) until CC resumes.
      case 'CodingAgentPermissionRequest': isWaitingForAnswer = true; break;
      case 'CodingAgentPermissionResolved': isWaitingForAnswer = false; break;
    }
  }

  // Follow-up exchanges in a CC thread inherit CC context even without
  // their own SessionStarted event (the session is shared across exchanges).
  if (threadIsCC) isCC = true;

  const hasSteps = exchange.steps.length > 0;

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
  // by a newer message and should fall through to 'done' (→ "Continued below ↳").
  if (hasPriorActive && !hasSteps && !isCC && isLast) return 'queued';
  // CC idle → done. WaitingBanner handles the "can interact" state separately.
  if (isCCWaiting) return 'done';
  // CC session ended with a normal reason (changes_proposed, completed, etc.) —
  // terminal even when CodingAgentIdled was missing.
  if (isCC && isSessionEndedNormally) return 'done';
  // CC paused on a user question — render as 'done' so the surrounding
  // spinner stops; the QuestionCard inside the exchange shows the question.
  if (isWaitingForAnswer) return 'done';
  // Non-last CC exchange: interrupted only if mid-work (not completed).
  if (!isLast && isCC && !isComplete && hasSteps) return 'interrupted';
  if (isComplete || !isLast) return 'done';
  // CC exchanges are 'cc-working' once they have steps, 'pending' before.
  if (isCC) return hasSteps ? 'cc-working' : 'pending';
  if (streamingBuffer) return 'streaming';

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

  const steps = exchangeSteps(exchange, isLast);
  const events = exchangeResponseEvents(exchange, 0, isLast);
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
      ...(pending.images?.length ? { images: pending.images } : {}),
    } as StoredEvent);
  }
  return groupIntoExchanges(augmented);
}

/** Event types that begin a new exchange in the timeline. Includes user-initiated
 *  events (MessageReceived, UserPromptInjected), system-initiated events that
 *  spawn a fresh round of work (engine restart, auto-hardening, auto-merge),
 *  and change lifecycle events (apply/discard/revert/fail) — each is its own
 *  auditable system action with an actor, not a step inside a CC response. */
const EXCHANGE_START_TYPES: ReadonlySet<string> = new Set([
  'MessageReceived',
  'TriggerStarted',
  'SessionRecovered',
  'UserPromptInjected',
  'MissingHardeningDetected',
  'MergeConflictDetected',
  'ChangeApplied',
  'ChangeDiscarded',
  'ChangeReverted',
  'ChangeApplyFailed',
]);

export function isExchangeStartEvent(type: string): boolean {
  return EXCHANGE_START_TYPES.has(type);
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

  const exchanges: Exchange[] = [];
  let current: Exchange | null = null;

  for (const { seq, event } of sorted) {
    if (isExchangeStartEvent(event.type)) {
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
    } else if (current) {
      current.steps.push({ seq, event });
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
): boolean {
  const thread = threadMap.get(threadId);
  if (!thread) return false;

  if (seq !== null) {
    if (thread.events.has(seq)) return false;
    if (!created) {
      console.warn(`[handleEvent] persisted event ${event.type} (seq=${seq}) missing created timestamp — this indicates a backend bug`);
    }
    const stored: StoredEvent = { ...(event as ThreadEvent), created };
    thread.events.set(seq, stored);
    thread.streamingBuffer = '';
    // Update updatedAt only for events that the backend updates last_activity for.
    // Must stay in sync with update_thread_projection() in event_bus.rs.
    if (created && updatesLastActivity(event.type)) thread.meta.updatedAt = created;
    // When a real MessageReceived event arrives from the backend,
    // remove the matching optimistic pending message by event_id (UUID).
    // Update section from SSE events
    const newSection = SECTION_TRANSITIONS[event.type];
    if (newSection) {
      thread.meta.section = newSection;
    }
    // Mirror backend status transitions from SSE events.
    // These must match the status updates in event_bus.rs update_thread_projection().
    const prevStatus = thread.meta.status;
    updateStatusFromEvent(thread, event);
    // Track when thread last entered 'running' — used for IN PROGRESS sort order.
    if (thread.meta.status === 'running' && prevStatus !== 'running' && created) {
      thread.meta.lastRevivedAt = created;
    }

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
  const sender = event.sender ?? event.source ?? 'user';
  if (sender === 'system') {
    return event.parent_thread_id
      ? { kind: 'parent_thread', thread_id: event.parent_thread_id, spawning_event_id: event.spawning_event_id, mode: 'agent' }
      : undefined;
  }
  if (event.device_id) {
    return { kind: 'device', device_id: event.device_id, label: event.device ?? 'Unknown device' };
  }
  return undefined;
}
