import type { SavedScroll } from '../savedScroll';
import type { ThreadStatus } from '../../generated/thread-lifecycle';
import { UNANCHORABLE_ASYNC_EVENTS, computeExchanges, dividerStillAwaitsUser } from './exchange-grouping';
import { isChangeLifecycleEvent, type Exchange } from './exchange';
import type { ThreadState } from './thread-meta';
import type { OngoingGroup } from '../store';
import type { CodingAgentExit } from './thread-event-types';
import type { ProviderError } from '../../generated/thread-event-wire';

export interface ExchangeError {
  message: string;
  agentExit?: CodingAgentExit;
  providerError?: ProviderError;
  spendCap?: { provider: string; resets_at: string | null; message: string };
  eventId?: string;
}

export function exchangeError(exchange: Exchange): ExchangeError | null {
  for (const { event } of exchange.steps) {
    if (event.type === 'ResponseFailed') return { message: event.error, agentExit: event.agent_exit, providerError: event.provider_error, spendCap: event.spend_cap, eventId: event._eventId };
  }
  return null;
}

export function stampedEventIds(exchange: Exchange): string[] {
  const ids: string[] = [];
  const starter = exchangeStarterId(exchange);
  if (starter) ids.push(starter);
  const failure = exchangeError(exchange)?.eventId;
  if (failure) ids.push(failure);
  for (const { event } of exchange.steps) {
    if (event.type === 'WidgetShown' && event._eventId) ids.push(event._eventId);
  }
  return ids;
}

export function exchangeStarterId(exchange: Exchange): string | undefined {
  if (exchange.continuationFragment) return undefined;
  return exchange.userEvent._eventId;
}

export type LandingGroup = Extract<OngoingGroup, 'blocked' | 'review'>;

export type LandingTarget =
  | { kind: 'card'; eventId: string }
  | { kind: 'card-not-loaded'; newestTurnId: string | null }
  | { kind: 'turn'; eventId: string | null };

export function landingTarget(exchanges: Exchange[], status: ThreadStatus, group: LandingGroup, changeReady = false): LandingTarget {
  const newestTurnId = newestStarterId(exchanges);
  if (group === 'review') {
    return { kind: 'turn', eventId: (changeReady ? newestProposalTurnId(exchanges) : null) ?? newestTurnId };
  }
  const awaitsUser = status === 'waiting_for_user_answer';
  const failed = status === 'failed';
  if (!awaitsUser && !failed) return { kind: 'turn', eventId: newestTurnId };
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const ex = exchanges[i];
    const card = awaitsUser
      ? (dividerStillAwaitsUser(ex) ? exchangeStarterId(ex) : undefined)
      : exchangeError(ex)?.eventId;
    if (card) return { kind: 'card', eventId: card };
  }
  return { kind: 'card-not-loaded', newestTurnId };
}

export function readerHasReached(exchanges: Exchange[], target: LandingTarget, saved: SavedScroll | null): boolean {
  if (saved?.kind === 'live-edge') return true;
  if (!saved || saved.kind === 'offset') return false;
  if (target.kind === 'card-not-loaded' || !target.eventId) return false;
  const targetIndex = exchangeIndexHoldingEvent(exchanges, target.eventId);
  const savedIndex = saved.kind === 'row'
    ? exchangeIndexHoldingEvent(exchanges, saved.rowEventId)
    : exchanges.findIndex(ex => exchangeStarterId(ex) === saved.eventId);
  return targetIndex >= 0 && savedIndex >= targetIndex;
}

function newestProposalTurnId(exchanges: Exchange[]): string | null {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const proposed = exchanges[i].steps.some(({ event }) => event.type === 'ChangeProposed' && !!event.change_id);
    const starter = proposed ? exchangeStarterId(exchanges[i]) : undefined;
    if (starter) return starter;
  }
  return null;
}

function newestStarterId(exchanges: Exchange[]): string | null {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const starter = exchangeStarterId(exchanges[i]);
    if (starter) return starter;
  }
  return null;
}

export function deepLinkAnchorForEvent(
  exchanges: Exchange[],
  eventId: string,
): string | null {
  const exchange = exchanges[exchangeIndexHoldingEvent(exchanges, eventId)];
  if (!exchange) return null;
  if (stampedEventIds(exchange).includes(eventId)) return eventId;
  const step = exchange.steps.find(({ event }) => event._eventId === eventId);
  if (step && UNANCHORABLE_ASYNC_EVENTS.has(step.event.type)) return null;
  return exchangeStarterId(exchange) ?? null;
}

function exchangeIndexHoldingEvent(exchanges: readonly Exchange[], eventId: string): number {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const exchange = exchanges[i];
    if (stampedEventIds(exchange).includes(eventId)) return i;
    if (exchange.steps.some(({ event }) => event._eventId === eventId)) return i;
  }
  return -1;
}

export function exchangeChangeId(exchange: Exchange, threadIsCC = true): string | undefined {
  if (isChangeLifecycleEvent(exchange.userEvent)) return exchange.userEvent.change_id;
  if (!threadIsCC) return undefined;
  for (const { event } of exchange.steps) {
    if (event.type === 'ChangeProposed' && event.change_id) return event.change_id;
  }
  return undefined;
}

export type DeepLinkTarget =
  | { kind: 'event'; eventId: string }
  | { kind: 'change'; changeId: string };

export function deepLinkTargetIndex(exchanges: readonly Exchange[], target: DeepLinkTarget): number {
  if (target.kind === 'event') {
    const index = exchangeIndexHoldingEvent(exchanges, target.eventId);
    const holder = exchanges[index];
    if (holder?.continuationFragment && !stampedEventIds(holder).includes(target.eventId)) return -1;
    return index;
  }
  if (!target.changeId) return -1;
  for (let i = exchanges.length - 1; i >= 0; i--) {
    if (exchangeChangeId(exchanges[i]) === target.changeId) return i;
  }
  return -1;
}

export function deepLinkAnchorInThread(thread: ThreadState | undefined, eventId: string): string | null {
  return thread ? deepLinkAnchorForEvent(computeExchanges(thread), eventId) : null;
}
