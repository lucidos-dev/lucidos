// The store reads these, so this module ships in the entry chunk. Keep
// transcript render code out of it (ADR 0288).
import type { ThreadStatus } from '../../generated/thread-lifecycle';
import { UNANCHORABLE_ASYNC_EVENTS, computeExchanges, dividerStillAwaitsUser } from './exchange-grouping';
import type { Exchange } from './exchange';
import type { ThreadState } from './thread-meta';
import type { OngoingGroup } from '../store';
import type { CodingAgentExit } from './thread-event-types';

/** The failure card's content: what a `ResponseFailed` step says, and which
 *  event said it. */
export interface ExchangeError {
  /** `ResponseFailed.error`, the text rendered in the card. */
  message: string;
  /** Set when the coding agent's process exited on its own, which swaps the
   *  generic card for one that explains it and offers Try again. */
  agentExit?: CodingAgentExit;
  /** The `ResponseFailed`'s OWN event id. `ChatExchange` stamps it on the card
   *  as `data-event-id`, which is what resolves a notification deep-link to a
   *  failure (`scrollToEventAndPulse`) and pulses the card itself.
   *  `ResponseFailed` is a step, not an exchange starter, so the exchange
   *  root's `data-event-id` carries a different event. Absent on legacy rows
   *  and on the client-synthesized transport-failure event (`actions/chat.ts`),
   *  neither of which any notification points at. */
  eventId?: string;
}

/** The error a failed exchange carries, or null when it didn't fail. */
export function exchangeError(exchange: Exchange): ExchangeError | null {
  for (const { event } of exchange.steps) {
    if (event.type === 'ResponseFailed') return { message: event.error, agentExit: event.agent_exit, eventId: event._eventId };
  }
  return null;
}

/** Every event id this exchange puts into the DOM as `data-event-id`: the
 *  complete set a deep-link can resolve against inside this turn.
 *
 *  Three kinds:
 *  - the turn root, in `ChatExchange`, carries the turn's STARTER;
 *  - the failure card, in `ChatExchange`, carries its own `ResponseFailed`
 *    (see `ExchangeError.eventId` above);
 *  - each old `WidgetShown`'s inline widget (`InlineWidget`) carries its own
 *    id, so "Show in thread" lands on it and not on the turn's start. A
 *    reply's inline widget wears nothing: its pin names the turn's starter.
 *
 *  Every other step is deliberately unstamped, inline steps most of all: the
 *  "Show steps" toggle can hide them, and an id there would resolve only
 *  sometimes. An old showing is a marker, drawn whatever the toggle says.
 *
 *  This is the one definition of the stamping rule, and
 *  `deepLinkAnchorForEvent` reads it. A source-scan tripwire in
 *  `__tests__/deep-link-anchor.test.ts` fails on any stamp not declared here. */
export function stampedEventIds(exchange: Exchange): string[] {
  const ids: string[] = [];
  // Every stamp is conditional in its component, since Preact drops an
  // `undefined` attribute. An id the DOM will not carry is not listed here.
  const starter = exchangeStarterId(exchange);
  if (starter) ids.push(starter);
  const failure = exchangeError(exchange)?.eventId;
  if (failure) ids.push(failure);
  for (const { event } of exchange.steps) {
    if (event.type === 'WidgetShown' && event._eventId) ids.push(event._eventId);
  }
  return ids;
}

/** The id the turn root wears, or undefined where the turn has no starter.
 *
 *  A *continuation fragment* is the undefined case. Its boundary is older than
 *  the loaded page. Its `userEvent` is only its own first step, which the rule
 *  above says a step never wears. Stamping it would make one row addressable
 *  out of a turn's many, and would collide with the failure card when that step
 *  IS the failure.
 *
 *  Read by `ChatExchange`'s root and by `stampedEventIds`, so the attribute and
 *  the claim about it cannot drift. */
export function exchangeStarterId(exchange: Exchange): string | undefined {
  if (exchange.continuationFragment) return undefined;
  return exchange.userEvent._eventId;
}

/** The Ongoing groups whose rows open on what needs the user rather than on
 *  the saved reading position (ADR 0409). */
export type LandingGroup = Extract<OngoingGroup, 'blocked' | 'review'>;

/** Where opening a thread from Blocked or Review lands.
 *
 *  - `card`: the thing waiting on the user, as a `data-event-id`.
 *  - `card-not-loaded`: the status names a card, and the loaded exchanges do
 *    not hold it. It may be older than the loaded page.
 *  - `turn`: no card applies, so a turn's start. A stopped child lands on the
 *    turn that stopped, and an open form sits inside that turn. */
export type LandingTarget =
  | { kind: 'card'; eventId: string }
  | { kind: 'card-not-loaded'; newestTurnId: string | null }
  | { kind: 'turn'; eventId: string | null };

/** Pick the {@link LandingTarget} for a thread opened from `group`. Newest
 *  first, so the live request wins. `changeReady` says the thread holds a
 *  ready change, which only Review reads.
 *
 *  From Blocked:
 *  - Waiting on the user: the open question or permission card.
 *  - Failed: the failure card.
 *
 *  Each card is gated on its own status, because an older canceled question
 *  or a failure the user moved past stays in the history.
 *
 *  From Review: the turn that proposed the ready change, else the newest turn,
 *  whose reply a read request points at. A change applied or discarded
 *  earlier still sits in its turn, so only a ready one counts. */
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

/** The start of the newest turn that proposed a change, or null. A per-commit
 *  `ChangeProposed` carries no `change_id`, so only the turn's aggregate
 *  proposal counts. */
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

/** The `data-event-id` a deep-link to `eventId` should target within
 *  `exchanges`, or `null` when there is nowhere honest to land.
 *
 *  Three answers, in order:
 *
 *   - An event that stamps its own element (`stampedEventIds`) is its own
 *     target, so the pulse stays on the thing the user was sent to see.
 *   - An event the turn PRODUCED but that renders no addressable element
 *     targets the turn containing it. That is the difference between a link
 *     that works and one that spends the 4s resolve deadline.
 *   - An event that merely arrived inside an open turn
 *     (`UNANCHORABLE_ASYNC_EVENTS`) has no anchor, and says so.
 *
 *  `null` is a real answer rather than a failure. Callers read it as "nowhere
 *  to go". The delivery and trigger rows then render no jump affordance rather
 *  than a tap that pulses something unrelated.
 *
 *  Every event deep link goes through it. The engine only checks that a
 *  notification's event is in its thread, so a mid-turn `CredentialRequested`
 *  is a step like any other. */
export function deepLinkAnchorForEvent(
  exchanges: Exchange[],
  eventId: string,
): string | null {
  // Backward walk, matching `findExchangeByAnchorId`: on the vanishingly rare
  // id collision the most recent owner is the one on screen.
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const exchange = exchanges[i];
    if (stampedEventIds(exchange).includes(eventId)) return eventId;
    const step = exchange.steps.find(({ event }) => event._eventId === eventId);
    if (step) {
      if (UNANCHORABLE_ASYNC_EVENTS.has(step.event.type)) return null;
      // A turn whose own starter is unstamped gives the deep-link nothing to
      // aim at. Saying so beats an `undefined` that would read as "not in this
      // thread" further down. Two ways to be unstamped: a legacy row carrying
      // no id, and a turn with no starter loaded at all.
      return exchangeStarterId(exchange) ?? null;
    }
  }
  return null;
}

/** {@link deepLinkAnchorForEvent} over a thread's own exchanges, and null for a
 *  thread that is not here. `computeExchanges` is memoized per thread, so a
 *  repeat call is a lookup rather than a regrouping. */
export function deepLinkAnchorInThread(thread: ThreadState | undefined, eventId: string): string | null {
  return thread ? deepLinkAnchorForEvent(computeExchanges(thread), eventId) : null;
}
