// The store reads these, so this module ships in the entry chunk. Keep
// transcript render code out of it (ADR 0288).
import { exchangeUserImageHashes, exchangeUserMessage } from './exchange';
import { TERMINAL_EVENT_TYPES, exchangeHoldsNoTurn, isLiveCallRow, isUnsentExchange, isUningestedMessage, isWaitingTypedMessage, latestRunSteps } from './exchange-grouping';
import type { Exchange } from './exchange';

function exchangeHasTerminalStep(exchange: Exchange): boolean {
  // The shared list, plus `SessionEnded`. A coding-agent session ending is a
  // terminal a QUEUE reads, not one the fold reads: it never resolves a turn's
  // continuation, so `TERMINAL_EVENT_TYPES` leaves it out.
  return latestRunSteps(exchange).some(({ event }) =>
    TERMINAL_EVENT_TYPES.has(event.type) || event.type === 'SessionEnded'
  );
}

/** Whether a non-queued exchange is a live or parked turn that can have chat
 *  follow-ups queued behind it. Terminal lifecycle panels deliberately return
 *  false, so a message sent after idle becomes the active turn rather than a
 *  queued bubble. */
function canQueueBehind(exchange: Exchange): boolean {
  if (exchangeHasTerminalStep(exchange)) return false;
  // A boundary that STARTED no turn can still hold one: a caller's utterance
  // takes the running turn's continuation (ADR 0201). A follow-up queues
  // behind that turn, wherever it is now showing.
  if (exchange.tookTheTurn) return true;
  // A *continuation fragment* is a turn whose boundary is off the loaded page,
  // so the switch below cannot recognise it: its `userEvent` is a step. Read as
  // holding no turn, it hands the running turn's stream to the follow-up's own
  // bubble. That message then shows no Queued tag and nothing to retract. The
  // terminal check above still decides a fragment that has finished.
  if (exchange.continuationFragment) return true;
  switch (exchange.userEvent.type) {
    case 'MessageReceived':
    case 'TriggerStarted':
    case 'ContinuationStarted':
    // An engine RE-ENTRY, and a turn like any other. An event wait resolving
    // writes one as its *re-entry anchor*, so a delivery or an expiry renders
    // as the new turn it is. The loop then runs, and a message typed meanwhile
    // waits for the same window.
    //
    // Only an anchor that STARTS an exchange reaches here. The fold absorbs one
    // carrying an `injected_message_id` into the message it names, whose
    // `MessageReceived` answers this above. A rerun pairs its anchor with a
    // `ContinuationStarted`, which starts the exchange and answers above too.
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

/** The bottom exchange that could own a turn, stepping over every live row a
 *  call is drawing, on either side.
 *
 *  Such a row owns no turn whether or not it carries words: no stream, no
 *  `last` role. Every fallback here means "whatever is at the bottom", and the
 *  rows are at the bottom by construction. An unsent message owns no turn either. */
function lastTurnBearingIndex(exchanges: Exchange[]): number {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    if (!isLiveCallRow(exchanges[i].userEvent) && !isUnsentExchange(exchanges[i])) return i;
  }
  return -1;
}

/** A coding-agent thread's queue: the messages the agent has yet to read
 *  (`Exchange.awaitingRead`). The fold keeps the running turn out of it, so
 *  the last exchange that is not waiting holds the turn. */
function codingAgentQueue(exchanges: Exchange[]): QueuedFollowupRun {
  const queuedOrder: number[] = [];
  let activeIndex = -1;
  for (let i = 0; i < exchanges.length; i++) {
    if (exchanges[i].awaitingRead) queuedOrder.push(i);
    else if (!isLiveCallRow(exchanges[i].userEvent) && !isUnsentExchange(exchanges[i])) activeIndex = i;
  }
  if (activeIndex === -1) activeIndex = lastTurnBearingIndex(exchanges);
  return { activeIndex, queuedOrder, queuedIndices: new Set(queuedOrder) };
}

/** Locate the exchange that owns the active response plus queued follow-ups.
 *  A question or permission divider can arrive after a queued MessageReceived
 *  but before injection. Queued indices are therefore tracked independently
 *  rather than as one contiguous trailing run. A coding-agent thread's queue
 *  is its unread messages instead (`codingAgentQueue`). */
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
  if (!threadBusy || exchanges.length === 0) return empty;
  if (threadIsCC) return codingAgentQueue(exchanges);

  // The QUEUE is what the reader may retract, so a delegated utterance is not
  // a candidate: it is speech, and nothing offers to unsay it.
  // Two lists, because they answer different questions. `awaiting` is every
  // message the loop has still to take, and the oldest of those is what the
  // reader is waiting on. `candidates` is the retractable subset.
  const awaiting: number[] = [];
  const candidates: number[] = [];
  for (let i = 0; i < exchanges.length; i++) {
    if (!isUningestedMessage(exchanges[i])) continue;
    awaiting.push(i);
    if (isWaitingTypedMessage(exchanges[i])) candidates.push(i);
  }

  let activeIndex = -1;
  for (let i = exchanges.length - 1; i >= 0; i--) {
    // The WALK asks a wider question, so it takes a wider predicate. What
    // decides the live turn is whether the loop has TAKEN the message, and a
    // delegated utterance awaiting its injection has not been taken. Read as
    // the active one, it steals the running turn's stream and badge.
    if (isUningestedMessage(exchanges[i])) continue;
    if (exchangeHoldsNoTurn(exchanges[i]) || isUnsentExchange(exchanges[i])) continue;
    // Only the MOST RECENT settled or in-flight turn can own queued
    // follow-ups, so stop at the first non-uningested exchange. If it can be
    // queued behind, the follow-up queues behind it. If it is terminal, the
    // thread idled and the just-sent message IS the active turn.
    //
    // Never walk PAST a terminal turn into older non-terminal ones. An
    // answered `UserQuestionAsked` whose continuation flowed into the next
    // question produces no terminal step. The fresh follow-up would then
    // render as "queued" up in history.
    if (canQueueBehind(exchanges[i])) activeIndex = i;
    break;
  }
  // Nothing beneath owns a turn. The reader is then waiting on the oldest
  // message the loop has still to take, which is the next one. `awaiting`
  // rather than `candidates`: a DELEGATED utterance is not retractable and so
  // is not a candidate, but it is exactly what a caller is waiting on. Picked
  // by `candidates` alone it could never be active, and the live stream went to
  // whatever the caller said last instead.
  //
  // With nothing awaiting either, the literal last exchange is active. That is
  // a call with no doer turn under it at all.
  if (activeIndex === -1) {
    activeIndex = awaiting.length > 0 ? awaiting[0] : lastTurnBearingIndex(exchanges);
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

/** A queued (uningested) chat follow-up: its retract id, text and images. */
export interface QueuedMessage {
  /** The client `event_id` (the events-table PK), which is the `message_id`
   *  the `/chat/queued-message/remove` endpoint retracts by. */
  id: string;
  text: string;
  imageHashes: string[];
}

/** The thread's queued (un-injected) chat follow-ups, in FIFO order: the set a
 *  user Stop retracts and returns to compose (see
 *  `store/actions/chat.ts::cancelCurrentExchange`). Derived from the same
 *  `queuedFollowupRun` the UI renders "Queued" bubbles from, so what Stop
 *  clears is exactly what the user saw queued. An exchange with no `_eventId`
 *  cannot be retracted by id and is skipped. */
export function queuedMessagesFromExchanges(
  exchanges: Exchange[],
  threadBusy: boolean,
  threadIsCC = false,
): QueuedMessage[] {
  // A coding agent already holds its queued messages, so nothing can take one
  // back. Only the Lucidos Agent's queue is retractable.
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
