import type { CacheKeepAliveStatus } from '../../generated/thread-event-wire';
import { EVENT_CLASSIFICATION } from '../../generated/thread-lifecycle';
import { instantMicros } from '../../utils/isoInstant';
import { eventWaitProjection } from './event-waits';
import { changePanelHasContinuation, findQuestionAnswer, isAgentReplyText, isChangeLifecycleEvent, modeToInitiator, parentToolUseIdOf } from './exchange';
import { isOneUtterance, joinSpoken } from './spokenMerge';
import { CONTINUATION_EFFORT_RESTART_REASON, SIDE_QUESTION_TYPES, LEGACY_WIDGET_PIN_TYPES, isChildNote, isEngineRaisedCard, isFormRequest, isRedirectCancel, isSideQuestionEvent, isTurnlessBoundary, isUserStoppedWait, wakesNoAgent } from './thread-event-types';
import { applyAggregateToThread, terminalProjection, updatesLastActivity } from './thread-meta';
import type { Exchange, TypedAnswer } from './exchange';
import type { MessageOrigin, SequencedEvent, StoredEvent, ThreadEvent, TransientEvent } from './thread-event-types';
import { KEEP_ALIVE_ENDING_EVENT_TYPES, KEEP_ALIVE_IDLE_REASON, QUESTION_OVERTAKEN_EVENT_TYPES } from '@lucidos/engine-constants';
import type { PendingUserMessage, ThreadAggregate, ThreadState } from './thread-meta';


export function computeExchanges(thread: ThreadState): Exchange[] {
  return withLiveCallRows(thread, foldedExchanges(thread));
}

function withLiveCallRows(thread: ThreadState, exchanges: Exchange[]): Exchange[] {
  const live = liveCallEvents(thread);
  if (live.length === 0) return exchanges;
  const out = [...exchanges];
  let seq = Number.MAX_SAFE_INTEGER - (live.length - 1);
  for (const userEvent of live) {
    const at = isLiveReplyRow(userEvent) ? liveReplyTargetIndex(out) : -1;
    if (at === -1) {
      out.push({ userEvent, userSeq: seq, steps: [] });
    } else {
      const steps = [...out[at].steps];
      steps.splice(callRowIndex(steps, happenedAt(userEvent)), 0, {
        seq,
        event: userEvent,
      });
      out[at] = {
        ...out[at],
        steps,
        liveReplyText: userEvent.type === 'SpokenReplyGenerated' ? userEvent.text : undefined,
      };
    }
    seq += 1;
  }
  return out;
}

export function happenedAt(event: StoredEvent): number | null {
  const landed = instantMicros(event.created ?? event._displayCreated);
  const secs = event.type === 'SpokenReplyGenerated' ? event.spoken_secs_before : undefined;
  if (landed === null || secs === undefined) return landed;
  return landed - secs * 1_000_000;
}

function callRowIndex(steps: SequencedEvent[], saidAt: number | null): number {
  if (saidAt === null) return steps.length;
  let at = steps.length;
  for (let i = 0; i < steps.length; i++) {
    const landed = happenedAt(steps[i].event);
    if (landed !== null && landed > saidAt) {
      at = i;
      break;
    }
  }
  return runStartBefore(steps, at, steps[at]?.event);
}

function spokenRowToGrow(
  steps: SequencedEvent[],
  at: number,
): SequencedEvent | undefined {
  const between: SequencedEvent[] = [];
  for (let i = at - 1; i >= 0; i--) {
    const step = steps[i];
    if (step.event.type !== 'SpokenReplyGenerated') {
      between.push(step);
      continue;
    }
    const stopped = instantMicros(step.event.created);
    return between.every(other => {
      const landed = happenedAt(other.event);
      return stopped !== null && landed !== null && landed <= stopped;
    })
      ? step
      : undefined;
  }
  return undefined;
}

function runStartBefore(
  steps: SequencedEvent[],
  at: number,
  rightOf: { type: string } | undefined,
): number {
  if (!rightOf || !isStreamedText(rightOf)) return at;
  let i = at;
  while (i > 0 && isStreamedText(steps[i - 1].event)) i -= 1;
  return i;
}

function isStreamedText(event: { type: string }): boolean {
  return isAgentReplyText(event);
}

function appendStep(exchange: Exchange, seq: number, event: StoredEvent): void {
  const steps = exchange.steps;
  const last = steps.length - 1;
  if (
    isStreamedText(event)
    && last >= 1
    && CALL_ROW_TYPES.has(steps[last].event.type)
    && isStreamedText(steps[last - 1].event)
  ) {
    const [lifted] = steps.splice(last, 1);
    steps.splice(runStartBefore(steps, steps.length, event), 0, lifted);
  }
  steps.push({ seq, event });
}

function liveCallEvents(thread: ThreadState): StoredEvent[] {
  const rows: { created: string; event: StoredEvent }[] = [];
  for (const row of thread.liveUtterances ?? []) {
    const provisional = row.text === undefined && row.partial !== undefined;
    rows.push({
      created: row.created,
      event: {
        type: 'MessageReceived' as const,
        text: row.text ?? row.partial ?? '',
        _eventId: row.eventId,
        _displayCreated: row.created,
        _liveUtterance: true as const,
        ...(provisional ? { _livePartial: true as const } : {}),
        channel: thread.meta.channel,
      } as StoredEvent,
    });
  }
  const reply = thread.liveReply;
  if (reply) {
    rows.push({
      created: reply.created,
      event: {
        type: 'SpokenReplyGenerated' as const,
        text: reply.text,
        interrupted: false,
        session_id: '',
        _eventId: reply.eventId,
        _displayCreated: reply.created,
        _liveReply: true as const,
      } as StoredEvent,
    });
  }
  return rows
    .map((row, i) => ({ ...row, i }))
    .sort((a, b) => (a.created === b.created ? a.i - b.i : a.created < b.created ? -1 : 1))
    .map(row => row.event);
}

function liveCallRowCount(thread: ThreadState): number {
  return (thread.liveUtterances?.length ?? 0) + (thread.liveReply ? 1 : 0);
}

type LiveRowMarks = { _liveUtterance?: true; _liveReply?: true; _livePartial?: true };

export function isLiveUtteranceRow(event: LiveRowMarks): boolean {
  return event._liveUtterance === true;
}

export function isLiveReplyRow(event: LiveRowMarks): boolean {
  return event._liveReply === true;
}

export function isLivePartialRow(event: LiveRowMarks): boolean {
  return event._livePartial === true;
}

export function isSettledLiveUtterance(event: LiveRowMarks & { text?: string }): boolean {
  return isLiveUtteranceRow(event) && !isLivePartialRow(event) && !!event.text;
}

export function isLiveCallRow(event: LiveRowMarks): boolean {
  return isLiveUtteranceRow(event) || isLiveReplyRow(event);
}

function syntheticSeqBase(thread: ThreadState): number {
  return Number.MAX_SAFE_INTEGER - liveCallRowCount(thread);
}

function foldedExchanges(thread: ThreadState): Exchange[] {
  const paged = thread.hasOlderEvents === true;
  const base = groupIntoExchangesCached(thread.events, paged);
  const { answers, rows } = splitTypedAnswers(base, thread.pendingUserMessages);
  const folded = foldPendingRows(thread, base, rows, paged);
  const waitingOn = (thread.unsentMessageSeqs?.size ?? 0) > 0 && thread.meta.status === 'waiting_for_user_answer'
    ? awaitedQuestionCard(folded)
    : undefined;
  return markWaitingCards(placeTypedAnswers(folded, answers, waitingOn));
}

export function isOpenQuestionCard(exchange: Exchange): boolean {
  const ue = exchange.userEvent;
  return ue.type === 'UserQuestionAsked'
    && !findQuestionAnswer(exchange, ue.tool_use_id)
    && exchange.questionOvertaken !== true;
}

export function markWaitingCards(exchanges: Exchange[]): Exchange[] {
  const open = exchanges.flatMap((ex, i) => (isOpenQuestionCard(ex) ? [i] : []));
  if (open.length < 2) return exchanges;
  const marked = [...exchanges];
  for (const i of open.slice(1)) marked[i] = { ...exchanges[i], cardWaiting: true };
  return marked;
}

export function opensAwaitingAnswer(exchange: Exchange): boolean {
  switch (exchange.userEvent.type) {
    case 'UserQuestionAsked':
    case 'CodingAgentPermissionRequest':
    case 'CommandPermissionRequested':
    case 'McpPermissionRequested':
      return true;
    default:
      return false;
  }
}

export function awaitedQuestionCard(exchanges: Exchange[]): string | undefined {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const ex = exchanges[i];
    if (!opensAwaitingAnswer(ex)) continue;
    const ue = ex.userEvent;
    return ue.type === 'UserQuestionAsked' && !findQuestionAnswer(ex, ue.tool_use_id) ? ue.tool_use_id : undefined;
  }
  return undefined;
}

function splitTypedAnswers(
  base: Exchange[],
  pending: PendingUserMessage[],
): { answers: Map<string, TypedAnswer>; rows: PendingUserMessage[] } {
  const answers = new Map<string, TypedAnswer>();
  if (!pending.some(p => p.answersQuestion)) return { answers, rows: pending };
  const cards = new Set<string>();
  for (const ex of base) {
    const ue = ex.userEvent;
    if (ue.type === 'UserQuestionAsked' && !findQuestionAnswer(ex, ue.tool_use_id)) cards.add(ue.tool_use_id);
  }
  const rows: PendingUserMessage[] = [];
  for (const p of pending) {
    const card = p.answersQuestion;
    if (card && !p.unconfirmed && cards.has(card) && !answers.has(card)) {
      answers.set(card, { state: 'sending', text: p.text, image_hashes: p.image_hashes ?? [] });
    } else {
      rows.push(p);
    }
  }
  return { answers, rows };
}

function placeTypedAnswers(exchanges: Exchange[], sending: Map<string, TypedAnswer>, waitingOn: string | undefined): Exchange[] {
  if (sending.size === 0 && !waitingOn) return exchanges;
  const typed = new Map(sending);
  let moved: Exchange | undefined;
  if (waitingOn && !typed.has(waitingOn)) {
    moved = exchanges.find((ex) => ex.userEvent.type === 'MessageReceived' && ex.userEvent._answersQuestion === waitingOn && !!ex.userEvent._eventId);
    const ue = moved?.userEvent;
    if (ue?.type === 'MessageReceived' && ue._eventId) {
      typed.set(waitingOn, { state: 'unsent', text: ue.text ?? '', image_hashes: ue.user_image_hashes ?? [], unsentEventId: ue._eventId });
    }
  }
  if (typed.size === 0) return exchanges;
  return exchanges.filter((ex) => ex !== moved).map((ex) => {
    const answer = ex.userEvent.type === 'UserQuestionAsked' ? typed.get(ex.userEvent.tool_use_id) : undefined;
    return answer ? { ...ex, typedAnswer: answer } : ex;
  });
}

function foldPendingRows(thread: ThreadState, base: Exchange[], rows: PendingUserMessage[], paged: boolean): Exchange[] {
  if (rows.length === 0) return filterRemovedQueuedExchanges(base, thread.events);
  const isCC = thread.meta.channel === 'claude_code';
  const pendingCount = rows.length;
  const synthetic: SequencedEvent[] = [];
  for (let i = 0; i < pendingCount; i++) {
    const pending = rows[i];
    const seq = syntheticSeqBase(thread) - pendingCount + i;
    synthetic.push({
      seq,
      event: {
        type: 'MessageReceived' as const,
        text: pending.text,
        _eventId: pending.eventId,
        channel: thread.meta.channel,
        ...(isCC ? { created: pending.created } : { _displayCreated: pending.created }),
        ...(pending.image_hashes?.length ? { user_image_hashes: pending.image_hashes } : {}),
      } as StoredEvent,
    });
  }

  const cache = incrementalCache.get(thread.events);
  const first = synthetic[0];
  const canAppendTrailing =
    !!cache &&
    cache.cacheable &&
    compareSortKeys(instantMicros(first.event.created), first.seq, cache.lastCreatedMicros, cache.lastSeq) >= 0;

  if (canAppendTrailing) {
    const exchanges = [...base];
    let turn = cache.fold.current;
    let othersWait = cache.fold.unreadMessages.length > 0;
    for (const { seq, event } of synthetic) {
      const pending: Exchange = { userEvent: event, userSeq: seq, steps: [] };
      if (turn && waitsForTheAgent(cache.fold, event, turn, othersWait)) {
        pending.awaitingRead = true;
        othersWait = true;
      } else {
        turn = pending;
      }
      exchanges.push(pending);
    }
    return filterRemovedQueuedExchanges(exchanges, thread.events);
  }

  const augmented = new Map(thread.events);
  for (const { seq, event } of synthetic) augmented.set(seq, event);
  return filterRemovedQueuedExchanges(groupIntoExchanges(augmented, paged), augmented);
}

function removedQueuedMessageIds(events: Map<number, StoredEvent>): Set<string> {
  const removed = new Set<string>();
  for (const event of events.values()) {
    if (event.type === 'QueuedMessageRemoved') removed.add(event.removed_message_id);
  }
  return removed;
}

function filterRemovedQueuedExchanges(
  exchanges: Exchange[],
  events: Map<number, StoredEvent>,
): Exchange[] {
  const removed = removedQueuedMessageIds(events);
  if (removed.size === 0) return exchanges;
  return exchanges.filter(ex => {
    const id = ex.userEvent._eventId;
    return !(isWaitingTypedMessage(ex) && id && removed.has(id));
  });
}

export const EXCHANGE_START_TYPES: ReadonlySet<string> = new Set([
  'MessageReceived',
  'TriggerStarted',
  'ResponseAborted',
  'ResponseCanceled',
  'ContinuationStarted',
  'PromptInjected',
  'MissingHardeningDetected',
  'MergeConflictDetected',
  'ChangeApplied',
  'ChangeDiscarded',
  'ChangeReverted',
  'ChangeApplyFailed',
  'UserQuestionAsked',
  'CodingAgentPermissionRequest',
  'CommandPermissionRequested',
  'McpPermissionRequested',
  'McpConsentRequested',
  'ChildThreadCompleted',
  'ChildThreadStopped',
  'ChildThreadDetached',
  'ChildReportFiled',
  'SpokenMessageReceived',
  'UserOnlyMessagePosted',
  'CodingAgentCommandReceived',
]);

export function isExchangeStartEvent(event: { type: string; cause?: string }): boolean {
  if (event.type === 'EventWaitCanceled') return isUserStoppedWait(event);
  return EXCHANGE_START_TYPES.has(event.type);
}

export type ContinuationHandoff = 'takes' | 'takes-unless-parked' | 'leaves' | 'ends' | 'own-arm';

export const BOUNDARY_CONTINUATION_HANDOFF: ReadonlyMap<string, ContinuationHandoff> = new Map<string, ContinuationHandoff>([
  ['MessageReceived', 'leaves'],
  ['TriggerStarted', 'leaves'],
  ['ResponseAborted', 'ends'],
  ['ResponseCanceled', 'ends'],
  ['ContinuationStarted', 'takes-unless-parked'],
  ['PromptInjected', 'takes-unless-parked'],
  ['ChildThreadCompleted', 'takes-unless-parked'],
  ['UserQuestionAsked', 'takes'],
  ['CommandPermissionRequested', 'takes'],
  ['McpPermissionRequested', 'takes'],
  ['McpConsentRequested', 'takes'],
  ['CodingAgentPermissionRequest', 'leaves'],
  ['MissingHardeningDetected', 'leaves'],
  ['MergeConflictDetected', 'leaves'],
  ['ChangeApplied', 'leaves'],
  ['ChangeDiscarded', 'leaves'],
  ['ChangeReverted', 'leaves'],
  ['ChangeApplyFailed', 'leaves'],
  ['SpokenMessageReceived', 'own-arm'],
  ['EventWaitCanceled', 'own-arm'],
  ['ChildThreadStopped', 'own-arm'],
  ['ChildThreadDetached', 'own-arm'],
  ['ChildReportFiled', 'own-arm'],
  ['UserOnlyMessagePosted', 'own-arm'],
  ['CodingAgentCommandReceived', 'own-arm'],
]);

function boundaryTakesTheTurn(type: string, previous: Exchange): boolean {
  const rule = BOUNDARY_CONTINUATION_HANDOFF.get(type);
  if (rule === 'takes') return true;
  if (rule === 'takes-unless-parked') return !dividerStillAwaitsUser(previous);
  return false;
}

export function dividerStillAwaitsUser(exchange: Exchange): boolean {
  const userEvent = exchange.userEvent;
  switch (userEvent.type) {
    case 'UserQuestionAsked':
      return !findQuestionAnswer(exchange, userEvent.tool_use_id);
    case 'CodingAgentPermissionRequest':
      return !exchange.steps.some(s =>
        s.event.type === 'CodingAgentPermissionResolved'
        && s.event.request_id === userEvent.request_id);
    case 'CommandPermissionRequested':
      return !exchange.steps.some(s =>
        s.event.type === 'CommandPermissionResolved'
        && s.event.request_id === userEvent.request_id);
    case 'McpPermissionRequested':
      return !exchange.steps.some(s =>
        s.event.type === 'McpPermissionResolved'
        && s.event.request_id === userEvent.request_id);
    case 'McpConsentRequested':
      return true;
    default:
      return false;
  }
}

function formRequestOwner(exchanges: Exchange[], requestId: string): Exchange | undefined {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const ex = exchanges[i];
    if (ex.steps.some(s => isFormRequest(s.event) && s.event.request_id === requestId)) return ex;
  }
  return undefined;
}

const NON_EXCHANGE_METADATA_EVENTS: ReadonlySet<string> = new Set([
  'CodingAgentMcpStatusesReported',
  ...Object.entries(EVENT_CLASSIFICATION)
    .filter(([evt, cls]) => cls === 'metadata' && evt.startsWith('Thread'))
    .map(([evt]) => evt),
  'QueuedMessageRemoved',
  'WorktreeCleaned',
  ...SIDE_QUESTION_TYPES,
  ...LEGACY_WIDGET_PIN_TYPES,
  'InlineWidgetSized',
  'AgentPaused',
  'AgentResumed',
  'HomeBoardArranged',
  'ApplyNowStarted',
  'ApplyNowEnded',
  'ModelCallFailed',
  'ChatSettingsChanged',
  'ChatTurnSettingsResolved',
  'CacheKeepAliveTurnedOff',
  'CacheKeepAliveTurnedOn',
  'WakeExpected',
  'UserQuestionQueued',
]);

export function isAuxiliaryCapture(event: { type: string }): boolean {
  if (event.type !== 'ContextCaptured') return false;
  const purpose = (event as { purpose?: string }).purpose;
  return purpose !== undefined && purpose !== 'turn';
}

export function isCacheKeepAliveAnchor(event: StoredEvent): boolean {
  return event.type === 'PromptInjected'
    && event.origin?.kind === 'engine'
    && event.origin.reason.kind === 'cache_keep_alive';
}

const KEEP_ALIVE_VISIBLE_CARDS: ReadonlySet<string> = new Set(KEEP_ALIVE_ENDING_EVENT_TYPES);

export function isCacheKeepAliveIdle(event: StoredEvent): boolean {
  return event.type === 'CodingAgentIdled' && event.reason === KEEP_ALIVE_IDLE_REASON;
}

function continuesWait(card: StoredEvent, status: CacheKeepAliveStatus | null | undefined): boolean {
  const last = (card as { status?: CacheKeepAliveStatus }).status;
  if (last?.state === 'ended') return status?.state === 'warming' && status.since === last.since;
  if (!last || !status || status.state !== 'warming') return true;
  if (last.state === 'turned_off') return Date.parse(status.since) <= Date.parse(last.at);
  return last.state === 'warming' && last.since === status.since;
}

function parkedCard(exchanges: Exchange[]): Exchange | null {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const ex = exchanges[i];
    if (isTurnlessBoundary(ex.userEvent)) continue;
    const awaits = opensAwaitingAnswer(ex);
    if (!awaits && ex.steps.length === 0) continue;
    const idle = ex.steps.every(({ event }) => event.type === 'CacheKeepAliveStatusChanged' || event.type === 'MessageHeld');
    return awaits && idle && dividerStillAwaitsUser(ex) ? ex : null;
  }
  return null;
}

function foldKeepAliveStatus(
  state: GroupFoldState,
  seq: number,
  event: Extract<StoredEvent, { type: 'CacheKeepAliveStatusChanged' }>,
  touched: Set<Exchange> | null,
): void {
  const status: CacheKeepAliveStatus | null | undefined = event.status;
  if (status?.state === 'not_kept') return;
  if (!status) {
    state.keepAliveRun = null;
    return;
  }
  const { beats } = status;
  const { keepAliveRun: run } = state;
  if (run && beats >= run.beats && continuesWait(runRecord(run), status)) {
    const next = { ...event, _eventId: runRecord(run)._eventId };
    if (run.step) run.step.event = next;
    else run.card.userEvent = next;
    run.beats = beats;
    touched?.add(run.card);
    return;
  }
  state.keepAliveRun = null;
  const parked = parkedCard(state.exchanges);
  if (parked) {
    const step = { seq, event };
    parked.steps.push(step);
    touched?.add(parked);
    state.keepAliveRun = { card: parked, beats, step };
    return;
  }
  const card: Exchange = { userEvent: event, userSeq: seq, steps: [] };
  state.exchanges.push(card);
  touched?.add(card);
  state.keepAliveRun = { card, beats, step: null };
}

function runRecord(run: NonNullable<GroupFoldState['keepAliveRun']>): StoredEvent {
  return run.step ? run.step.event : run.card.userEvent;
}

function isKeepAliveTurnEvent(thread: ThreadState, event: StoredEvent): boolean {
  if (isCacheKeepAliveAnchor(event) || isCacheKeepAliveIdle(event)) return true;
  const req = requestEventIdOf(event);
  if (req === undefined || !thread.keepAliveAnchorIds?.has(req)) return false;
  if (!KEEP_ALIVE_VISIBLE_CARDS.has(event.type)) return true;
  thread.keepAliveAnchorIds.delete(req);
  return false;
}

export function hasContentEvents(events: Map<number, StoredEvent>): boolean {
  for (const event of events.values()) {
    if (event.type === 'CodingAgentCommandReceived' || EVENT_CLASSIFICATION[event.type] !== 'metadata') return true;
  }
  return false;
}

function foldsAsEffortRestart(state: GroupFoldState, event: StoredEvent): Exchange | null {
  if (event.type !== 'ContinuationStarted' || event.reason !== CONTINUATION_EFFORT_RESTART_REASON) return null;
  const turn = state.effortRestartedTurn;
  state.effortRestartedTurn = null;
  return turn;
}

function findAbsorbTarget(
  state: GroupFoldState,
  current: Exchange | null,
  exchanges: Exchange[],
  event: StoredEvent,
): Exchange | null {
  if (event.type !== 'PromptInjected') return null;
  if (event.mode === 'engine'
      && current
      && current.userEvent.type === 'ContinuationStarted') {
    return current;
  }
  if (event.injected_message_id) {
    const own = exchanges.find(ex =>
      ex.userEvent.type === 'MessageReceived' && ex.userEvent._eventId === event.injected_message_id,
    );
    return own ?? state.reqIdRedirect.get(event.injected_message_id) ?? null;
  }
  return null;
}

const REQUEST_ID_ROUTED_TYPES: ReadonlySet<string> = new Set([
  'ThoughtStreamed',
  'MemoryRecalled',
  'MemorySearched',
  'ContextCaptured',
  'ContextAssembled',
  'ContextTokensMeasured',
  'ToolCalled',
  'ToolResult',
  'TextStreamed',
  'ResponseGenerated',
  'ResponseCanceled',
  'ResponseAborted',
  'ResponseFailed',
  'CommandCheckpointed',
  'CommandCheckpointReverted',
]);

function shouldRouteByRequestId(event: StoredEvent): boolean {
  if (!REQUEST_ID_ROUTED_TYPES.has(event.type)) return false;
  switch (event.type) {
    case 'ResponseGenerated':
    case 'ResponseCanceled':
    case 'ResponseAborted':
    case 'ResponseFailed':
    case 'ContextCaptured':
    case 'ContextAssembled':
    case 'ContextTokensMeasured':
      return (event as { channel?: string }).channel !== 'claude_code';
    default:
      return true;
  }
}

function requestEventIdOf(event: { type: string }): string | undefined {
  return (event as { request_event_id?: string }).request_event_id;
}

function rerunReqIdOf(event: StoredEvent): string | undefined {
  return shouldRouteByRequestId(event) ? requestEventIdOf(event) : undefined;
}

export function isCallerUtterance(event: { type: string; voice_session_id?: string }): boolean {
  if (event.type === 'SpokenMessageReceived') return true;
  return event.type === 'MessageReceived' && !!event.voice_session_id;
}

const UNCLAIMED_UTTERANCE_MEMORY = 8;

function claimKey(text: string | undefined): string | undefined {
  const trimmed = text?.trim();
  return trimmed ? trimmed : undefined;
}

export function claimUtteranceRows(thread: ThreadState): void {
  const owed = thread.unclaimedUtterances;
  if (!owed || owed.length === 0) return;
  const rows = thread.liveUtterances ?? [];
  let kept = [...rows];
  const unmatched: string[] = [];
  for (const words of owed) {
    const at = kept.findIndex(row => claimKey(row.text) === words);
    if (at === -1) unmatched.push(words);
    else kept.splice(at, 1);
    kept = kept.filter(row => !isEarlierInStretch(row.text, words));
  }
  thread.unclaimedUtterances = unmatched.slice(-UNCLAIMED_UTTERANCE_MEMORY);
  if (kept.length !== rows.length) thread.liveUtterances = kept;
}

function isEarlierInStretch(text: string | undefined, landed: string): boolean {
  const words = claimKey(text);
  return words !== undefined && words.length < landed.length && landed.startsWith(words);
}

export function recordCallerUtterance(thread: ThreadState, text: string): void {
  const words = claimKey(text);
  if (!words) return;
  thread.unclaimedUtterances = [...(thread.unclaimedUtterances ?? []), words];
  claimUtteranceRows(thread);
}

export function offerCallerUtterance(thread: ThreadState, text: string): void {
  const words = claimKey(text);
  const rows = thread.liveUtterances;
  if (!words || !rows || rows.length === 0) return;
  const at = rows.findIndex(row => claimKey(row.text) === words);
  if (at !== -1) thread.liveUtterances = rows.filter((_, i) => i !== at);
}

export function exchangeHoldsNoTurn(exchange: Exchange): boolean {
  if (exchange.tookTheTurn) return false;
  if (isLiveUtteranceRow(exchange.userEvent)) return true;
  const type = exchange.userEvent.type;
  return type === 'SpokenMessageReceived' || type === 'SpokenReplyGenerated';
}

export function isCallBoundary(event: { type: string; voice_session_id?: string }): boolean {
  return event.type === 'SpokenReplyGenerated' || isCallerUtterance(event);
}

export function toolUseIdOf(event: { type: string }): string | undefined {
  const id = (event as { tool_use_id?: string }).tool_use_id;
  return id ? id : undefined;
}

function grownSpokenRow(prev: StoredEvent, next: StoredEvent): StoredEvent | null {
  if (prev.type !== next.type) return null;
  if (prev.type !== 'SpokenMessageReceived' && prev.type !== 'SpokenReplyGenerated') return null;
  if (isLiveCallRow(prev) || isLiveCallRow(next)) return null;
  const sessions = prev as { session_id?: string };
  if (sessions.session_id !== (next as { session_id?: string }).session_id) return null;
  const from = instantMicros(prev.created);
  const to = instantMicros(next.created);
  if (from === null || to === null) return null;
  if (!isOneUtterance((to - from) / 1_000_000, true)) return null;
  const text = joinSpoken((prev as { text?: string }).text ?? '', (next as { text?: string }).text ?? '');
  const interrupted = (next as { interrupted?: boolean }).interrupted;
  const began = happenedAt(prev) ?? from;
  const age = prev.type === 'SpokenReplyGenerated'
    ? { spoken_secs_before: (to - began) / 1_000_000 }
    : {};
  return {
    ...prev,
    text,
    created: next.created,
    ...age,
    ...(interrupted === undefined ? {} : { interrupted }),
  } as StoredEvent;
}

export const TERMINAL_EVENT_TYPES: ReadonlySet<string> = new Set([
  'ResponseGenerated',
  'ResponseFailed',
  'ResponseCanceled',
  'ResponseAborted',
  'CodingAgentIdled',
]);

export function latestRunSteps(exchange: Exchange): Exchange['steps'] {
  for (let i = exchange.steps.length - 1; i >= 0; i--) {
    if (exchange.steps[i].event.type === 'ContinuationStarted') return exchange.steps.slice(i + 1);
  }
  return exchange.steps;
}

function stillRunning(exchange: Exchange): boolean {
  return !latestRunSteps(exchange).some(({ event }) => TERMINAL_EVENT_TYPES.has(event.type));
}

function hasNonChildStep(exchange: Exchange): boolean {
  return exchange.steps.some(({ event }) => !isChildNote(event));
}

function canTakeChildNote(exchange: Exchange): boolean {
  const anchor = exchange.userEvent;
  if (exchangeHoldsNoTurn(exchange) || dividerStillAwaitsUser(exchange) || exchange.continuationMoved) return false;
  if (TERMINAL_EVENT_TYPES.has(anchor.type)) return false;
  if (anchor.type === 'UserQuestionAsked') {
    const answer = findQuestionAnswer(exchange, anchor.tool_use_id)?.answer.kind;
    if (wakesNoAgent(anchor) || answer === 'Canceled' || answer === 'Superseded') return false;
  }
  if (isChangeLifecycleEvent(anchor) && !changePanelHasContinuation(exchange)) return false;
  if ((anchor.type === 'MissingHardeningDetected' || anchor.type === 'MergeConflictDetected')
    && !exchange.steps.some(isLoopStep)) return false;
  return !latestRunSteps(exchange).some(({ event }) =>
    TERMINAL_EVENT_TYPES.has(event.type) || event.type === 'SessionEnded' || event.type === 'EventWaitStarted');
}

function settlesEndedTurn(event: StoredEvent): boolean {
  if (event.type === 'CodingAgentIdled') return true;
  return onCodingAgentChannel(event)
    && EVENT_CLASSIFICATION[event.type] === 'metadata'
    && event.type !== 'SessionStarted';
}

function turnSettledLate(state: GroupFoldState, current: Exchange | null, event: StoredEvent): Exchange | null {
  if (!settlesEndedTurn(event)) return null;
  if (current?.userEvent.type !== 'MessageReceived' || hasNonChildStep(current)) return null;
  let i = state.exchanges.indexOf(current) - 1;
  while (i >= 0 && !hasNonChildStep(state.exchanges[i])) i--;
  const previous = state.exchanges[i];
  if (!previous) return null;
  const run = latestRunSteps(previous);
  const ended = run.some(({ event: step }) => TERMINAL_EVENT_TYPES.has(step.type));
  const idled = run.some(({ event: step }) => step.type === 'CodingAgentIdled');
  return ended && !idled ? previous : null;
}

function onCodingAgentChannel(event: { type: string }): boolean {
  return (event as { channel?: string }).channel === 'claude_code';
}

function provesReadsReported(event: StoredEvent): boolean {
  return event.type === 'CodingAgentInputRead'
    || (event.type === 'CodingAgentPromptSent' && !!event.input_event_id);
}

function waitsForTheAgent(
  state: GroupFoldState,
  event: StoredEvent,
  turn: Exchange,
  othersWait = state.unreadMessages.length > 0,
): boolean {
  if (event.type !== 'MessageReceived' || !onCodingAgentChannel(event)) return false;
  if (!state.readsReported) return false;
  if (othersWait) return true;
  const agentTurn = onCodingAgentChannel(turn.userEvent)
    || turn.steps.some(({ event: step }) => step.type.startsWith('CodingAgent'));
  return agentTurn && stillRunning(turn);
}

function chatTurnQueuedBehind(state: GroupFoldState, current: Exchange | null, event: StoredEvent): Exchange | null {
  if (VOICE_ONLY_STEP_TYPES.has(event.type) || UNANCHORABLE_ASYNC_EVENTS.has(event.type)) return null;
  if (!current || !isUningestedMessage(current)) return null;
  const turn = chatTurnOwner(state, null);
  return turn && turn !== current && stillRunning(turn) ? turn : null;
}

function turnEndedBy(state: GroupFoldState, current: Exchange | null, event: StoredEvent): Exchange | null {
  const routed = chatTurnQueuedBehind(state, current, event);
  if (routed || !current) return routed ?? current;
  const waitingChat = (ex: Exchange) => isUningestedMessage(ex) && !onCodingAgentChannel(ex.userEvent);
  if (!waitingChat(current)) return current;
  let oldest = state.exchanges.indexOf(current);
  while (oldest > 0 && waitingChat(state.exchanges[oldest - 1])) oldest--;
  return state.exchanges[oldest] ?? current;
}

function takeTurnFromQueue(state: GroupFoldState, message: Exchange): Exchange {
  reanchorAboveSpeechAndQueue(state.exchanges, message);
  return message;
}

function handOverTheTurn(
  state: GroupFoldState,
  previous: Exchange,
  next: Exchange,
  touched: Set<Exchange> | null,
): void {
  previous.continuationMoved = true;
  touched?.add(previous);
  for (const [reqId, exchange] of state.reqIdRedirect.entries()) {
    if (exchange === previous) state.reqIdRedirect.set(reqId, next);
  }
  const anchorId = previous.userEvent._eventId;
  if (anchorId) state.reqIdRedirect.set(anchorId, next);
}

function findExchangeByAnchorId(exchanges: Exchange[], anchorId: string): Exchange | null {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    if (exchanges[i].userEvent._eventId === anchorId) return exchanges[i];
  }
  return null;
}

export function sortEventsChronologically(
  events: Map<number, StoredEvent>,
): SequencedEvent[] {
  const keyed = [...events.entries()].map(([seq, event]) => ({
    seq,
    event,
    micros: instantMicros(event.created),
  }));
  keyed.sort((a, b) => compareSortKeys(a.micros, a.seq, b.micros, b.seq));
  return keyed.map(({ seq, event }) => ({ seq, event }));
}

const QUESTION_OVERTAKEN_STEP_TYPES: ReadonlySet<string> = new Set(QUESTION_OVERTAKEN_EVENT_TYPES);

function overtakesQuestion(event: { type: string }, questionToolUseId: string | null): boolean {
  if (QUESTION_OVERTAKEN_STEP_TYPES.has(event.type)) return true;
  if (event.type !== 'CodingAgentToolResult') return false;
  return questionToolUseId === null
    || (questionToolUseId !== '' && toolUseIdOf(event) === questionToolUseId);
}

function markOvertakenQuestionDividers(exchanges: Exchange[]): void {
  for (const exchange of exchanges) {
    markOvertakenForExchange(exchange);
  }
}

function markOvertakenForExchange(exchange: Exchange): void {
  if (exchange.userEvent.type !== 'UserQuestionAsked') return;
  const toolUseId = exchange.userEvent.tool_use_id;
  if (findQuestionAnswer(exchange, toolUseId) || isEngineRaisedCard(exchange.userEvent)) {
    exchange.questionOvertaken = false;
    return;
  }
  exchange.questionOvertaken = exchange.steps.some(s => overtakesQuestion(s.event, toolUseId));
}

interface GroupFoldState {
  exchanges: Exchange[];
  current: Exchange | null;
  toolCallOwners: Map<string, Exchange>;
  chatToolCallOwners: Map<string, Exchange>;
  questionDividerOwners: Map<string, Exchange>;
  permissionDividerOwners: Map<string, Exchange>;
  lastDelegationHost: Exchange | null;
  turnDividers: Map<string, Exchange>;
  unreadMessages: Exchange[];
  readsReported: boolean;
  gatedCalls: Map<string, { exchange: Exchange; seq: number }>;
  pendingRelease: { holder: Exchange; held: Extract<StoredEvent, { type: 'MessageHeld' }> } | null;
  effortRestartedTurn: Exchange | null;
  reqIdRedirect: Map<string, Exchange>;
  resolvedReqIds: Set<string>;
  abortReqIds: Set<string>;
  keepAliveAnchors: Set<string>;
  keepAliveRun: { card: Exchange; beats: number; step: SequencedEvent | null } | null;
  lastChatTurnReqId?: string;
  paged: boolean;
}

function newFoldState(paged: boolean): GroupFoldState {
  return {
    paged,
    exchanges: [],
    current: null,
    toolCallOwners: new Map(),
    chatToolCallOwners: new Map(),
    questionDividerOwners: new Map(),
    permissionDividerOwners: new Map(),
    lastDelegationHost: null,
    turnDividers: new Map(),
    unreadMessages: [],
    readsReported: false,
    gatedCalls: new Map(),
    pendingRelease: null,
    effortRestartedTurn: null,
    reqIdRedirect: new Map(),
    resolvedReqIds: new Set(),
    abortReqIds: new Set(),
    keepAliveAnchors: new Set(),
    keepAliveRun: null,
  };
}

const CALL_ROW_TYPES: ReadonlySet<string> = new Set([
  'SpokenReplyGenerated',
  'VoiceSessionStarted',
  'VoiceSessionEnded',
]);

export const VOICE_ONLY_STEP_TYPES: ReadonlySet<string> = new Set([
  'SpokenReplyGenerated',
  'VoiceSessionStarted',
  'VoiceSessionEnded',
  'WorkDelegated',
]);

const DRAWS_NO_ROW: ReadonlySet<string> = new Set([
  'VoiceSessionStarted',
  'VoiceSessionEnded',
  'WorkDelegated',
]);

function readerMetNothing(exchange: Exchange): boolean {
  return exchange.steps.every(({ event }) => DRAWS_NO_ROW.has(event.type));
}

export const UNANCHORABLE_ASYNC_EVENTS: ReadonlySet<string> = new Set([
  'BackgroundBashCompleted',
]);

export function isUningestedMessage(exchange: Exchange): boolean {
  if (exchange.userEvent.type !== 'MessageReceived') return false;
  if (isLiveUtteranceRow(exchange.userEvent)) return false;
  return !exchange.steps.some(isLoopStep);
}

function isLoopStep({ event }: SequencedEvent): boolean {
  return !isChildNote(event) && !VOICE_ONLY_STEP_TYPES.has(event.type) && !UNANCHORABLE_ASYNC_EVENTS.has(event.type);
}

export function messageReadTimestamp(exchange: Exchange): string | undefined {
  if (exchange.inputRead) return exchange.inputRead.created;
  return exchange.steps.find(isLoopStep)?.event.created;
}

export function isWaitingTypedMessage(exchange: Exchange): boolean {
  if (!isUningestedMessage(exchange)) return false;
  return !(exchange.userEvent as { voice_session_id?: string }).voice_session_id;
}

function callRowTarget(exchanges: Exchange[]): Exchange | null {
  const at = liveReplyTargetIndex(exchanges);
  return at === -1 ? null : exchanges[at];
}

function liveReplyTargetIndex(exchanges: Exchange[]): number {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const exchange = exchanges[i];
    if (isWaitingTypedMessage(exchange)) continue;
    if (isTurnlessBoundary(exchange.userEvent)) continue;
    if (isUnsentExchange(exchange)) continue;
    return i;
  }
  return -1;
}

export function isUnsentExchange(exchange: Exchange): boolean {
  return exchange.userEvent._unsent === true;
}

export function groupIntoExchanges(events: Map<number, StoredEvent>, paged = false): Exchange[] {
  return foldSorted(sortEventsChronologically(events), paged).exchanges;
}

interface IncrementalCache {
  fold: GroupFoldState;
  processedCount: number;
  lastCreatedMicros: number | null;
  lastSeq: number;
  cacheable: boolean;
}

const incrementalCache = new WeakMap<Map<number, StoredEvent>, IncrementalCache>();

export function compareSortKeys(
  aMicros: number | null,
  aSeq: number,
  bMicros: number | null,
  bSeq: number,
): number {
  if (aMicros !== null && bMicros !== null && aMicros !== bMicros) {
    return aMicros < bMicros ? -1 : 1;
  }
  return aSeq - bSeq;
}

function rebuildIncrementalCache(events: Map<number, StoredEvent>, paged: boolean): Exchange[] {
  const sorted = sortEventsChronologically(events);
  let cacheable = true;
  for (const { event } of sorted) {
    if (!event.created) {
      cacheable = false;
      break;
    }
  }
  const fold = foldSorted(sorted, paged);
  const last = sorted.length > 0 ? sorted[sorted.length - 1] : null;
  incrementalCache.set(events, {
    fold,
    processedCount: events.size,
    lastCreatedMicros: instantMicros(last?.event.created),
    lastSeq: last?.seq ?? Number.MIN_SAFE_INTEGER,
    cacheable,
  });
  return [...fold.exchanges];
}

function groupIntoExchangesCached(events: Map<number, StoredEvent>, paged: boolean): Exchange[] {
  const cache = incrementalCache.get(events);
  if (!cache) return rebuildIncrementalCache(events, paged);
  if (cache.fold.paged !== paged) return rebuildIncrementalCache(events, paged);
  if (!cache.cacheable) return groupIntoExchanges(events, paged);
  if (events.size === cache.processedCount) return [...cache.fold.exchanges];
  if (events.size < cache.processedCount) return rebuildIncrementalCache(events, paged);

  const appended: SequencedEvent[] = [];
  let i = 0;
  for (const [seq, event] of events) {
    if (i++ < cache.processedCount) continue;
    appended.push({ seq, event });
  }
  appended.sort((a, b) =>
    compareSortKeys(instantMicros(a.event.created), a.seq, instantMicros(b.event.created), b.seq),
  );

  let prevMicros = cache.lastCreatedMicros;
  let prevSeq = cache.lastSeq;
  const batchAbortReqIds = new Set<string>();
  for (const { seq, event } of appended) {
    const micros = instantMicros(event.created);
    if (micros === null) {
      cache.cacheable = false;
      return groupIntoExchanges(events, paged);
    }
    if (compareSortKeys(micros, seq, prevMicros, prevSeq) < 0) {
      return rebuildIncrementalCache(events, paged);
    }
    if (provesReadsReported(event) && !cache.fold.readsReported) {
      return rebuildIncrementalCache(events, paged);
    }
    const reqId = rerunReqIdOf(event);
    if (event.type === 'ResponseAborted' && reqId) {
      batchAbortReqIds.add(reqId);
    }
    if (
      (event.type === 'ResponseGenerated' || event.type === 'ResponseFailed') &&
      reqId &&
      (cache.fold.abortReqIds.has(reqId) || batchAbortReqIds.has(reqId))
    ) {
      return rebuildIncrementalCache(events, paged);
    }
    prevMicros = micros;
    prevSeq = seq;
  }

  const touched = new Set<Exchange>();
  for (const { seq, event } of appended) {
    const reqId = rerunReqIdOf(event);
    const superseded =
      event.type === 'ResponseAborted' && !!reqId && cache.fold.resolvedReqIds.has(reqId);
    foldEvent(cache.fold, seq, event, superseded, touched);
  }
  for (const exchange of touched) {
    markOvertakenForExchange(exchange);
    exchange.revision = (exchange.revision ?? 0) + 1;
  }
  cache.processedCount = events.size;
  cache.lastCreatedMicros = prevMicros;
  cache.lastSeq = prevSeq;
  return [...cache.fold.exchanges];
}

function foldSorted(sorted: SequencedEvent[], paged: boolean): GroupFoldState {
  const resolvedReqIds = new Set<string>();
  for (const { event } of sorted) {
    if (event.type !== 'ResponseGenerated' && event.type !== 'ResponseFailed') continue;
    const reqId = rerunReqIdOf(event);
    if (reqId) resolvedReqIds.add(reqId);
  }
  const legacySupersededAbortSeqs = new Set<number>();
  for (const { seq, event } of sorted) {
    if (event.type !== 'ResponseAborted') continue;
    const reqId = rerunReqIdOf(event);
    if (reqId && resolvedReqIds.has(reqId)) legacySupersededAbortSeqs.add(seq);
  }

  const state = newFoldState(paged);
  state.readsReported = sorted.some(({ event }) => provesReadsReported(event));
  for (const { seq, event } of sorted) {
    foldEvent(state, seq, event, legacySupersededAbortSeqs.has(seq), null);
  }
  markOvertakenQuestionDividers(state.exchanges);
  return state;
}

function reanchorAboveSpeechAndQueue(exchanges: Exchange[], exchange: Exchange): void {
  const from = exchanges.indexOf(exchange);
  if (from === -1) return;
  let wall = exchanges.length;
  for (let i = from + 1; i < exchanges.length; i++) {
    if (isCallerUtterance(exchanges[i].userEvent) || exchanges[i].awaitingRead) {
      wall = i;
      break;
    }
  }
  const to = wall - 1;
  if (to <= from) return;
  exchanges.splice(from, 1);
  exchanges.splice(to, 0, exchange);
}

function callerSpokeAfter(exchanges: Exchange[], exchange: Exchange): boolean {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    if (exchanges[i] === exchange) return false;
    if (isCallerUtterance(exchanges[i].userEvent)) return true;
  }
  return false;
}

function reanchorResolvedDivider(
  state: GroupFoldState,
  divider: Exchange,
  current: Exchange | null,
): Exchange | null {
  if (divider.userEvent.type === 'UserQuestionAsked' && wakesNoAgent(divider.userEvent)) return current;
  if (
    !ownsRequestContinuation(state, divider)
    && agentWorkedSince(state.exchanges, state.exchanges.indexOf(divider))
  ) {
    return current;
  }
  reanchorAboveSpeechAndQueue(state.exchanges, divider);
  if (callerSpokeAfter(state.exchanges, divider)) return current;
  divider.continuationMoved = false;
  return divider;
}

function ownsRequestContinuation(state: GroupFoldState, divider: Exchange): boolean {
  for (const target of state.reqIdRedirect.values()) {
    if (target === divider) return true;
  }
  return false;
}

export function agentWorkedSince(exchanges: Exchange[], from: number): boolean {
  if (from === -1) return false;
  const card = exchanges[from].userEvent;
  const questionToolUseId = card.type === 'UserQuestionAsked' ? card.tool_use_id : null;
  for (let i = from; i < exchanges.length; i++) {
    if (exchanges[i].steps.some(s => overtakesQuestion(s.event, questionToolUseId))) return true;
  }
  return false;
}

function chatTurnOwner(state: GroupFoldState, previousCurrent: Exchange | null): Exchange | null {
  const reqId = state.lastChatTurnReqId;
  if (!reqId) return previousCurrent;
  return state.reqIdRedirect.get(reqId)
    ?? findExchangeByAnchorId(state.exchanges, reqId)
    ?? previousCurrent;
}

function gatedCallOf(
  state: GroupFoldState,
  event: StoredEvent,
  previousCurrent: Exchange | null,
): { exchange: Exchange; seq: number } | null {
  const isCodingAgent = event.type === 'CodingAgentPermissionRequest';
  const toolUseId = (event as { tool_use_id?: string }).tool_use_id;
  const exchange = isCodingAgent
    ? (toolUseId ? state.toolCallOwners.get(toolUseId) : undefined)
    : chatTurnOwner(state, previousCurrent);
  if (!exchange) return null;
  for (let i = exchange.steps.length - 1; i >= 0; i--) {
    const step = exchange.steps[i];
    if (isCodingAgent) {
      if (step.event.type === 'CodingAgentToolCalled' && toolUseIdOf(step.event) === toolUseId) {
        return { exchange, seq: step.seq };
      }
    } else if (step.event.type === 'ToolCalled') {
      return { exchange, seq: step.seq };
    }
  }
  return null;
}

function settleGatedCall(
  state: GroupFoldState,
  requestId: string,
  allowed: boolean,
  touched: Set<Exchange> | null,
): void {
  const gated = state.gatedCalls.get(requestId);
  if (!gated) return;
  state.gatedCalls.delete(requestId);
  gated.exchange.blockedStepSeqs?.delete(gated.seq);
  if (!allowed) {
    (gated.exchange.deniedStepSeqs ??= new Set()).add(gated.seq);
  }
  touched?.add(gated.exchange);
}

function foldEvent(
  state: GroupFoldState,
  seq: number,
  event: StoredEvent,
  isLegacySupersededAbort: boolean,
  touched: Set<Exchange> | null,
): void {
  const { exchanges, toolCallOwners, chatToolCallOwners, questionDividerOwners, permissionDividerOwners, reqIdRedirect } = state;
  let current = state.current;
  {
    const reqId = rerunReqIdOf(event);
    if ((event.type === 'ResponseGenerated' || event.type === 'ResponseFailed') && reqId) {
      state.resolvedReqIds.add(reqId);
    }
    if (event.type === 'ResponseAborted' && reqId) {
      state.abortReqIds.add(reqId);
    }
  }
  const step = (): void => {
    if (event.type === 'EventWaitTimingUpdated') {
      const target = exchanges.find((exchange) => exchange.steps.some(({ event: step }) =>
        step.type === 'EventWaitStarted' && step.wait_id === event.wait_id));
      if (target) {
        appendStep(target, seq, event);
        touched?.add(target);
      }
      return;
    }
    if (event.type === 'TriggerRunInterrupted') {
      const fire = exchanges.findIndex(ex => ex.userEvent._eventId === event.fire_event_id);
      let target = fire >= 0 ? exchanges[fire] : undefined;
      for (let i = exchanges.length - 1; i > fire; i--) {
        if (exchanges[i].userEvent.type === 'TriggerStarted') break;
        if (exchanges[i].userEvent.type === 'ResponseAborted') {
          target = exchanges[i];
          break;
        }
      }
      if (target) appendStep(target, seq, event);
      else {
        target = { userEvent: event, userSeq: seq, steps: [], continuationFragment: true };
        exchanges.push(target);
      }
      touched?.add(target);
      return;
    }
    if (event.type === 'CodingAgentInputRead') {
      const read = exchanges.find(ex => ex.userEvent._eventId === event.input_event_id);
      if (read && !read.inputRead) {
        read.inputRead = event;
        touched?.add(read);
      }
      const at = read ? state.unreadMessages.indexOf(read) : -1;
      if (read && at !== -1) {
        for (const message of state.unreadMessages.splice(0, at + 1)) {
          message.awaitingRead = undefined;
          touched?.add(message);
        }
        current = takeTurnFromQueue(state, read);
      }
      return;
    }
    if (event.type === 'QueuedMessageRemoved') {
      const at = state.unreadMessages.findIndex(ex => ex.userEvent._eventId === event.removed_message_id);
      if (at !== -1) state.unreadMessages.splice(at, 1);
      return;
    }
    if (event._unsent && event.type === 'ResponseFailed') {
      const unsent = exchanges.find((ex) => ex.userSeq === seq - 1);
      if (unsent) {
        appendStep(unsent, seq, event);
        touched?.add(unsent);
      }
      return;
    }
    if (event.type === 'CacheKeepAliveStatusChanged') {
      foldKeepAliveStatus(state, seq, event, touched);
      return;
    }
    if (NON_EXCHANGE_METADATA_EVENTS.has(event.type)) return;
    if (isAuxiliaryCapture(event)) return;
    if (isCacheKeepAliveAnchor(event)) {
      if (event._eventId) state.keepAliveAnchors.add(event._eventId);
      return;
    }
    if (isCacheKeepAliveIdle(event)) return;
    const keepAliveReq = requestEventIdOf(event);
    if (keepAliveReq !== undefined && state.keepAliveAnchors.has(keepAliveReq)) {
      if (!KEEP_ALIVE_VISIBLE_CARDS.has(event.type)) return;
      state.keepAliveAnchors.delete(keepAliveReq);
    }
    if (event.type === 'CodingAgentPromptSent' && current && stillRunning(current)
      && state.unreadMessages.some(({ userEvent }) =>
        userEvent.type === 'MessageReceived' && userEvent.text === event.text)) {
      return;
    }

    if (event.type === 'ChangeSummarized') {
      for (let i = exchanges.length - 1; i >= 0; i--) {
        const holder = exchanges[i];
        if (!holder.steps.some(s => s.event.type === 'ChangeProposed' && s.event.change_id === event.change_id)) continue;
        holder.steps.push({ seq, event });
        touched?.add(holder);
        break;
      }
      return;
    }

    if (event.type === 'HeldMessageReleased') {
      for (const holder of exchanges) {
        const held = holder.steps.find(s =>
          s.event.type === 'MessageHeld' && s.event._eventId === event.held_message_id);
        if (held?.event.type !== 'MessageHeld') continue;
        holder.steps.push({ seq, event });
        touched?.add(holder);
        state.pendingRelease = { holder, held: held.event };
        return;
      }
    }

    const callTarget = CALL_ROW_TYPES.has(event.type) ? callRowTarget(exchanges) : null;
    const reqId = shouldRouteByRequestId(event) ? requestEventIdOf(event) : undefined;
    if (reqId) state.lastChatTurnReqId = reqId;
    const owner = reqId
      ? (reqIdRedirect.get(reqId) ?? findExchangeByAnchorId(exchanges, reqId))
      : null;
    const terminalTarget = event.type === 'ResponseAborted' || event.type === 'ResponseCanceled'
      ? owner ?? turnEndedBy(state, current, event)
      : null;

    if (event.type === 'ResponseAborted' || (event.type === 'ResponseCanceled' && event.cause !== 'effort_restart')) {
      state.effortRestartedTurn = null;
    }

    if (event.type === 'ResponseAborted' && !isLegacySupersededAbort) {
      if (terminalTarget && terminalTarget.userEvent.type !== 'ResponseAborted') {
        terminalTarget.steps.push({ seq, event });
        touched?.add(terminalTarget);
        current = { userEvent: event, userSeq: seq, steps: [] };
        exchanges.push(current);
        touched?.add(current);
        return;
      }
    }
    if (event.type === 'ResponseCanceled') {
      if (terminalTarget && terminalTarget.userEvent.type !== 'ResponseCanceled') {
        if (isRedirectCancel(event.cause)) {
          terminalTarget.steps.push({ seq, event });
          touched?.add(terminalTarget);
          if (event.cause === 'effort_restart') state.effortRestartedTurn = terminalTarget;
          return;
        }
        terminalTarget.steps.push({ seq, event });
        touched?.add(terminalTarget);
        const teller = (reqId ? state.turnDividers.get(reqId) : undefined) ?? terminalTarget;
        if (teller.userEvent.type === 'UserQuestionAsked') {
          const answered = findQuestionAnswer(teller, teller.userEvent.tool_use_id);
          if (answered?.answer.kind === 'Canceled' || answered?.answer.kind === 'Superseded') {
            return;
          }
        }
        current = { userEvent: event, userSeq: seq, steps: [] };
        exchanges.push(current);
        touched?.add(current);
        return;
      }
    }
    {
      const resultOf = event.type === 'CodingAgentToolResult' ? toolUseIdOf(event) : undefined;
      const parent = parentToolUseIdOf(event);
      const callOwner = (resultOf ? toolCallOwners.get(resultOf) : undefined)
        ?? (parent ? toolCallOwners.get(parent) : undefined);
      if (callOwner && callOwner !== current) {
        appendStep(callOwner, seq, event);
        touched?.add(callOwner);
        const id = event.type === 'CodingAgentToolCalled' ? toolUseIdOf(event) : undefined;
        if (id) toolCallOwners.set(id, callOwner);
        return;
      }
    }
    if (event.type === 'ToolResult') {
      const tcId = (event as { tool_called_event_id?: string }).tool_called_event_id;
      const callOwner = tcId ? chatToolCallOwners.get(tcId) : undefined;
      if (callOwner) {
        callOwner.steps.push({ seq, event });
        touched?.add(callOwner);
        return;
      }
    }
    if (event.type === 'UserQuestionAnswered') {
      const dividerOwner = questionDividerOwners.get(event.tool_use_id);
      if (dividerOwner) {
        dividerOwner.steps.push({ seq, event });
        touched?.add(dividerOwner);
        current = reanchorResolvedDivider(state, dividerOwner, current);
        return;
      }
    }
    if (
      event.type === 'CodingAgentPermissionResolved'
      || event.type === 'CommandPermissionResolved'
      || event.type === 'McpPermissionResolved'
    ) {
      settleGatedCall(state, event.request_id, event.allowed, touched);
      const dividerOwner = permissionDividerOwners.get(event.request_id);
      if (dividerOwner) {
        dividerOwner.steps.push({ seq, event });
        touched?.add(dividerOwner);
        current = reanchorResolvedDivider(state, dividerOwner, current);
        return;
      }
    }
    if (
      event.type === 'CodingAgentCommandAnswered'
      || event.type === 'AgentSignInRequested'
      || event.type === 'TerminalCommandSuggested'
    ) {
      const commandId = requestEventIdOf(event);
      const command = commandId ? findExchangeByAnchorId(exchanges, commandId) : null;
      if (command?.userEvent.type === 'CodingAgentCommandReceived') {
        command.steps.push({ seq, event });
        touched?.add(command);
        return;
      }
    }
    if (event.type === 'AgentSignInRequested') {
      const signIn: Exchange = { userEvent: event, userSeq: seq, steps: [{ seq, event }] };
      exchanges.push(signIn);
      touched?.add(signIn);
      return;
    }
    if (event.type === 'FormRequestResolved') {
      const owner = formRequestOwner(exchanges, event.request_id);
      if (owner) {
        owner.steps.push({ seq, event });
        touched?.add(owner);
      }
      return;
    }
    if (event.type === 'WorkDelegated' && current && isCallerUtterance(current.userEvent)) {
      current.steps.push({ seq, event });
      current.tookTheTurn = true;
      state.lastDelegationHost = current;
      if (event._eventId) reqIdRedirect.set(event._eventId, current);
      touched?.add(current);
      return;
    }
    if (isChildNote(event)) {
      const turn = chatTurnQueuedBehind(state, current, event) ?? current;
      if (turn && canTakeChildNote(turn) && !parkedCard(exchanges)) {
        turn.steps.push({ seq, event });
        touched?.add(turn);
        return;
      }
    }
    const restarted = foldsAsEffortRestart(state, event);
    if (restarted) {
      restarted.steps.push({ seq, event });
      touched?.add(restarted);
      if (event._eventId) reqIdRedirect.set(event._eventId, restarted);
      current = restarted;
      return;
    }
    if (isExchangeStartEvent(event) && !isTurnlessBoundary(event)) state.effortRestartedTurn = null;
    const absorbTarget = findAbsorbTarget(state, current, exchanges, event);
    if (absorbTarget) {
      reanchorAboveSpeechAndQueue(exchanges, absorbTarget);
      absorbTarget.steps.push({ seq, event });
      touched?.add(absorbTarget);
      absorbTarget.continuationMoved = false;
      const ownId = absorbTarget.userEvent._eventId;
      if (ownId) reqIdRedirect.set(ownId, absorbTarget);
      current = absorbTarget;
      if (event.type === 'PromptInjected') {
        const absorbedReqId = requestEventIdOf(event);
        if (absorbedReqId) reqIdRedirect.set(absorbedReqId, absorbTarget);
      }
    } else if (isExchangeStartEvent(event) && !isLegacySupersededAbort) {
      const delegationHost = state.lastDelegationHost;
      state.lastDelegationHost = null;
      if (delegationHost && event.type === 'MessageReceived' && isCallerUtterance(event)) {
        delegationHost.tookTheTurn = undefined;
        touched?.add(delegationHost);
      }
      const openUtterance = exchanges[exchanges.length - 1];
      if (
        event.type === 'SpokenMessageReceived'
        && openUtterance
        && readerMetNothing(openUtterance)
      ) {
        const grown = grownSpokenRow(openUtterance.userEvent, event);
        if (grown) {
          openUtterance.userEvent = grown;
          touched?.add(openUtterance);
          return;
        }
      }
      const previousCurrent = current;
      current = { userEvent: event, userSeq: seq, steps: [] };
      exchanges.push(current);
      touched?.add(current);
      if (isTurnlessBoundary(event) || event._unsent) {
        current = previousCurrent;
        return;
      }
      if (event.type === 'MessageReceived' && event.queued_for_permission) {
        if (onCodingAgentChannel(event)) {
          current.awaitingRead = true;
          state.unreadMessages.push(current);
        }
        current = previousCurrent;
        return;
      }
      if (event.type === 'MessageReceived' && previousCurrent && !stillRunning(previousCurrent)) {
        state.unreadMessages = state.unreadMessages.filter(stale => {
          if (stale.userEvent.queued_for_permission) return true;
          stale.awaitingRead = undefined;
          touched?.add(stale);
          return false;
        });
      }
      if (previousCurrent && waitsForTheAgent(state, event, previousCurrent)) {
        current.awaitingRead = true;
        state.unreadMessages.push(current);
        current = previousCurrent;
        return;
      }
      if (event.type === 'SpokenMessageReceived') {
        if (previousCurrent && !exchangeHoldsNoTurn(previousCurrent) && stillRunning(previousCurrent)) {
          handOverTheTurn(state, previousCurrent, current, touched);
          current.tookTheTurn = true;
        }
        return;
      }
      if (event.type === 'UserQuestionAsked' && event.tool_use_id) {
        questionDividerOwners.set(event.tool_use_id, current);
      } else if (
        (event.type === 'CodingAgentPermissionRequest'
          || event.type === 'CommandPermissionRequested'
          || event.type === 'McpPermissionRequested')
        && event.request_id
      ) {
        permissionDividerOwners.set(event.request_id, current);
        const gated = gatedCallOf(state, event, previousCurrent);
        if (gated) {
          state.gatedCalls.set(event.request_id, gated);
          (gated.exchange.blockedStepSeqs ??= new Set()).add(gated.seq);
          touched?.add(gated.exchange);
        }
      }
      if (previousCurrent && boundaryTakesTheTurn(event.type, previousCurrent)) {
        handOverTheTurn(state, previousCurrent, current, touched);
      }
      if (
        (event.type === 'UserQuestionAsked'
          || event.type === 'CommandPermissionRequested'
          || event.type === 'McpPermissionRequested')
        && state.lastChatTurnReqId
      ) {
        const priorOwner = reqIdRedirect.get(state.lastChatTurnReqId)
          ?? findExchangeByAnchorId(exchanges, state.lastChatTurnReqId);
        if (priorOwner && priorOwner !== current) {
          priorOwner.continuationMoved = true;
          touched?.add(priorOwner);
        }
        reqIdRedirect.set(state.lastChatTurnReqId, current);
        state.turnDividers.set(state.lastChatTurnReqId, current);
      }
    } else if (event.type === 'CodingAgentUserMessageSent') {
      if (current && current.userEvent.type === 'MessageReceived' && !hasNonChildStep(current)) {
        return;
      }
      const text = (event as { text: string }).text;
      current = { userEvent: { type: 'MessageReceived', text } as StoredEvent, userSeq: seq, steps: [] };
      exchanges.push(current);
      touched?.add(current);
    } else if (event.type === 'CodingAgentPromptSent' && !current) {
      current = { userEvent: event, userSeq: seq, steps: [] };
      exchanges.push(current);
      touched?.add(current);
    } else if (event.type === 'SpokenReplyGenerated' && !callTarget) {
      const greeting: Exchange = { userEvent: event, userSeq: seq, steps: [] };
      exchanges.push(greeting);
      touched?.add(greeting);
      if (!current) current = greeting;
    } else if (CALL_ROW_TYPES.has(event.type)) {
      if (callTarget) {
        const at = callRowIndex(callTarget.steps, happenedAt(event));
        const above = spokenRowToGrow(callTarget.steps, at);
        const grown = above ? grownSpokenRow(above.event, event) : null;
        if (above && grown) {
          above.event = grown;
        } else {
          callTarget.steps.splice(at, 0, { seq, event });
        }
        touched?.add(callTarget);
      }
    } else if (owner) {
      appendStep(owner, seq, event);
      touched?.add(owner);
      if (event.type === 'ToolCalled' && event._eventId) {
        chatToolCallOwners.set(event._eventId, owner);
      }
    } else {
      const next = state.unreadMessages[0];
      if (next && current && !stillRunning(current)
        && !TERMINAL_EVENT_TYPES.has(event.type) && !settlesEndedTurn(event)) {
        state.unreadMessages.shift();
        next.awaitingRead = undefined;
        touched?.add(next);
        current = takeTurnFromQueue(state, next);
      }
      const closed = turnSettledLate(state, current, event);
      if (closed) {
        appendStep(closed, seq, event);
        touched?.add(closed);
        return;
      }
      const runningTurn = chatTurnQueuedBehind(state, current, event);
      const target = runningTurn ?? current ?? openContinuationFragment(state, seq, event, touched);
      if (!target) return;
      if (!runningTurn) current = target;
      appendStep(target, seq, event);
      touched?.add(target);
      if (event.type === 'CodingAgentToolCalled') {
        const id = toolUseIdOf(event);
        if (id) toolCallOwners.set(id, target);
      }
      if (event.type === 'ToolCalled' && event._eventId) {
        chatToolCallOwners.set(event._eventId, target);
      }
    }
  };
  step();
  state.current = current;
  if (event.type === 'MessageReceived' && event.mode === 'agent') pairHeldDelivery(state, event, touched);
}

function pairHeldDelivery(
  state: GroupFoldState,
  event: Extract<StoredEvent, { type: 'MessageReceived' }>,
  touched: Set<Exchange> | null,
): void {
  const release = state.pendingRelease;
  if (!release) return;
  state.pendingRelease = null;
  const held = release.held;
  if (held.text !== event.text || senderKey(held.origin) !== senderKey(event.origin)) return;
  let delivered: Exchange | undefined;
  for (let i = state.exchanges.length - 1; i >= 0 && !delivered; i--) {
    if (state.exchanges[i].userEvent === event) delivered = state.exchanges[i];
  }
  if (!delivered || !held._eventId) return;
  (release.holder.deliveredHeldIds ??= new Set()).add(held._eventId);
  delivered.releasedFromHold = true;
  touched?.add(release.holder);
  touched?.add(delivered);
}

function senderKey(origin: MessageOrigin | undefined): string {
  switch (origin?.kind) {
    case 'thread_link': return `thread:${origin.thread_id}`;
    case 'workspace': return `workspace:${origin.workspace}`;
    default: return origin?.kind ?? '';
  }
}

function openContinuationFragment(
  state: GroupFoldState,
  seq: number,
  event: StoredEvent,
  touched: Set<Exchange> | null,
): Exchange | null {
  if (!state.paged) return null;
  const fragment: Exchange = { userEvent: event, userSeq: seq, steps: [], continuationFragment: true };
  state.exchanges.push(fragment);
  touched?.add(fragment);
  return fragment;
}

export interface HandleEventResult {
  applied: boolean;
  metaChanged: boolean;
  clearedPendingUserMessage: boolean;
  retiredUnsentEventId?: string;
}

export function retireUnsentExchange(thread: ThreadState, eventId: string): boolean {
  const seq = thread.unsentMessageSeqs?.get(eventId);
  if (seq === undefined) return false;
  thread.unsentMessageSeqs!.delete(eventId);
  const events = new Map(thread.events);
  events.delete(seq);
  events.delete(seq + 1);
  thread.events = events;
  return true;
}

export function handleEvent(
  threadMap: Map<string, ThreadState>,
  threadId: string,
  seq: number | null,
  event: ThreadEvent | TransientEvent,
  created?: string,
  eventId?: string,
  aggregate?: ThreadAggregate,
): HandleEventResult {
  const thread = threadMap.get(threadId);
  if (!thread) return { applied: false, metaChanged: false, clearedPendingUserMessage: false };

  let metaChanged = false;
  let clearedPendingUserMessage = false;
  let retiredUnsentEventId: string | undefined;

  if (aggregate) {
    const prevStatus = thread.meta.status;
    if (applyAggregateToThread(thread, aggregate)) metaChanged = true;
    if (thread.meta.status === 'running' && prevStatus !== 'running' && created) {
      thread.meta.lastRevivedAt = created;
      metaChanged = true;
    }
  }

  if (seq !== null) {
    if (thread.events.has(seq)) return { applied: false, metaChanged, clearedPendingUserMessage: false };
    if (!created) {
      console.warn(`[handleEvent] persisted event ${event.type} (seq=${seq}) missing created timestamp — this indicates a backend bug`);
    }
    if (event.type === 'MessageReceived' && seq > 0 && eventId && retireUnsentExchange(thread, eventId)) {
      retiredUnsentEventId = eventId;
    }
    const stored: StoredEvent = { ...(event as ThreadEvent), created, ...(eventId ? { _eventId: eventId } : {}) };
    thread.events.set(seq, stored);
    if (!isSideQuestionEvent(event)) thread.streamingBuffer = '';
    if (isCacheKeepAliveAnchor(stored) && eventId) {
      (thread.keepAliveAnchorIds ??= new Set()).add(eventId);
    }
    if (created && updatesLastActivity(event.type) && !isKeepAliveTurnEvent(thread, stored)) {
      thread.meta.updatedAt = created;
    }
    if (isCallerUtterance(event)) {
      recordCallerUtterance(thread, (event as { text?: string }).text ?? '');
    }
    if (event.type === 'SpokenReplyGenerated') thread.liveReply = undefined;
    if (event.type === 'VoiceSessionEnded') {
      if (thread.liveUtterances?.length) thread.liveUtterances = [];
      thread.liveReply = undefined;
      thread.unclaimedUtterances = [];
    }
    if ((event.type === 'MessageReceived' || event.type === 'PromptInjected' || event.type === 'UserOnlyMessagePosted' || event.type === 'CodingAgentCommandReceived') && thread.pendingUserMessages.length > 0) {
      if (eventId) {
        const idx = thread.pendingUserMessages.findIndex(p => p.eventId === eventId);
        if (idx !== -1) {
          thread.pendingUserMessages.splice(idx, 1);
          clearedPendingUserMessage = true;
        }
      } else {
        thread.pendingUserMessages.shift();
        clearedPendingUserMessage = true;
      }
    }
    if (event.type === 'UserQuestionAnswered' && event.answer.kind === 'FreeText' && thread.pendingUserMessages.length > 0) {
      const text = event.answer.text;
      const idx = thread.pendingUserMessages.findIndex(p => p.text === text);
      if (idx !== -1) {
        thread.pendingUserMessages.splice(idx, 1);
        clearedPendingUserMessage = true;
      }
    }
    if (event.type === 'TodoListWritten') {
      thread.meta.latestTodoList = event.items;
      thread.meta.latestTodoNotes = event.notes ?? null;
      metaChanged = true;
    }
    if (eventWaitProjection(thread.meta, event)) {
      metaChanged = true;
    }
    if (terminalProjection(thread.meta, event, seq)) {
      metaChanged = true;
    }
    if (event.type === 'QueuedMessageRemoved' && thread.pendingUserMessages.length > 0) {
      const before = thread.pendingUserMessages.length;
      thread.pendingUserMessages = thread.pendingUserMessages.filter(
        p => p.eventId !== event.removed_message_id,
      );
      if (thread.pendingUserMessages.length < before) {
        clearedPendingUserMessage = true;
      }
    }
  } else {
    if ('text' in event && typeof event.text === 'string') {
      thread.streamingBuffer = event.text;
    }
    if (created && event.type !== 'ChildrenCountChanged' && event.type !== 'CodingAgentDiffChanged') {
      thread.meta.updatedAt = created;
    }
  }
  return { applied: true, metaChanged, clearedPendingUserMessage, retiredUnsentEventId };
}

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
    return { kind: 'device', device_id: event.device_id };
  }
  return undefined;
}
