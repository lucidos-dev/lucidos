import { MODELS, REASONING_LEVELS } from '../models';
import { ENGINE_LABEL, RESPONSE_CANCELED_SUMMARY, originMode, responseAbortedSummary } from './thread-event-types';
import { CC_ACTIVITY_EVENTS } from './thread-meta';
import type { ActorMode, SequencedEvent, StoredEvent, ThreadEvent, ThreadInitiator } from './thread-event-types';

export type Exchange = {
  userEvent: StoredEvent;
  userSeq: number;
  steps: SequencedEvent[];
  /** True on divider exchanges (`userEvent.type === 'UserQuestionAsked'`)
   *  where the agent kept emitting progression events past the question
   *  without an answer (CC's parallel-tool-call race). Undefined on
   *  non-divider exchanges and on divider exchanges that have neither
   *  progression nor a matching answer yet. */
  questionOvertaken?: boolean;
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
    // Same event covers three triggers: real engine restart recovery,
    // watchdog auto-recovery, and the user clicking Continue after a
    // safety-net abort. `originMode` collapses the device/api-with-
    // human-mode cases into 'human' — using it here (vs hand-checking
    // `kind === 'device' || 'api'`) keeps the API-with-mode='engine'
    // path on the engine-restart wording, which is what an engine-
    // driven REST Continue actually is.
    return originMode(ev.actor) === 'human'
      ? 'Continued the response'
      : 'Resumed after engine restart';
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
 *  paths emit ResponseAborted with model=null). Claude Code sessions fall back to
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
  // Claude Code session short aliases — `CodingAgentSettingsChanged.model` carries
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

/** If this exchange's terminator was the chat agent's per-turn cap, return the
 *  message body (the prose after "[ENGINE-LIMIT] "). Otherwise empty string.
 *
 *  The cap is emitted as `ResponseGenerated { text: "[ENGINE-LIMIT] …" }` by
 *  emit_iteration_cap_response_generated in agentic_loop.rs — with NO preceding
 *  TextStreamed, so the text never lands in exchangeResponseText. The UI needs
 *  this side channel to render the "cap reached" banner; without it the user
 *  just sees the agent silently stop. */
const ENGINE_LIMIT_PREFIX = '[ENGINE-LIMIT]';
export function exchangeEngineLimitDetail(exchange: Exchange): string {
  for (let i = exchange.steps.length - 1; i >= 0; i--) {
    const event = exchange.steps[i].event;
    if (event.type !== 'ResponseGenerated') continue;
    const text = event.text;
    if (text && text.startsWith(ENGINE_LIMIT_PREFIX)) {
      return text.slice(ENGINE_LIMIT_PREFIX.length).trim();
    }
    return '';
  }
  return '';
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
export function describeEngineTool(name: string, args: unknown): string {
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
    case 'create_trigger': return str('name') ? `Schedule: ${str('name')}` : 'Create trigger';
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
    case 'list_triggers': return 'List triggers';
    case 'update_trigger': return str('name') ? `Update trigger: ${str('name')}` : 'Update trigger';
    case 'delete_trigger': return 'Delete trigger';
    case 'browser_forget_login': return 'Forget browser login';
    case 'browser_clear_data': return 'Clear browser data';
    case 'run_thread': return str('prompt') ? `Run thread: ${str('prompt').slice(0, 50)}` : 'Run thread';
    case 'generate_image': return str('prompt') ? `Generate image: ${str('prompt').slice(0, 44)}` : 'Generate image';
    case 'manage_repositories': return 'Manage repositories';
    default: { const s = name.replace(/_/g, ' '); return s.charAt(0).toUpperCase() + s.slice(1); }
  }
}

/** @deprecated Fallback for old events without a stored description. New descriptions come from Rust `describe_cc_tool()`. */
export function describeCCTool(name: string, args: unknown): string {
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
