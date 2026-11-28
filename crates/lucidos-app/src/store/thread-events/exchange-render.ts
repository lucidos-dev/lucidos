import { SESSION_END_REASONS } from '../../generated/thread-lifecycle';
import { hasVisibleText, isMeaningfulText, mergeAdjacentTextEvents } from '../event-rendering';
import { formatTokens } from '../../utils/formatTokens';
import { AWAIT_EVENT_TOOL } from './event-waits';
import { displayModelName, exchangeHasCCContent, exchangeQuietReply, isAgentReplyText, isChangeLifecycleEvent, exchangeResponseText, apiCallIdOf, parentToolUseIdOf } from './exchange';
import { describeCCTool, describeEngineTool, fullCommandForCCTool, fullCommandForEngineTool } from './tool-descriptions';
import { TERMINAL_EVENT_TYPES, VOICE_ONLY_STEP_TYPES, exchangeHoldsNoTurn, isCallBoundary, isLiveCallRow, isLiveReplyRow, isLiveUtteranceRow, isSettledLiveUtterance, isUningestedMessage, opensAwaitingAnswer, toolUseIdOf } from './exchange-grouping';
import { IDLE_ENGINE_RESTART_INTERRUPT_REASON, isChildNote, isEngineDownAbort, isRedirectCancel, isSwitchTeardownAbort, isTurnlessBoundary, isUserStoppedWait, wakesNoAgent } from './thread-event-types';
import { exchangeError } from './exchange-anchors';
import type { ExchangeStatus } from '../exchange-status';
import type { ContextAssembledData, ContextCapture, ContextSection, RecalledMemory, ResponseEvent, Step, StepOutcome } from '../types';
import type { Exchange } from './exchange';
import type { FireRetry, RetrySkipReason, SessionContinuity } from '../../generated/thread-event-wire';
import type { ActorMode, EventSubscription, EventWaitCancelCause, MessageOrigin, SequencedEvent, StoredEvent, ThreadEvent } from './thread-event-types';

export function triggerRunInterruption(exchange: Exchange): Extract<StoredEvent, { type: 'TriggerRunInterrupted' }> | undefined {
  if (exchange.userEvent.type === 'TriggerRunInterrupted') return exchange.userEvent;
  const step = exchange.steps.find(step => step.event.type === 'TriggerRunInterrupted');
  return step?.event.type === 'TriggerRunInterrupted' ? step.event : undefined;
}

const RETRY_SKIP_SENTENCE: Record<RetrySkipReason, string> = {
  already_retried: 'This run was already a retry.',
  fire_completed: 'Its main work had finished. Use Continue to resume the rest.',
  spawned_work: 'Its sub-threads carry on by themselves.',
  too_old: 'It started more than 24 hours ago.',
  later_slot_due: 'Its next scheduled run is already due.',
  newer_fire: 'A newer run of this trigger has started.',
  trigger_unavailable: 'The trigger is paused, disabled or deleted.',
  request_unavailable: 'Lucidos could not find what started it.',
  already_queued: 'Another run of this trigger is already queued.',
};

export function triggerRetrySummary(retry: FireRetry): string {
  return retry.kind === 'queued'
    ? 'Running it again once'
    : `It was not run again. ${RETRY_SKIP_SENTENCE[retry.reason]}`;
}

type StepLike = { outcome: StepOutcome; description?: string; tool_name?: string; call_event_id?: string };

type TerminalKind = null | 'clean' | 'unclean';

function pendingOutcomeFor(terminal: TerminalKind): StepOutcome {
  return terminal === 'unclean' ? 'unfinished' : 'success';
}

function resolveLastPendingStep(
  steps: StepLike[],
  pred?: (s: StepLike) => boolean,
  outcome: StepOutcome = 'success',
): void {
  for (let i = steps.length - 1; i >= 0; i--) {
    if (steps[i].outcome === 'pending' && (!pred || pred(steps[i]))) {
      steps[i].outcome = outcome;
      return;
    }
  }
}

function toolResultOutcome(event: StoredEvent): StepOutcome {
  return (event as { success?: boolean }).success === false ? 'error' : 'success';
}

function answeredBy(
  event: StoredEvent,
  legacy: (s: StepLike) => boolean,
): (s: StepLike) => boolean {
  const callId = (event as { tool_called_event_id?: string }).tool_called_event_id;
  return callId ? s => s.call_event_id === callId && legacy(s) : legacy;
}

function resolvePendingSteps(
  steps: StepLike[],
  outcome: StepOutcome,
  pred?: (s: StepLike) => boolean,
): void {
  for (const step of steps) {
    if (step.outcome === 'pending' && (!pred || pred(step))) step.outcome = outcome;
  }
}

const isLiveStep = (s: StepLike) => s.outcome === 'pending' || s.outcome === 'blocked';

function settleMatchedCallStep(step: StepLike): void {
  if (step.outcome === 'pending' || step.outcome === 'blocked') step.outcome = 'success';
}

function needsLiveThinkingRow(opts: {
  exchange: Exchange;
  isLast: boolean;
  hasCCContent: boolean;
  anyLive: boolean;
}): boolean {
  const { exchange, isLast, hasCCContent, anyLive } = opts;
  if (!isLast || anyLive) return false;
  if (abortTookEngineDown(exchange.userEvent)) return false;
  return hasCCContent
    || !!exchange.inputRead
    || exchange.steps.some(({ event }) => event.type === 'SessionStarted');
}

export const isThinking = (s: StepLike) => s.description === 'Thinking';
const isNotThinking = (s: StepLike) => !isThinking(s);

export function memoryRecalledLabel(results: number): string {
  if (results <= 0) return 'No memories recalled';
  if (results === 1) return 'Recalled 1 memory';
  return `Recalled ${results} memories`;
}
export function compactedLabel(event: Extract<ThreadEvent, { type: 'CodingAgentCompacted' }>): string {
  const { pre_tokens: before, post_tokens: after } = event;
  if (before == null) return 'Compacted its context';
  if (after == null) return `Compacted its context from ${formatTokens(before)} tokens`;
  return `Compacted its context from ${formatTokens(before)} to ${formatTokens(after)} tokens`;
}
export function modelSubstitutedLabel(event: Extract<ThreadEvent, { type: 'CodingAgentModelSubstituted' }>): string {
  const requested = displayModelName(event.requested);
  return `Ran ${displayModelName(event.served)}, not ${requested}: this account cannot use ${requested}`;
}
const isAwaitEventStep = (s: StepLike) => s.tool_name === AWAIT_EVENT_TOOL;

function nameThinkingRow<T extends StepLike>(rows: T[], naming: Partial<T>): boolean {
  for (let i = rows.length - 1; i >= 0; i--) {
    if (rows[i].outcome === 'pending' && isThinking(rows[i])) {
      Object.assign(rows[i], naming);
      return true;
    }
  }
  const last = rows[rows.length - 1];
  if (last && last.outcome === 'success' && isThinking(last)) {
    Object.assign(last, naming);
    last.outcome = naming.outcome ?? 'pending';
    return true;
  }
  return false;
}

function callOutcome(exchange: Exchange, seq: number): StepOutcome {
  if (exchange.deniedStepSeqs?.has(seq)) return 'denied';
  if (exchange.blockedStepSeqs?.has(seq)) return 'blocked';
  return 'pending';
}

function nameThinkingStep(
  events: ResponseEvent[],
  naming: Partial<Extract<ResponseEvent, { type: 'step' }>>,
): boolean {
  const pending = lastPendingStepIndex(events, isThinking);
  if (pending >= 0) {
    Object.assign(events[pending], naming);
    return true;
  }
  const idx = lastStepIndex(events);
  const marker = idx >= 0 ? events[idx] : undefined;
  if (!marker || marker.type !== 'step' || marker.outcome !== 'success' || !isThinking(marker)) {
    return false;
  }
  Object.assign(marker, naming);
  marker.outcome = naming.outcome ?? 'pending';
  events.splice(idx, 1);
  events.push(marker);
  return true;
}

export interface LegacyContextEvents {
  thinking?: { text?: string; context_tokens?: number; context_messages?: number; trimmed?: boolean };
  tokensMeasured?: { input_tokens?: number };
  assembled?: { sections?: ContextSection[]; tools?: string[]; model?: string };
}

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
    estimated_total_tokens: legacy.thinking?.context_tokens ?? 0,
    usage,
    trimmed: legacy.thinking?.trimmed ?? false,
    legacy: true,
  };
}

export function capturedEventToData(
  snap: Extract<ThreadEvent, { type: 'ContextCaptured' }>,
  eventId?: string,
): ContextCapture {
  return {
    producer: snap.producer,
    model: snap.model,
    context_window: snap.context_window,
    sections: snap.sections ?? [],
    tools: snap.tools ?? [],
    estimated_total_tokens: snap.estimated_total_tokens,
    usage: snap.usage,
    trimmed: snap.trimmed ?? false,
    sections_stripped: snap.sections_stripped,
    event_id: eventId,
  };
}

function bindSnapshotToStep<T>(
  data: ContextCapture,
  parent: string | undefined,
  items: T[],
  isStep: (item: T) => boolean,
  isThinking: (item: T) => boolean,
  parentOf: (item: T) => string | undefined,
  captureOf: (item: T) => ContextCapture | undefined,
  assign: (item: T, snap: ContextCapture) => void,
  apiCall: string | undefined,
  apiCallOf: (item: T) => string | undefined,
): void {
  if (apiCall && items.some(item => apiCallOf(item) === apiCall)) return;
  const ownedByAnotherCall = (item: T) =>
    apiCall !== undefined && (apiCallOf(item) !== undefined || captureOf(item) !== undefined);
  const mainLlm = data.producer === 'main_llm';
  const acceptable = mainLlm
    ? isThinking
    : (item: T) => isStep(item) && parentOf(item) === parent && !ownedByAnotherCall(item);
  for (let i = items.length - 1; i >= 0; i--) {
    if (acceptable(items[i])) {
      const held = captureOf(items[i]);
      if (!(mainLlm && held && !held.legacy)) assign(items[i], data);
      return;
    }
  }
}

function bindCapturesByApiCall<T>(
  items: T[],
  captures: ReadonlyMap<string, ContextCapture>,
  apiCallOf: (item: T) => string | undefined,
  assign: (item: T, snap: ContextCapture) => void,
): void {
  for (const item of items) {
    const apiCall = apiCallOf(item);
    const snap = apiCall ? captures.get(apiCall) : undefined;
    if (snap) assign(item, snap);
  }
}

export function exchangeSteps(exchange: Exchange, isLast = true, threadIdle = false): Step[] {
  const steps: Step[] = [];
  let terminal: TerminalKind = null;
  let legacyAcc: LegacyContextEvents = {};
  let lastThinkingIdx = -1;
  const callCaptures = new Map<string, ContextCapture>();
  const refreshLegacySnapshot = () => {
    if (lastThinkingIdx < 0) return;
    steps[lastThinkingIdx].contextCapture = synthesizeContextCapture(legacyAcc);
  };
  for (const { seq, event } of exchange.steps) {
    switch (event.type) {
      case 'MemoryRecalled':
      case 'MemorySearched': {
        const results = (event as { results?: number }).results ?? 0;
        steps.push({ description: memoryRecalledLabel(results), outcome: 'success' });
        break;
      }
      case 'WorkingUnderstandingWritten': {
        steps.push({
          description: 'Updated its working understanding',
          outcome: 'success',
        });
        break;
      }
      case 'CodingAgentCompacted':
        steps.push({ description: compactedLabel(event), outcome: 'success' });
        break;
      case 'CodingAgentModelSubstituted':
        steps.push({ description: modelSubstitutedLabel(event), outcome: 'success' });
        break;
      case 'ThoughtStreamed': {
        resolveLastPendingStep(steps, isThinking);
        const ctx = event as { context_tokens?: number; context_messages?: number; trimmed?: boolean; text?: string };
        legacyAcc = { thinking: ctx };
        steps.push({
          description: 'Thinking',
          outcome: 'pending',
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
        const snap = capturedEventToData(
          event as Extract<ThreadEvent, { type: 'ContextCaptured' }>,
          event._eventId,
        );
        const apiCall = apiCallIdOf(event);
        if (apiCall) callCaptures.set(apiCall, snap);
        bindSnapshotToStep(
          snap,
          parentToolUseIdOf(event),
          steps,
          () => true,
          (s) => s.description === 'Thinking',
          (s) => s.parent_tool_use_id,
          (s) => s.contextCapture,
          (s, snap) => { s.contextCapture = snap; },
          apiCall,
          (s) => s.api_call_id,
        );
        break;
      }
      case 'TextStreamed':
        if (hasVisibleText((event as { text?: string }).text)) {
          resolveLastPendingStep(steps, isThinking);
        }
        break;
      case 'ToolCalled': {
        const e = event as { name: string; args: unknown; description?: string };
        const naming = {
          description: e.description || describeEngineTool(e.name, e.args),
          outcome: callOutcome(exchange, seq),
          ...(event._eventId ? { call_event_id: event._eventId } : {}),
        };
        if (!nameThinkingRow(steps, naming)) steps.push({ ...naming });
        break;
      }
      case 'ToolResult':
        resolveLastPendingStep(steps, answeredBy(event, isNotThinking), toolResultOutcome(event));
        break;
      case 'CodingAgentPromptSent':
        steps.push({ description: 'Thinking', outcome: 'pending' });
        break;
      case 'CodingAgentToolCalled': {
        const e = event as { name: string; args: unknown; description?: string };
        const parent = parentToolUseIdOf(event);
        const apiCall = apiCallIdOf(event);
        const naming = {
          description: e.description || describeCCTool(e.name, e.args),
          tool_use_id: toolUseIdOf(event),
          ...(parent ? { parent_tool_use_id: parent } : {}),
          ...(apiCall ? { api_call_id: apiCall } : {}),
          outcome: callOutcome(exchange, seq),
        };
        if (parent || !nameThinkingRow(steps, naming)) steps.push({ ...naming });
        terminal = null;
        break;
      }
      case 'CodingAgentToolResult': {
        const id = toolUseIdOf(event);
        let resolved = false;
        if (id) {
          for (const step of steps) {
            if (step.tool_use_id === id) {
              settleMatchedCallStep(step);
              resolved = true;
              break;
            }
          }
        }
        if (!resolved) resolveLastPendingStep(steps, isNotThinking);
        break;
      }
      case 'ResponseGenerated': case 'CodingAgentIdled':
        terminal = 'clean';
        break;
      case 'ResponseCanceled': case 'ResponseAborted': case 'ResponseFailed':
        terminal = 'unclean';
        break;
      case 'CodingAgentThoughtStreamed': {
        const text = (event as { text?: string }).text ?? '';
        let target: Step | undefined;
        for (let i = steps.length - 1; i >= 0; i--) {
          if (steps[i].outcome === 'pending' && steps[i].description === 'Thinking') {
            target = steps[i];
            break;
          }
        }
        if (!target) {
          target = { description: 'Thinking', outcome: 'pending' };
          steps.push(target);
        }
        target.thinkingText = (target.thinkingText ?? '') + text;
        terminal = null;
        break;
      }
      case 'CodingAgentTextStreamed':
        if (isAgentReplyText(event) && hasVisibleText((event as { text?: string }).text)) {
          resolveLastPendingStep(steps, isThinking);
        }
        terminal = null;
        break;
    }
  }
  bindCapturesByApiCall(steps, callCaptures, s => s.api_call_id, (s, snap) => { s.contextCapture = snap; });
  if (terminal !== null || threadIdle) resolvePendingSteps(steps, pendingOutcomeFor(terminal));
  else if (exchange.continuationMoved) resolvePendingSteps(steps, 'success', isThinking);
  else if (needsLiveThinkingRow({
    exchange,
    isLast,
    hasCCContent: exchangeHasCCContent(exchange),
    anyLive: steps.some(isLiveStep),
  })) {
    steps.push({ description: 'Thinking', outcome: 'pending' });
  }
  return steps;
}

function lastStepIndex(events: ResponseEvent[], pred?: (s: StepLike) => boolean): number {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.type === 'step' && (!pred || pred(e))) return i;
  }
  return -1;
}

function lastPendingStepIndex(events: ResponseEvent[], pred?: (s: StepLike) => boolean): number {
  return lastStepIndex(events, s => s.outcome === 'pending' && (!pred || pred(s)));
}

function lastDeniedStepAwaitingResult(
  events: ResponseEvent[],
  answers: (s: StepLike) => boolean,
): Extract<ResponseEvent, { type: 'step' }> | null {
  const idx = lastStepIndex(events, s => s.outcome === 'denied' && answers(s));
  const step = idx >= 0 ? events[idx] : undefined;
  if (!step || step.type !== 'step' || step.result !== undefined) return null;
  return step;
}

function resolveLastPendingResponseStep(
  events: ResponseEvent[],
  pred?: (s: StepLike) => boolean,
  outcome: StepOutcome = 'success',
): Extract<ResponseEvent, { type: 'step' }> | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.type === 'step' && e.outcome === 'pending' && (!pred || pred(e))) {
      e.outcome = outcome;
      return e;
    }
  }
  return null;
}

function failureEchoPredicate(exchange: Exchange): (text: string | undefined) => boolean {
  const failure = exchangeError(exchange)?.message.trim();
  if (!failure) return () => false;
  return text => (text ?? '').trim() === failure;
}

export function agentMessageSender(origin: MessageOrigin | undefined): string | undefined {
  switch (origin?.kind) {
    case 'thread_link':
      return origin.title ? `"${origin.title}"` : 'another thread';
    case 'workspace':
      return `workspace "${origin.workspace}"`;
    default:
      return undefined;
  }
}

function handoffSession(
  continuity: Extract<SessionContinuity, { kind: 'handoff' }>,
): Extract<ResponseEvent, { type: 'handoff_session' }> {
  return {
    type: 'handoff_session',
    checkpoint: continuity.checkpoint,
    progress: continuity.progress,
    opening: continuity.opening,
    ...(continuity.crossing && { crossing: continuity.crossing }),
    message: continuity.message,
  };
}

export function drawnResponseEvents(exchange: Exchange, isLast: boolean, threadIdle: boolean, cacheCards: boolean): ResponseEvent[] {
  const events = exchangeResponseEvents(exchange, isLast, threadIdle);
  return cacheCards ? events : events.filter((e) => e.type !== 'cache_keep_alive');
}

export function exchangeResponseEvents(exchange: Exchange, isLast = true, threadIdle = false): ResponseEvent[] {
  const events: ResponseEvent[] = [];
  const hasCCContent = exchangeHasCCContent(exchange);
  const isFailureEcho = failureEchoPredicate(exchange);
  let terminal: TerminalKind = null;
  let emptyCompletion = false;
  let endedByQuietAnswer = false;
  let quietReply = exchangeQuietReply(exchange);
  let currentContext: ContextAssembledData | undefined;
  let legacyAcc: LegacyContextEvents = {};
  const callCaptures = new Map<string, ContextCapture>();
  const agentCaptures = new Map<string, ContextCapture>();
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

  for (const { seq, event } of exchange.steps) {
    const created = event.created;
    const pushedBefore = events.length;
    switch (event.type) {
      case 'MemoryRecalled':
      case 'MemorySearched': {
        const ms = event as { results?: number; queries?: string[]; memories?: RecalledMemory[] };
        const recall = { memories: ms.memories ?? [], queries: ms.queries ?? [] };
        pushStep({
          type: 'step',
          description: memoryRecalledLabel(ms.results ?? 0),
          outcome: 'success',
          recall: recall.memories.length || recall.queries.length ? recall : undefined,
          created,
        });
        break;
      }
      case 'WorkingUnderstandingWritten': {
        const wu = event as { document?: string };
        pushStep({
          type: 'step',
          description: 'Updated its working understanding',
          outcome: 'success',
          detail: wu.document || undefined,
          created,
        });
        break;
      }
      case 'CodingAgentCompacted':
        pushStep({ type: 'step', description: compactedLabel(event), outcome: 'success', created });
        break;
      case 'CodingAgentModelSubstituted':
        pushStep({ type: 'step', description: modelSubstitutedLabel(event), outcome: 'success', created });
        break;
      case 'ThoughtStreamed': {
        resolveLastPendingResponseStep(events, isThinking);
        const ctx = event as { context_tokens?: number; context_messages?: number; trimmed?: boolean };
        legacyAcc = { thinking: ctx };
        pushStep({
          type: 'step',
          description: 'Thinking',
          outcome: 'pending',
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
        const snap = capturedEventToData(
          event as Extract<ThreadEvent, { type: 'ContextCaptured' }>,
          event._eventId,
        );
        const apiCall = apiCallIdOf(event);
        if (apiCall) callCaptures.set(apiCall, snap);
        const parent = parentToolUseIdOf(event);
        if (parent) agentCaptures.set(parent, snap);
        bindSnapshotToStep(
          snap,
          parent,
          events,
          (e) => e.type === 'step',
          (e) => e.type === 'step' && e.description === 'Thinking',
          (e) => (e.type === 'step' ? e.parent_tool_use_id : undefined),
          (e) => (e.type === 'step' ? e.contextCapture : undefined),
          (e, snap) => { if (e.type === 'step') e.contextCapture = snap; },
          apiCall,
          (e) => (e.type === 'step' ? e.api_call_id : undefined),
        );
        break;
      }
      case 'ToolCalled': {
        const e = event as { name: string; args?: unknown; description?: string; args_stripped?: boolean };
        const naming = {
          description: e.description || describeEngineTool(e.name, e.args),
          tool_name: e.name,
          full: fullCommandForEngineTool(e.name, e.args),
          outcome: callOutcome(exchange, seq),
          created,
          seq,
          ...(event._eventId ? { call_event_id: event._eventId } : {}),
          ...(e.args_stripped ? { args_stripped: true, tool_channel: 'chat' as const } : {}),
        };
        if (!nameThinkingStep(events, naming)) pushStep({ type: 'step', ...naming });
        break;
      }
      case 'ToolResult': {
        const toolResult = event as { name?: string; result?: string; images?: string[]; result_stripped?: boolean };
        const answers = answeredBy(event, toolResult.name === AWAIT_EVENT_TOOL ? isAwaitEventStep : isNotThinking);
        const resolved = resolveLastPendingResponseStep(events, answers, toolResultOutcome(event))
          ?? lastDeniedStepAwaitingResult(events, answers);
        if (resolved) {
          if (toolResult.result !== undefined) resolved.result = toolResult.result;
          if (event._eventId) resolved.result_event_id = event._eventId;
          if (toolResult.result_stripped) resolved.result_stripped = true;
        }
        if (toolResult.images?.length) {
          const prompt = resolved?.tool_name === 'generate_image' ? resolved.full : undefined;
          for (const b64 of toolResult.images) {
            events.push({ type: 'image', base64: b64, mime_type: 'image/jpeg', ...(prompt ? { prompt } : {}) });
          }
        }
        break;
      }
      case 'TextStreamed': {
        const text = (event as { text: string }).text;
        if (hasVisibleText(text)) resolveLastPendingResponseStep(events, isThinking);
        if (!isFailureEcho(text)) events.push({ type: 'text', md: text });
        break;
      }
      case 'SessionStarted':
        if (hasCCContent) events.push({ type: 'section_break', channel: 'claude_code' });
        if (event.continuity?.kind === 'fresh') events.push({ type: 'fresh_session', reason: event.continuity.reason });
        if (event.continuity?.kind === 'handoff') events.push(handoffSession(event.continuity));
        break;
      case 'CodingAgentPromptSent':
        pushStep({ type: 'step', description: 'Thinking', outcome: 'pending', created });
        break;
      case 'CodingAgentToolCalled': {
        const e = event as { name: string; args?: unknown; description?: string; args_stripped?: boolean };
        const parent = parentToolUseIdOf(event);
        const apiCall = apiCallIdOf(event);
        const naming = {
          description: e.description || describeCCTool(e.name, e.args),
          tool_name: e.name,
          tool_use_id: toolUseIdOf(event),
          ...(parent ? { parent_tool_use_id: parent } : {}),
          ...(apiCall ? { api_call_id: apiCall } : {}),
          full: fullCommandForCCTool(e.name, e.args),
          outcome: callOutcome(exchange, seq),
          created,
          seq,
          ...(event._eventId ? { call_event_id: event._eventId } : {}),
          ...(e.args_stripped ? { args_stripped: true, tool_channel: 'coding_agent' as const } : {}),
        };
        if (parent || !nameThinkingStep(events, naming)) pushStep({ type: 'step', ...naming });
        terminal = null;
        break;
      }
      case 'CodingAgentToolResult': {
        const id = toolUseIdOf(event);
        const cc = event as { result?: string; result_stripped?: boolean };
        const land = (step: Extract<ResponseEvent, { type: 'step' }>) => {
          if (cc.result !== undefined) step.result = cc.result;
          if (event._eventId) step.result_event_id = event._eventId;
          if (cc.result_stripped) step.result_stripped = true;
        };
        let resolved = false;
        if (id) {
          for (const e of events) {
            if (e.type === 'step' && e.tool_use_id === id) {
              settleMatchedCallStep(e);
              land(e);
              resolved = true;
              break;
            }
          }
        }
        if (!resolved) {
          const fallback = resolveLastPendingResponseStep(events, isNotThinking);
          if (fallback) land(fallback);
        }
        break;
      }
      case 'CodingAgentThoughtStreamed': {
        const text = (event as { text?: string }).text ?? '';
        let target: Extract<ResponseEvent, { type: 'step' }> | undefined;
        for (let i = events.length - 1; i >= 0; i--) {
          const e = events[i];
          if (e.type === 'step' && e.outcome === 'pending' && e.description === 'Thinking') {
            target = e;
            break;
          }
        }
        if (!target) {
          target = { type: 'step', description: 'Thinking', outcome: 'pending', created };
          pushStep(target);
        }
        target.thinkingText = (target.thinkingText ?? '') + text;
        terminal = null;
        break;
      }
      case 'CodingAgentTextStreamed': {
        terminal = null;
        if (!isAgentReplyText(event)) break;
        const text = (event as { text: string }).text;
        if (hasVisibleText(text)) resolveLastPendingResponseStep(events, isThinking);
        if (!isFailureEcho(text)) events.push({ type: 'text', md: text });
        break;
      }
      case 'CodingAgentUserMessageSent':
        break;
      case 'SpokenReplyGenerated': {
        const e = event as { text: string; interrupted?: boolean };
        if (!hasVisibleText(e.text)) break;
        const after = events[events.length - 1];
        events.push({
          type: 'spoken_reply',
          text: e.text,
          interrupted: e.interrupted === true,
          ...(after?.type === 'spoken_reply' ? { follows: true as const } : {}),
        });
        break;
      }
      case 'CommandCheckpointed': {
        const e = event as {
          checkpoint_id: string;
          command: string;
          summary: string;
          restores?: number;
          removes?: number;
        };
        events.push({
          type: 'checkpoint',
          checkpoint_id: e.checkpoint_id,
          command: e.command,
          summary: e.summary,
          reverted: false,
          restores: e.restores ?? 0,
          removes: e.removes ?? 0,
        });
        break;
      }
      case 'ChildReportFiled':
      case 'ChildThreadStopped':
      case 'ChildThreadDetached':
        events.push({ type: 'child_note', note: event });
        break;
      case 'ContinuationStarted':
        events.push({ type: 'restart', reason: event.reason ?? '', actor: event.actor });
        break;
      case 'WidgetShown': {
        const e = event as { app_id: string; params?: Record<string, unknown>; label?: string; _eventId?: string };
        events.push({ type: 'widget', app_id: e.app_id, params: e.params, label: e.label, event_id: e._eventId });
        break;
      }
      case 'MessageHeld': {
        if (event._eventId && exchange.deliveredHeldIds?.has(event._eventId)) break;
        events.push({
          type: 'held_message',
          held_id: event._eventId ?? '',
          text: event.text,
          sender: agentMessageSender(event.origin) ?? 'an agent',
          released: false,
        });
        break;
      }
      case 'HeldMessageReleased': {
        for (const row of events) {
          if (row.type === 'held_message' && row.held_id === event.held_message_id) {
            row.released = true;
          }
        }
        break;
      }
      case 'CodingAgentCommandAnswered':
        events.push({ type: 'coding_agent_command_answer', answer: event });
        break;
      case 'AgentSignInNeeded':
        events.push({ type: 'agent_sign_in_needed', notice: event });
        break;
      case 'TerminalCommandSuggested':
        events.push({ type: 'terminal_suggestion', suggestion: event });
        break;
      case 'AgentSignInRequested':
      case 'CredentialRequested':
      case 'PluginInstallRequested':
      case 'PluginUninstallRequested':
      case 'EmailConfirmRequested':
      case 'OAuthAuthorizationRequested':
        events.push({ type: 'form_request', request: event });
        break;
      case 'FormRequestResolved':
        for (const row of events) {
          if (row.type === 'form_request' && row.request.request_id === event.request_id) {
            row.resolution = event.outcome;
          }
        }
        break;
      case 'CacheKeepAliveStatusChanged': {
        const status = event.status;
        if (!status || status.state === 'not_kept') break;
        events.push({ type: 'cache_keep_alive', status });
        break;
      }
      case 'EventWaitStarted': {
        const row: ResponseEvent = {
          type: 'event_wait',
          wait_id: event.wait_id,
          subscriptions: event.on,
          reason: event.reason,
          created,
          expires_at: event.expires_at,
          background_timing: event.background_timing,
          state: 'waiting',
        };
        const parked = lastPendingStepIndex(events, isAwaitEventStep);
        if (parked >= 0) events[parked] = { ...row, seq: events[parked].seq };
        else events.push(row);
        terminal = null;
        break;
      }
      case 'EventWaitTimingUpdated': {
        for (const prior of events) {
          if (prior.type === 'event_wait' && prior.state === 'waiting' && prior.wait_id === event.wait_id) {
            prior.reason = event.reason;
            prior.expires_at = event.expires_at;
            prior.background_timing = event.background_timing;
            break;
          }
        }
        break;
      }
      case 'EventWaitDelivered':
      case 'EventWaitExpired': {
        const e = event as { wait_id: string };
        const state = event.type === 'EventWaitDelivered' ? 'matched' : 'timed_out';
        for (const prior of events) {
          if (prior.type === 'event_wait' && prior.wait_id === e.wait_id) {
            prior.state = state;
            if (state === 'matched') prior.matched_at = created;
            break;
          }
        }
        break;
      }
      case 'EventWaitCanceled': {
        if (isUserStoppedWait(event)) break;
        const e = event as {
          wait_id: string;
          on?: EventSubscription[];
          reason?: string;
          cause?: EventWaitCancelCause;
        };
        events.push({
          type: 'event_wait',
          wait_id: e.wait_id,
          subscriptions: e.on ?? [],
          reason: e.reason ?? '',
          created,
          expires_at: '',
          state: 'canceled',
          cause: e.cause,
        });
        break;
      }
      case 'CommandCheckpointReverted': {
        const id = (event as { checkpoint_id: string }).checkpoint_id;
        for (const prior of events) {
          if (prior.type === 'checkpoint' && prior.checkpoint_id === id) {
            prior.reverted = true;
            break;
          }
        }
        break;
      }
      case 'ResponseGenerated':
        terminal = 'clean';
        emptyCompletion = !(event as { text?: string }).text?.trim();
        if (endedByQuietAnswer && quietReply !== null) {
          events.push({ type: 'text', md: quietReply });
          quietReply = null;
        }
        break;
      case 'CodingAgentIdled':
        terminal = 'clean';
        break;
      case 'UserQuestionAnswered':
        endedByQuietAnswer = event.quiet === true;
        break;
      case 'ResponseCanceled': case 'ResponseAborted': case 'ResponseFailed':
        terminal = 'unclean';
        break;
      case 'SessionEnded':
        break;
    }
    for (let i = pushedBefore; i < events.length; i++) events[i].seq ??= seq;
  }
  bindCapturesByApiCall(
    events,
    callCaptures,
    e => (e.type === 'step' ? e.api_call_id : undefined),
    (e, snap) => { if (e.type === 'step') e.contextCapture = snap; },
  );
  for (const e of events) {
    const final = e.type === 'step' && e.tool_use_id ? agentCaptures.get(e.tool_use_id) : undefined;
    if (final && e.type === 'step') e.finalContextCapture = final;
  }
  const finalizeAll = terminal !== null || threadIdle;
  if (finalizeAll || exchange.continuationMoved) {
    const stepEvents = events.filter(e => e.type === 'step') as StepLike[];
    if (finalizeAll) resolvePendingSteps(stepEvents, pendingOutcomeFor(terminal));
    else resolvePendingSteps(stepEvents, 'success', isThinking);
    while (events.length > 1) {
      const last = events[events.length - 1];
      if (last.type === 'step' && isThinking(last)) {
        events.pop();
      } else {
        break;
      }
    }
    if (
      emptyCompletion
      && !endedByQuietAnswer
      && !events.some(isMeaningfulText)
      && !events.some(e => e.type === 'image')
    ) {
      events.push({ type: 'empty' });
    }
  } else if (needsLiveThinkingRow({
    exchange,
    isLast,
    hasCCContent,
    anyLive: lastStepIndex(events, isLiveStep) >= 0,
  })) {
    pushStep({ type: 'step', description: 'Thinking', outcome: 'pending' });
  }
  return mergeAdjacentTextEvents(nestSubAgentSteps(events));
}

function nestSubAgentSteps(events: ResponseEvent[]): ResponseEvent[] {
  const agents = new Map<string, Extract<ResponseEvent, { type: 'step' }>>();
  for (const e of events) {
    if (e.type === 'step' && e.tool_use_id) agents.set(e.tool_use_id, e);
  }
  return events.filter(e => {
    const agent = e.type === 'step' && e.parent_tool_use_id ? agents.get(e.parent_tool_use_id) : undefined;
    if (!agent || agent === e || e.type !== 'step') return true;
    (agent.children ??= []).push(e);
    return false;
  });
}

export function hasRenderableResponseContent(events: ResponseEvent[]): boolean {
  return events.some(e => e.type !== 'text' || isMeaningfulText(e));
}

function hasSpokenRow(events: ResponseEvent[]): boolean {
  return events.some(e => e.type === 'spoken_reply');
}

export function dividerBodyIsSuppressed(exchange: Exchange, events: ResponseEvent[]): boolean {
  return questionDividerResolution(exchange) !== null && !hasSpokenRow(events);
}

export function stepStatus(outcome: StepOutcome): { label: string; className: StepOutcome } {
  switch (outcome) {
    case 'pending': return { label: 'In progress', className: 'pending' };
    case 'success': return { label: 'Completed', className: 'success' };
    case 'error': return { label: 'Failed', className: 'error' };
    case 'unfinished': return { label: 'Did not finish', className: 'unfinished' };
    case 'blocked': return { label: 'Needs approval', className: 'blocked' };
    case 'denied': return { label: 'Denied', className: 'denied' };
  }
}

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

export function questionDividerResolution(
  exchange: Exchange,
): 'canceled' | 'superseded' | null {
  if (exchange.userEvent.type !== 'UserQuestionAsked') return null;
  for (const { event } of exchange.steps) {
    if (event.type !== 'UserQuestionAnswered') continue;
    if (event.answer.kind === 'Canceled') return 'canceled';
    if (event.answer.kind === 'Superseded') return 'superseded';
  }
  return null;
}

export function abortTookEngineDown(ev: ThreadEvent): boolean {
  return ev.type === 'ResponseAborted' && isEngineDownAbort(ev.cause);
}

function isCallOnly(exchange: Exchange): boolean {
  if (!isCallBoundary(exchange.userEvent)) return false;
  return exchange.steps.every(s => VOICE_ONLY_STEP_TYPES.has(s.event.type));
}

export function isSpeechOnlyTurn(exchange: Exchange): boolean {
  if (exchange.tookTheTurn) return false;
  if (isLiveCallRow(exchange.userEvent)) return true;
  return isCallOnly(exchange);
}

export function turnBodyFolded(folded: ReadonlySet<string>, threadId: string, exchange: Exchange): boolean {
  return folded.has(`${threadId}:${exchange.userSeq}`) && !isSpeechOnlyTurn(exchange);
}

function callAnswered(exchange: Exchange): boolean {
  if (exchange.tookTheTurn) return false;
  if (exchange.userEvent.type === 'MessageReceived') return false;
  if (exchange.userEvent.type === 'SpokenReplyGenerated') return true;
  return exchange.steps.some(s => s.event.type === 'SpokenReplyGenerated');
}

function callHasEnded(exchange: Exchange): boolean {
  for (let i = exchange.steps.length - 1; i >= 0; i--) {
    const type = exchange.steps[i].event.type;
    if (type === 'VoiceSessionEnded') return true;
    if (type === 'VoiceSessionStarted') return false;
  }
  return false;
}

export function abortPromisesAutoResume(ev: ThreadEvent): boolean {
  return ev.type === 'ResponseAborted' && isSwitchTeardownAbort(ev.actor, ev.cause);
}

export function restartPauseFoldsInto(pause: Exchange, next: Exchange | undefined): boolean {
  if (next?.userEvent.type !== 'ContinuationStarted') return false;
  if (!abortPromisesAutoResume(pause.userEvent)) return false;
  return !exchangeResponseText(pause)
    && !hasRenderableResponseContent(exchangeResponseEvents(pause, false, true));
}

export function continuableAbortIndex(exchanges: Exchange[]): number | null {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const ev = exchanges[i].userEvent;
    if (ev.type === 'ContinuationStarted') return null;
    if (ev.type === 'ResponseAborted') {
      if (ev.cause === 'stale_settle') return null;
      if (abortPromisesAutoResume(ev)) return null;
      if (abortBoundaryResolved(exchanges[i])) continue;
      return i;
    }
  }
  return null;
}

function abortBoundaryResolved(exchange: Exchange): boolean {
  return exchange.steps.some(({ event }) =>
    TERMINAL_EVENT_TYPES.has(event.type) && !isRecoveryInterruptMarker(event));
}

function isRecoveryInterruptMarker(event: ThreadEvent): boolean {
  return event.type === 'CodingAgentIdled'
    && event.reason === IDLE_ENGINE_RESTART_INTERRUPT_REASON;
}


export function resumeEngineNote(exchange: Exchange): { text: string; toolCount: number } | null {
  for (const { event } of exchange.steps) {
    if (event.type === 'PromptInjected' && (event as { mode?: ActorMode }).mode === 'engine') {
      const text = (event as { text: string }).text || '';
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


export type ReadMarker = 'sent' | 'read';

export function readMarkers(exchanges: Exchange[], threadIsCC: boolean): Map<number, ReadMarker> {
  return threadIsCC ? codingAgentReadMarkers(exchanges) : chatReadMarkers(exchanges);
}

function chatReadMarkers(exchanges: Exchange[]): Map<number, ReadMarker> {
  const markers = new Map<number, ReadMarker>();
  exchanges.forEach((exchange, i) => {
    const { userEvent } = exchange;
    if (userEvent.type !== 'MessageReceived' || !userEvent._eventId) return;
    if (isLiveUtteranceRow(userEvent) || (userEvent as { voice_session_id?: string }).voice_session_id) return;
    markers.set(i, isUningestedMessage(exchange) ? 'sent' : 'read');
  });
  return markers;
}

function codingAgentReadMarkers(exchanges: Exchange[]): Map<number, ReadMarker> {
  const markers = new Map<number, ReadMarker>();
  const isMessage = ({ userEvent }: Exchange) =>
    userEvent.type === 'MessageReceived' && !!userEvent._eventId;
  const first = exchanges.findIndex(ex => ex.inputRead && isMessage(ex));
  if (first === -1) return markers;
  for (let i = first; i < exchanges.length; i++) {
    if (!isMessage(exchanges[i])) continue;
    markers.set(i, exchanges[i].inputRead ? 'read' : 'sent');
  }
  return markers;
}


declare const abortEvidenceBrand: unique symbol;

export type AbortEvidence = {
  readonly event: StoredEvent;
  readonly [abortEvidenceBrand]: true;
};

export function abortEvidenceOf(event: StoredEvent): AbortEvidence | null {
  const records = event.type === 'ResponseAborted'
    || (event.type === 'SessionEnded' && !NORMAL_SESSION_END_REASONS.has(event.reason ?? 'completed'));
  return records ? { event } as AbortEvidence : null;
}

export type ExchangeVerdict =
  | { readonly status: Exclude<ExchangeStatus, 'aborted'> }
  | { readonly status: 'aborted'; readonly evidence: AbortEvidence };

function verdict(status: Exclude<ExchangeStatus, 'aborted'>): ExchangeVerdict {
  return { status };
}

function aborted(evidence: AbortEvidence): ExchangeVerdict {
  return { status: 'aborted', evidence };
}

export function exchangeStatus(exchange: Exchange, streamingBuffer: string, isLast: boolean, hasPriorActive?: boolean, threadIsCC?: boolean, threadIdle = false, threadAwaitingAnswer = false, behindOpenQuestion = false): ExchangeStatus {
  return exchangeVerdict(exchange, streamingBuffer, isLast, hasPriorActive, threadIsCC, threadIdle, threadAwaitingAnswer, behindOpenQuestion).status;
}

export function exchangeVerdict(exchange: Exchange, streamingBuffer: string, isLast: boolean, hasPriorActive?: boolean, threadIsCC?: boolean, threadIdle = false, threadAwaitingAnswer = false, behindOpenQuestion = false): ExchangeVerdict {
  if (isLiveUtteranceRow(exchange.userEvent)) {
    return verdict(isSettledLiveUtterance(exchange.userEvent) ? 'done' : 'pending');
  }
  if (isLiveReplyRow(exchange.userEvent)) return verdict('done');
  let isComplete = false;
  let isCanceled = false;
  let abortedBy: AbortEvidence | null = null;
  let isFailed = false;
  let isCC = false;
  let isCCWaiting = false;
  let isSessionEndedNormally = false;
  let shutdownBy: AbortEvidence | null = null;
  let abnormalEndBy: AbortEvidence | null = null;
  let isWaitingForAnswer = false;
  let isParkedOnEventWait = false;
  let wasCompleted = false;
  let completedBeforeAbort = false;
  let restartOwed = false;
  let workStepCount = 0;
  let firstWorkStep: StoredEvent | undefined;

  const supersededAborts = supersededAbortIndices(exchange.steps);

  if (opensAwaitingAnswer(exchange)) isWaitingForAnswer = true;
  const card = exchange.userEvent;
  const engineCardId = card.type === 'UserQuestionAsked' && wakesNoAgent(card) ? card.tool_use_id : null;

  for (let i = 0; i < exchange.steps.length; i++) {
    const event = exchange.steps[i].event;
    if (isChildNote(event)) continue;
    workStepCount++;
    firstWorkStep ??= event;
    switch (event.type) {
      case 'ResponseGenerated': isComplete = true; wasCompleted = true; break;
      case 'ResponseCanceled':
        if (isRedirectCancel(event.cause)) {
          isComplete = true;
          if (event.cause === 'effort_restart') restartOwed = true;
          break;
        }
        isCanceled = true; isComplete = true; break;
      case 'ContinuationStarted':
        restartOwed = false; isComplete = false; isCCWaiting = false; wasCompleted = false;
        break;
      case 'ResponseAborted':
        if (supersededAborts.has(i)) break;
        if (wasCompleted) completedBeforeAbort = true;
        abortedBy = abortEvidenceOf(event); isComplete = true; break;
      case 'ResponseFailed': isFailed = true; isComplete = true; break;
      case 'SessionStarted':
        isCC = true; abnormalEndBy = null; isSessionEndedNormally = false; shutdownBy = null;
        break;
      case 'SessionEnded': {
        const reason = event.reason ?? 'completed';
        if (reason === 'shutdown') {
          if (wasCompleted) completedBeforeAbort = true;
          shutdownBy = abortEvidenceOf(event);
        }
        if (!NORMAL_SESSION_END_REASONS.has(reason)) {
          abnormalEndBy = abortEvidenceOf(event);
        } else if (reason !== 'stale_resume') {
          isSessionEndedNormally = true;
        }
        break;
      }
      case 'CodingAgentIdled': isCCWaiting = true; wasCompleted = true; break;
      case 'CodingAgentUserMessageSent':
        isCCWaiting = false; isComplete = false; wasCompleted = false; break;
      case 'CodingAgentToolCalled':
      case 'CodingAgentTextStreamed':
      case 'CodingAgentPromptSent':
        isCCWaiting = false; isComplete = false; isWaitingForAnswer = false;
        isParkedOnEventWait = false;
        break;
      case 'CodingAgentToolResult': isParkedOnEventWait = false; break;
      case 'UserQuestionAsked':
      case 'CodingAgentPermissionRequest':
      case 'CommandPermissionRequested':
      case 'McpPermissionRequested':
        isWaitingForAnswer = true; break;
      case 'UserQuestionAnswered':
        isWaitingForAnswer = false;
        if (event.tool_use_id === engineCardId) isComplete = true;
        break;
      case 'CodingAgentPermissionResolved':
      case 'CommandPermissionResolved':
      case 'McpPermissionResolved':
        isWaitingForAnswer = false; break;
      case 'EventWaitStarted': isParkedOnEventWait = true; break;
      case 'EventWaitDelivered':
      case 'EventWaitExpired':
        isParkedOnEventWait = false; break;
    }
  }

  if (threadIsCC) isCC = true;

  const hasSteps = workStepCount > 0;
  const onlyStep = workStepCount === 1 ? firstWorkStep : undefined;
  const injectionRequestId = (onlyStep as { request_event_id?: string } | undefined)?.request_event_id;
  const anchorId = exchange.userEvent._eventId;
  const announcesThisExchange = !!injectionRequestId && !!anchorId && injectionRequestId === anchorId;
  const isAbsorbedInjectionPlaceholder = onlyStep?.type === 'PromptInjected'
    && !!onlyStep.injected_message_id
    && !announcesThisExchange;

  const boundaryAbort = abortTookEngineDown(exchange.userEvent) ? abortEvidenceOf(exchange.userEvent) : null;
  const isEngineDownBoundary = boundaryAbort !== null;

  const isStale =
    (threadIdle || isEngineDownBoundary) && !threadAwaitingAnswer
    && isLast && !isComplete && hasSteps
    && !isAbsorbedInjectionPlaceholder;

  if (isFailed) return verdict('error');
  if ((abortedBy || shutdownBy) && completedBeforeAbort) return verdict('done');
  if (abortedBy) return aborted(abortedBy);
  if (shutdownBy) return aborted(shutdownBy);
  if (isCanceled) return verdict('canceled');
  if (abnormalEndBy && !isComplete && !isCCWaiting) return aborted(abnormalEndBy);
  if (isTurnlessBoundary(exchange.userEvent)) return verdict('done');
  if ((threadIdle || exchangeHoldsNoTurn(exchange)) && isCallOnly(exchange)) {
    if (!isLast) return verdict('done');
    if (callAnswered(exchange)) return verdict('done');
    if (!callHasEnded(exchange)) return verdict('streaming');
    if (!exchange.tookTheTurn && exchange.userEvent.type !== 'MessageReceived') return verdict('done');
  }
  if (hasPriorActive && !hasSteps && !isCC && isLast) return verdict('queued');
  if (restartOwed && isLast) return verdict('coding-agent-working');
  if (isCCWaiting) return verdict('done');
  if (isParkedOnEventWait) return verdict('done');
  if (isCC && isSessionEndedNormally) return verdict('done');
  if (isWaitingForAnswer && !exchange.questionOvertaken) return verdict('awaiting-answer');
  if (threadAwaitingAnswer && threadIdle && (isLast || behindOpenQuestion) && !hasSteps
    && !isChangeLifecycleEvent(exchange.userEvent)) return verdict('held');
  if (!isLast && !isComplete && hasSteps && !isAbsorbedInjectionPlaceholder) return verdict('interrupted');
  if (isComplete) return verdict('done');
  if (!isLast && (isCC || threadIdle)) return verdict('done');
  if (!hasSteps && !isCC && (!isLast || threadIdle)) return verdict('done');
  if (exchange.userEvent.type === 'ChildThreadCompleted' && !hasSteps && (threadIdle || !isLast)) return verdict('done');

  if (isCC && !isStale) return verdict(hasSteps ? 'coding-agent-working' : 'pending');
  if (streamingBuffer) return verdict('streaming');

  if (isAbsorbedInjectionPlaceholder) return verdict('done');

  if (isStale) return boundaryAbort ? aborted(boundaryAbort) : verdict('done');

  const responseText = exchangeResponseText(exchange);
  if (responseText) return verdict('streaming');

  const steps = exchangeSteps(exchange, isLast, threadIdle);
  const events = exchangeResponseEvents(exchange, isLast, threadIdle);
  if (steps.length > 0 || events.some(event => event.type !== 'child_note')) return verdict('streaming');

  return verdict('pending');
}
