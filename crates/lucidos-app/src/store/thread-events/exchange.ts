import { MODELS, REASONING_LEVELS } from '../models';
import { chatModels } from '../store';
import { ENGINE_LABEL, RESPONSE_CANCELED_SUMMARY, continuationStartedSummary, isChildNote, responseAbortedSummary } from './thread-event-types';
import { CC_ACTIVITY_EVENTS } from './thread-meta';
import type { ActorMode, SequencedEvent, StoredEvent, ThreadEvent, ThreadInitiator } from './thread-event-types';
import type { CodingAgent } from '../../generated/thread-event-wire';

export type TypedAnswer =
  | { state: 'sending'; text: string; image_hashes: string[] }
  | { state: 'unsent'; text: string; image_hashes: string[]; unsentEventId: string };

export type Exchange = {
  userEvent: StoredEvent;
  userSeq: number;
  steps: SequencedEvent[];
  questionOvertaken?: boolean;
  cardWaiting?: boolean;
  continuationFragment?: true;
  continuationMoved?: boolean;
  tookTheTurn?: boolean;
  inputRead?: StoredEvent;
  awaitingRead?: true;
  typedAnswer?: TypedAnswer;
  releasedFromHold?: true;
  deliveredHeldIds?: Set<string>;
  blockedStepSeqs?: Set<number>;
  deniedStepSeqs?: Set<number>;
  liveReplyText?: string;
  revision?: number;
};

export function exchangeKey(exchange: Exchange): string {
  const id = exchange.userEvent._eventId;
  return id ? `id:${id}` : `seq:${exchange.userSeq}`;
}

export type AnsweredQuestion = Extract<StoredEvent, { type: 'UserQuestionAnswered' }>;

export type ResolvedPermission = Extract<StoredEvent, { type: 'CodingAgentPermissionResolved' }>;

export type ResolvedCommandPermission = Extract<StoredEvent, { type: 'CommandPermissionResolved' }>;

export type ResolvedMcpPermission = Extract<StoredEvent, { type: 'McpPermissionResolved' }>;

export function findQuestionAnswer(exchange: Exchange, toolUseId: string): AnsweredQuestion | undefined {
  for (const { event } of exchange.steps) {
    if (event.type === 'UserQuestionAnswered' && event.tool_use_id === toolUseId) return event;
  }
  return undefined;
}

export function findPermissionResolution(exchange: Exchange, requestId: string): ResolvedPermission | undefined {
  for (const { event } of exchange.steps) {
    if (event.type === 'CodingAgentPermissionResolved' && event.request_id === requestId) return event;
  }
  return undefined;
}

export function findCommandPermissionResolution(
  exchange: Exchange,
  requestId: string,
): ResolvedCommandPermission | undefined {
  for (const { event } of exchange.steps) {
    if (event.type === 'CommandPermissionResolved' && event.request_id === requestId) return event;
  }
  return undefined;
}

export function findMcpPermissionResolution(
  exchange: Exchange,
  requestId: string,
): ResolvedMcpPermission | undefined {
  for (const { event } of exchange.steps) {
    if (event.type === 'McpPermissionResolved' && event.request_id === requestId) return event;
  }
  return undefined;
}


export function isUserBubbleEvent(userEvent: { type: string }): boolean {
  return userEvent.type === 'MessageReceived' || userEvent.type === 'SpokenMessageReceived'
    || userEvent.type === 'UserOnlyMessagePosted' || userEvent.type === 'CodingAgentCommandReceived';
}

export function drawsInitiatorPanel(exchange: Exchange): boolean {
  return exchange.continuationFragment !== true && exchange.userEvent.type !== 'AgentSignInRequested'
    && !isResumeTurn(exchange) && !isSuccessorSessionOpening(exchange);
}

function isSuccessorSessionOpening({ userEvent, steps }: Exchange): boolean {
  return userEvent.type === 'MessageReceived'
    && userEvent.origin?.kind === 'engine'
    && userEvent.origin.reason.kind === 'successor_session'
    && steps.some(({ event }) => event.type === 'SessionStarted' && event.continuity?.kind === 'handoff');
}

export function isResumeTurn(exchange: Exchange): boolean {
  return exchange.userEvent.type === 'ContinuationStarted';
}

export function exchangeUserMessage(exchange: Exchange): string {
  const ev = exchange.userEvent;
  if (ev.type === 'TriggerStarted') {
    return ev.prompt || ev.trigger_name || '';
  }
  if (ev.type === 'ContinuationStarted') {
    return continuationStartedSummary(ev.reason, ev.actor);
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

export type ChangeLifecycleType =
  | 'ChangeApplied' | 'ChangeDiscarded' | 'ChangeReverted' | 'ChangeApplyFailed';

export type ChangeLifecycleEvent = Extract<ThreadEvent, { type: ChangeLifecycleType }>;

const CHANGE_LIFECYCLE_TYPES: ReadonlySet<string> = new Set([
  'ChangeApplied', 'ChangeDiscarded', 'ChangeReverted', 'ChangeApplyFailed',
]);

export function isChangeLifecycleEvent(event: { type: string }): event is ChangeLifecycleEvent {
  return CHANGE_LIFECYCLE_TYPES.has(event.type);
}

export function changePanelHasContinuation(exchange: Exchange): boolean {
  if (exchange.steps.some(({ event }) => event.type === 'SpokenReplyGenerated')) return true;
  if (exchange.userEvent.type === 'ChangeApplyFailed' || !isChangeLifecycleEvent(exchange.userEvent)) return false;
  return exchange.steps.some(({ event }) =>
    event.type === 'CodingAgentTextStreamed'
    || event.type === 'CodingAgentToolCalled'
    || event.type === 'TextStreamed'
    || event.type === 'ToolCalled',
  );
}

export function modeToInitiator(mode: ActorMode | undefined): ThreadInitiator {
  return mode === 'agent' || mode === 'engine' ? 'system' : 'user';
}

export function exchangeUserImageHashes(exchange: Exchange): string[] {
  const type = exchange.userEvent.type;
  if (!isUserBubbleEvent(exchange.userEvent) || type === 'SpokenMessageReceived') return [];
  const raw = (exchange.userEvent as { user_image_hashes?: unknown }).user_image_hashes;
  if (!Array.isArray(raw)) return [];
  return raw.filter((h): h is string => typeof h === 'string');
}

type ResponseField = 'model' | 'reasoning_effort';
function extractResponseField(
  exchange: Exchange,
  field: ResponseField,
  midTurn: (event: StoredEvent) => string | undefined,
): string | undefined {
  let ccFallback: string | undefined;
  for (let i = exchange.steps.length - 1; i >= 0; i--) {
    const event = exchange.steps[i].event;
    if (event.type === 'ResponseGenerated' || event.type === 'ResponseCanceled' || event.type === 'ResponseAborted') {
      const v = event[field];
      if (v) return v;
    }
    ccFallback ??= midTurn(event);
  }
  if (ccFallback) return ccFallback;
  const starter = exchange.userEvent;
  if (starter.type === 'MessageReceived' || starter.type === 'TriggerStarted') {
    const v = starter[field];
    if (v) return v;
  }
  return undefined;
}

function spawnSettings(event: StoredEvent): Extract<StoredEvent, { type: 'CodingAgentSettingsChanged' }> | undefined {
  return event.type === 'CodingAgentSettingsChanged' && !(event as { actor?: unknown }).actor ? event : undefined;
}

export function exchangeResponseModel(exchange: Exchange): string | undefined {
  return extractResponseField(exchange, 'model', (event) => spawnSettings(event)?.model || undefined);
}

export function exchangeReasoningEffort(exchange: Exchange): string | undefined {
  return extractResponseField(exchange, 'reasoning_effort', (event) => {
    if (event.type === 'ContextCaptured') {
      return parentToolUseIdOf(event) ? undefined : event.reasoning_effort || undefined;
    }
    return spawnSettings(event)?.reasoning_effort || undefined;
  });
}

export function sessionCodingAgents(exchanges: Exchange[], threadBackend: CodingAgent): CodingAgent[] {
  const backends: CodingAgent[] = [];
  let current: CodingAgent | undefined;
  for (const exchange of exchanges) {
    for (const { event } of exchange.steps) {
      if (event.type !== 'SessionStarted') continue;
      const backend = event.coding_agent ?? 'claude-code';
      if (current === undefined) {
        backends.fill(event.continuity?.kind === 'handoff' ? threadBackend : backend);
      }
      current = backend;
    }
    backends.push(current ?? threadBackend);
  }
  return backends;
}

const STATIC_MODEL_LABELS: Record<string, string> = Object.fromEntries([
  ...MODELS.map(m => [m.value, m.label]),
  ['claude-opus-4-8@default', 'Opus 4.8'],
  ['claude-opus-4-8@default[1m]', 'Opus 4.8 (1M)'],
  ['claude-opus-4-7', 'Opus 4.7'],
  ['claude-opus-4-7[1m]', 'Opus 4.7 (1M)'],
  ['claude-sonnet-4-6', 'Sonnet 4.6'],
  ['claude-sonnet-4-6[1m]', 'Sonnet 4.6 (1M)'],
  ['claude-opus-4-6', 'Opus 4.6'],
  ['claude-opus-4-6[1m]', 'Opus 4.6 (1M)'],
  ['claude-opus-4-5@20251101', 'Opus 4.5'],
  ['gpt-5.5', 'GPT-5.5'],
  ['gpt-5.4', 'GPT-5.4'],
  ['gpt-5.3-codex', 'GPT-5.3 Codex'],
  ['gpt-5.2-codex', 'GPT-5.2 Codex'],
  ['gpt-5.3-codex-spark', 'Codex Spark'],
  ['claude-opus-4-1', 'Opus 4.1'],
  ['claude-opus-4-8[1m]', 'Opus 4.8 (1M)'],
  ['claude-opus-4-8', 'Opus 4.8'],
  ['claude-fable-5', 'Fable 5'],
  ['claude-fable-5[1m]', 'Fable 5 (1M)'],
  ['claude-opus-5', 'Opus 5'],
  ['claude-opus-5[1m]', 'Opus 5 (1M)'],
  ['claude-sonnet-5', 'Sonnet 5'],
  ['claude-sonnet-5[1m]', 'Sonnet 5 (1M)'],
  ['claude-fable-5-1[1m]', 'Fable 5.1 (1M)'],
  ['claude-opus-5-5[1m]', 'Opus 5.5 (1M)'],
  ['claude-sonnet-5-5[1m]', 'Sonnet 5.5 (1M)'],
  ['claude-haiku-4-5', 'Haiku 4.5'],
  ['gemini-3-flash-preview', 'Gemini 3 Flash'],
  ['gpt-5.6-sol', 'GPT-5.6 Sol'],
  ['gpt-5.6-terra', 'GPT-5.6 Terra'],
  ['gpt-5.6-luna', 'GPT-5.6 Luna'],
  ['gpt-5.5-pro', 'GPT-5.5 Pro'],
  ['gpt-5.4-mini', 'GPT-5.4 mini'],
  ['claude-opus-5@default', 'Opus 5'],
  ['claude-opus-5@default[1m]', 'Opus 5 (1M)'],
  ['claude-haiku-4-5-20251001', 'Haiku 4.5'],
  ['claude-haiku-4-5@20251001', 'Haiku 4.5'],
  ['opus', 'Opus (latest)'],
  ['opus[1m]', 'Opus (latest, 1M)'],
  ['sonnet', 'Sonnet (latest)'],
  ['sonnet[1m]', 'Sonnet 4.6 (1M)'],
  ['haiku', 'Haiku 4.5'],
]);

export function displayModelName(modelId: string): string {
  const loaded = chatModels.value;
  if (loaded.status === 'loaded') {
    const m = loaded.data.find(x => x.id === modelId);
    if (m) return m.label;
  }
  return STATIC_MODEL_LABELS[modelId] ?? modelId;
}

const EFFORT_LABELS: Record<string, string> = Object.fromEntries(
  REASONING_LEVELS.map(l => [l.value, l.label]),
);

export function displayReasoningEffort(effort: string): string {
  return EFFORT_LABELS[effort] ?? effort;
}

export function exchangeTimestamp(exchange: Exchange): string {
  return exchange.userEvent.created
    || exchange.userEvent._displayCreated
    || new Date().toISOString();
}

const NON_RESPONSE_STEP_TYPES: ReadonlySet<string> = new Set([
  'ThreadArchived',
  'SessionEnded',
  'FormRequestResolved',
]);

export function exchangeResponseTimestamp(exchange: Exchange): string | undefined {
  for (let i = exchange.steps.length - 1; i >= 0; i--) {
    const { event } = exchange.steps[i];
    if (isChildNote(event) || NON_RESPONSE_STEP_TYPES.has(event.type)) continue;
    if (event.created) return event.created;
  }
  return undefined;
}

export function exchangeHasCCContent(exchange: Exchange): boolean {
  return exchange.steps.some(({ event }) => CC_ACTIVITY_EVENTS.has(event.type));
}

export function parentToolUseIdOf(event: { type: string }): string | undefined {
  return (event as { parent_tool_use_id?: string }).parent_tool_use_id || undefined;
}

export function apiCallIdOf(event: { type: string }): string | undefined {
  return (event as { api_call_id?: string }).api_call_id || undefined;
}

export function isAgentReplyText(event: { type: string }): boolean {
  return event.type === 'TextStreamed'
    || (event.type === 'CodingAgentTextStreamed' && !parentToolUseIdOf(event));
}

export function exchangeResponseText(exchange: Exchange): string {
  let text = '';
  for (const { event } of exchange.steps) {
    if (isAgentReplyText(event)) text += (event as { text: string }).text;
  }
  const quietReply = exchangeQuietReply(exchange);
  if (quietReply === null) return text;
  return text.trim() ? `${text}\n\n${quietReply}` : quietReply;
}

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

export function exchangeQuietReply(exchange: Exchange): string | null {
  let afterQuietPick = false;
  let streamed = '';
  for (const { event } of exchange.steps) {
    if (event.type === 'UserQuestionAnswered') {
      afterQuietPick = event.quiet === true;
      streamed = '';
    } else if (!afterQuietPick) {
      continue;
    } else if (isAgentReplyText(event)) {
      streamed += (event as { text: string }).text;
    } else if (event.type === 'ResponseGenerated') {
      const text = event.text ?? '';
      const drawn = text.trim() && text.trim() !== streamed.trim() && !text.startsWith(ENGINE_LIMIT_PREFIX);
      return drawn ? text : null;
    }
  }
  return null;
}
