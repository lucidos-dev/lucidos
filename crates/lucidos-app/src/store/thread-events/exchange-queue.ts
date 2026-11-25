import { exchangeUserImageHashes, exchangeUserMessage } from './exchange';
import { TERMINAL_EVENT_TYPES, exchangeHoldsNoTurn, isLiveCallRow, isUnsentExchange, isUningestedMessage, isWaitingTypedMessage, latestRunSteps } from './exchange-grouping';
import type { Exchange } from './exchange';
import { isTurnlessBoundary } from './thread-event-types';

function exchangeHasTerminalStep(exchange: Exchange): boolean {
  return latestRunSteps(exchange).some(({ event }) =>
    TERMINAL_EVENT_TYPES.has(event.type) || event.type === 'SessionEnded'
  );
}

function canQueueBehind(exchange: Exchange): boolean {
  if (exchangeHasTerminalStep(exchange)) return false;
  if (exchange.tookTheTurn) return true;
  if (exchange.continuationFragment) return true;
  switch (exchange.userEvent.type) {
    case 'MessageReceived':
    case 'TriggerStarted':
    case 'ContinuationStarted':
    case 'PromptInjected':
    case 'UserQuestionAsked':
    case 'CodingAgentPermissionRequest':
    case 'CommandPermissionRequested':
    case 'McpPermissionRequested':
    case 'McpConsentRequested':
    case 'ChildThreadCompleted':
    case 'MissingHardeningDetected':
    case 'MergeConflictDetected':
      return true;
    default:
      return false;
  }
}

export interface QueuedFollowupRun {
  activeIndex: number;
  queuedOrder: number[];
  queuedIndices: Set<number>;
}

function bearsTurn(exchange: Exchange): boolean {
  return !isLiveCallRow(exchange.userEvent) && !isUnsentExchange(exchange) && !isTurnlessBoundary(exchange.userEvent);
}

function lastTurnBearingIndex(exchanges: Exchange[]): number {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    if (bearsTurn(exchanges[i])) return i;
  }
  return -1;
}

function codingAgentQueue(exchanges: Exchange[]): QueuedFollowupRun {
  const queuedOrder: number[] = [];
  let activeIndex = -1;
  for (let i = 0; i < exchanges.length; i++) {
    if (exchanges[i].awaitingRead) queuedOrder.push(i);
    else if (bearsTurn(exchanges[i])) activeIndex = i;
  }
  if (activeIndex === -1) activeIndex = lastTurnBearingIndex(exchanges);
  return { activeIndex, queuedOrder, queuedIndices: new Set(queuedOrder) };
}

export function queuedFollowupRun(
  exchanges: Exchange[],
  threadBusy: boolean,
  threadIsCC = false,
): QueuedFollowupRun {
  const empty: QueuedFollowupRun = {
    activeIndex: lastTurnBearingIndex(exchanges),
    queuedOrder: [],
    queuedIndices: new Set(),
  };
  const held = (ex: Exchange) => ex.userEvent.queued_for_permission
    && (threadIsCC ? ex.awaitingRead : isUningestedMessage(ex));
  if (!threadBusy && !exchanges.some(held)) return empty;
  if (threadIsCC) return codingAgentQueue(exchanges);

  const awaiting: number[] = [];
  const candidates: number[] = [];
  for (let i = 0; i < exchanges.length; i++) {
    if (!isUningestedMessage(exchanges[i])) continue;
    awaiting.push(i);
    if (isWaitingTypedMessage(exchanges[i])) candidates.push(i);
  }

  let activeIndex = -1;
  for (let i = exchanges.length - 1; i >= 0; i--) {
    if (isUningestedMessage(exchanges[i])) continue;
    if (exchangeHoldsNoTurn(exchanges[i]) || isUnsentExchange(exchanges[i]) || isTurnlessBoundary(exchanges[i].userEvent)) continue;
    if (canQueueBehind(exchanges[i])) activeIndex = i;
    break;
  }
  if (activeIndex === -1) {
    activeIndex = awaiting.find(i => !held(exchanges[i]))
      ?? (awaiting.length === 0 ? lastTurnBearingIndex(exchanges) : -1);
  }

  const queuedOrder = candidates.filter(i => i !== activeIndex);
  if (queuedOrder.length === 0) {
    return { ...empty, activeIndex };
  }

  const queuedIndices = new Set<number>(queuedOrder);

  return {
    activeIndex,
    queuedOrder,
    queuedIndices,
  };
}

export interface QueuedMessage {
  id: string;
  text: string;
  imageHashes: string[];
}

export function queuedMessagesFromExchanges(
  exchanges: Exchange[],
  threadBusy: boolean,
  threadIsCC = false,
): QueuedMessage[] {
  if (threadIsCC) return [];
  const { queuedOrder } = queuedFollowupRun(exchanges, threadBusy, threadIsCC);
  const out: QueuedMessage[] = [];
  for (const idx of queuedOrder) {
    const id = exchanges[idx].userEvent._eventId;
    if (!id) continue;
    out.push({ id, text: exchangeUserMessage(exchanges[idx]), imageHashes: exchangeUserImageHashes(exchanges[idx]) });
  }
  return out;
}
