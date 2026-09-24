import { EVENT_CLASSIFICATION } from '../../generated/thread-lifecycle';
import { instantMicros } from '../../utils/isoInstant';
import { eventWaitProjection } from './event-waits';
import { findQuestionAnswer, modeToInitiator } from './exchange';
import { isOneUtterance, joinSpoken } from './spokenMerge';
import { isTurnlessBoundary, isUserStoppedWait } from './thread-event-types';
import { applyAggregateToMeta, updatesLastActivity } from './thread-meta';
import type { Exchange } from './exchange';
import type { MessageOrigin, SequencedEvent, StoredEvent, ThreadEvent, TransientEvent } from './thread-event-types';
import type { ThreadAggregate, ThreadState } from './thread-meta';

// ---------------------------------------------------------------------------
// Exchange grouping
// ---------------------------------------------------------------------------

/** Compute exchanges for a thread, merging any pending user messages as
 *  synthetic MessageReceived events. No signal dependencies. Memoized per
 *  thread via `groupIntoExchangesCached` — a streaming token extends the
 *  fold instead of re-sorting and re-walking the whole event history every
 *  frame. A synthetic seq never enters the cache. The fast path appends to the
 *  cached fold; the fallback folds an augmented COPY of the events map. */
export function computeExchanges(thread: ThreadState): Exchange[] {
  return withLiveCallRows(thread, foldedExchanges(thread));
}

/**
 * Draw the call under everything, both sides of it, as it happens.
 *
 * Appended HERE, past every path through the fold, and that is the whole of its
 * safety. The fold never sees them. So one cannot be re-anchored, cannot become
 * a wall a re-anchor stops at, and cannot take a step off a running turn.
 *
 * A caller's row carries `partial` while they speak and `text` from the instant
 * the provider ends the turn. The talker's row carries what it has said so far.
 * Each is retired by the engine's own row for it (ADR 0174).
 *
 * **Ordered by `created` across both.** One clock stamps them, and a caller
 * cutting in mid-reply must read BELOW the reply they cut into. Sorting is what
 * keeps an answer from sitting above the question.
 *
 * They take the very top of the synthetic seq range, above every pending typed
 * message, which is right: the call is happening now, and those were sent
 * before. `syntheticSeqBase` is what keeps the two blocks from overlapping.
 */
function withLiveCallRows(thread: ThreadState, exchanges: Exchange[]): Exchange[] {
  const live = liveCallEvents(thread);
  if (live.length === 0) return exchanges;
  const out = [...exchanges];
  let seq = Number.MAX_SAFE_INTEGER - (live.length - 1);
  for (const userEvent of live) {
    // The talker's row goes INSIDE the block its persisted row will land in,
    // which is the one `callRowTarget` picks. Two things follow, and both were
    // reported. It draws no Lucidos Agent header of its own, so none appears
    // between two speech bubbles. And the swap to the engine's row moves
    // nothing, because both sit in the same place.
    //
    // A reply with nowhere to land still opens a boundary, which is the
    // greeting: the talker spoke before anybody else did.
    const at = isLiveReplyRow(userEvent) ? liveReplyTargetIndex(out) : -1;
    if (at === -1) {
      out.push({ userEvent, userSeq: seq, steps: [] });
    } else {
      // Filed where it was SAID, by the rule the persisted row takes, so the
      // swap moves nothing.
      //
      // Its moment is the browser's own clock, and every step it is ordered
      // against carries the database's. That skew is the one this cannot
      // close, and it is not bounded by a step: a device minutes behind files
      // the bubble minutes too early, until the engine's row corrects it.
      //
      // Cloned, never mutated: the fold's result is memoized, and splicing
      // into its array would make the live row permanent.
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

/**
 * When a row HAPPENED, which for speech is when the words began.
 *
 * `created` is when a row was written. For a step that is the same instant,
 * and for a spoken reply it is when the talker STOPPED. Those words covered a
 * stretch of time and the reader heard them start. So the row carries how long
 * it had been speaking, and this subtracts it (ADR 0206).
 *
 * A LIVE row has no `created` at all, nothing having written it down. Its
 * `_displayCreated` is the moment the bridge drew it, which answers the same
 * question on the browser's clock.
 *
 * `null` for a row with no timestamp to read, which no caller may order.
 *
 * Exported for `render-order.ts`, which checks the fold's output against the
 * very rule the fold placed those rows by. Two readings would drift, and the
 * drift is a check that passes over a transcript the reader sees out of order.
 */
export function happenedAt(event: StoredEvent): number | null {
  const landed = instantMicros(event.created ?? event._displayCreated);
  const secs = event.type === 'SpokenReplyGenerated' ? event.spoken_secs_before : undefined;
  if (landed === null || secs === undefined) return landed;
  return landed - secs * 1_000_000;
}

/**
 * Where a call row belongs among an exchange's steps.
 *
 * The row goes to the BOTTOM exchange rather than to `current`, so that
 * exchange can already hold steps stamped after the words (ADR 0201). This
 * walks to the first step that followed them.
 *
 * **Every step is read by `happenedAt` too**, the rule the row itself is
 * placed by. A spoken row already filed is then not a wall at its own write
 * time.
 *
 * The END for a row with no stamp, and a step with none is stepped over rather
 * than compared. A session mark was never said, and a legacy row cannot say
 * when it was, so both keep the placement they have always had.
 *
 * **Never INSIDE a run of streamed text**, which the renderer merges only
 * while adjacent: a row dropped mid-run cuts one answer in two.
 */
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

/** The spoken row a new one continues, or undefined. `at` is where the new row
 *  would otherwise land.
 *
 *  Normally the row directly above it. Anything between two fragments means
 *  the reader met something in between, so they are two things said.
 *
 *  **Unless it landed while the first one was still being said.** A spoken row
 *  covers a stretch of time (ADR 0206), so a step stamped inside it separated
 *  nothing: the talker never stopped. Splitting there draws one sentence as
 *  two bubbles around a step. It also splits the live bubble in two as the
 *  rows land, which is the jump this all exists to end.
 *
 *  The walk stops at the first spoken row either way, and only a call row pays
 *  for it. */
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

/** Back out to the start of a run of streamed text, when the row would land
 *  INSIDE one. `rightOf` is what follows it there.
 *
 *  **Both sides have to be text.** A row that merely FOLLOWS a finished run is
 *  not inside it. Lifting it there puts a note above the whole answer it came
 *  after, where the live row that drew it sat below.
 *
 *  One walk, two callers: a row filed by the clock and a row lifted back out
 *  of a run the next delta extended. Two copies would let the same three
 *  events land in different orders depending only on arrival timing. */
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

/** A step the renderer folds into the prose beside it, so two of them in a row
 *  are one document rather than two. */
function isStreamedText(event: { type: string }): boolean {
  return event.type === 'TextStreamed' || event.type === 'CodingAgentTextStreamed';
}

/** Put one step at the end, keeping a run of streamed text whole.
 *
 *  A spoken row said mid-answer lands at the end of the run so far, because
 *  nothing yet says the run continues. The next delta is what says so, and
 *  this is where it lands: the row is lifted to the run's start rather than
 *  left cutting one answer into two markdown documents.
 *
 *  Only a CALL row is ever lifted. Every other step belongs to the same turn
 *  as the text around it, and its position says something about the work. */
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
    // `event` is the run's next delta, which is what says the run continues.
    steps.splice(runStartBefore(steps, steps.length, event), 0, lifted);
  }
  steps.push({ seq, event });
}

/** The call's live rows as synthetic events, oldest first.
 *
 *  Each wears the type its persisted counterpart wears, so every reader
 *  downstream already knows how to draw it and what it means. */
function liveCallEvents(thread: ThreadState): StoredEvent[] {
  const rows: { created: string; event: StoredEvent }[] = [];
  for (const row of thread.liveUtterances ?? []) {
    // The partial rides `text`, so every reader that draws a user bubble draws
    // one here too. `_livePartial` is what stops them reading it as finished:
    // the provider has not ended the turn, so nothing is in flight behind it.
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
        // No frame carries the session id, so the client never learns one.
        // Empty rather than invented, and nothing reads it off this type.
        session_id: '',
        _eventId: reply.eventId,
        _displayCreated: reply.created,
        _liveReply: true as const,
      } as StoredEvent,
    });
  }
  // Stable within one timestamp, which keeps a caller's own two rows in the
  // order they were drawn when a reply lands in the same millisecond.
  return rows
    .map((row, i) => ({ ...row, i }))
    .sort((a, b) => (a.created === b.created ? a.i - b.i : a.created < b.created ? -1 : 1))
    .map(row => row.event);
}

/** How many live rows the call is drawing, both sides counted. */
function liveCallRowCount(thread: ThreadState): number {
  return (thread.liveUtterances?.length ?? 0) + (thread.liveReply ? 1 : 0);
}

/** The marks a synthetic live row wears. A persisted event carries none. */
type LiveRowMarks = { _liveUtterance?: true; _liveReply?: true; _livePartial?: true };

/** True for the caller's synthetic row, and for nothing the engine ever wrote. */
export function isLiveUtteranceRow(event: LiveRowMarks): boolean {
  return event._liveUtterance === true;
}

/** True for the talker's synthetic row, same promise. */
export function isLiveReplyRow(event: LiveRowMarks): boolean {
  return event._liveReply === true;
}

/** True while the caller's row shows a PARTIAL rather than their finished
 *  words. They are still speaking, so nothing is in flight behind it. */
export function isLivePartialRow(event: LiveRowMarks): boolean {
  return event._livePartial === true;
}

/** True for a live caller row carrying the provider's FINAL words.
 *
 *  Three shapes wear `_liveUtterance` and this is the last of them. The pulse
 *  has no words at all, a partial has words the provider may still revise, and
 *  this one has the sentence. Only here has the caller finished. */
export function isSettledLiveUtterance(event: LiveRowMarks & { text?: string }): boolean {
  return isLiveUtteranceRow(event) && !isLivePartialRow(event) && !!event.text;
}

/** True for either side's live row. What they share is that no event carries
 *  them, so nothing downstream may treat one as a persisted boundary. */
export function isLiveCallRow(event: LiveRowMarks): boolean {
  return isLiveUtteranceRow(event) || isLiveReplyRow(event);
}

/** Where the PENDING typed messages' synthetic seqs start.
 *
 *  Two synthetic blocks share the top of the range, and a `userSeq` is an
 *  identity: `exchangeKey` falls back to it, the collapse stores key on it, and
 *  the transcript stamps it as `data-user-seq`. Both blocks counting down from
 *  `MAX_SAFE_INTEGER` would collide the moment a reader typed during a call
 *  with two utterances still un-landed. The caller's rows take the top, so the
 *  typed ones start below them. */
function syntheticSeqBase(thread: ThreadState): number {
  return Number.MAX_SAFE_INTEGER - liveCallRowCount(thread);
}

function foldedExchanges(thread: ThreadState): Exchange[] {
  // The thread's own watermark, not a property of the events it holds. A page
  // starting mid-turn and a corrupt thread look identical to the fold, and
  // only this tells them apart. See `Exchange.continuationFragment`.
  const paged = thread.hasOlderEvents === true;
  if (thread.pendingUserMessages.length === 0) {
    return filterRemovedQueuedExchanges(groupIntoExchangesCached(thread.events, paged), thread.events);
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
  const isCC = thread.meta.channel === 'claude_code';
  const pendingCount = thread.pendingUserMessages.length;
  const synthetic: SequencedEvent[] = [];
  for (let i = 0; i < pendingCount; i++) {
    const pending = thread.pendingUserMessages[i];
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

  // Fast path. When every pending message sorts after the last folded real
  // event, the augmented fold is the cached fold plus one trailing exchange per
  // pending message. So append, rather than re-fold the whole history on every
  // send and every streamed token. Checking only the earliest pending suffices:
  // later ones have strictly larger seqs and same-or-later timestamps.
  const base = groupIntoExchangesCached(thread.events, paged);
  const cache = incrementalCache.get(thread.events);
  const first = synthetic[0];
  const canAppendTrailing =
    !!cache &&
    cache.cacheable &&
    compareSortKeys(instantMicros(first.event.created), first.seq, cache.lastCreatedMicros, cache.lastSeq) >= 0;

  if (canAppendTrailing) {
    const exchanges = [...base];
    for (const { seq, event } of synthetic) {
      exchanges.push({ userEvent: event, userSeq: seq, steps: [] });
    }
    return filterRemovedQueuedExchanges(exchanges, thread.events);
  }

  // Fallback (CC clock-skew / a real event landing before the MessageReceived
  // echo cleared the pending; or a non-cacheable legacy map): the augmented full
  // re-fold — literally the prior behavior, so equivalence holds either way.
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

/** Drop the exchanges of messages the reader retracted.
 *
 *  The test is `isWaitingTypedMessage`, the same one that OFFERED the retract
 *  (`queuedMessagesFromExchanges`). Steplessness is a different question. A
 *  message a delegation marker had folded into would be offered and then
 *  survive the removal, rendering as a turn with a red Aborted badge. */
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

/** Event types that begin a new exchange in the timeline. Each agent pause is
 *  its own auditable boundary with an actor, never a step inside the prior
 *  agent response.
 *
 *  One boundary is decided by the EVENT rather than by its type and so is not
 *  in this set: see `isExchangeStartEvent`. */
export const EXCHANGE_START_TYPES: ReadonlySet<string> = new Set([
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
  'CommandPermissionRequested',
  'McpPermissionRequested',
  'CredentialRequested',
  'McpConsentRequested',
  'ChildThreadCompleted',
  // A user Stop paused a child (ADR 0252). A note that holds no turn: see
  // `isTurnlessBoundary`.
  'ChildThreadStopped',
  // A caller's utterance the talker answered alone. A reader sorts a
  // transcript by who is talking. Which turn was running underneath comes
  // second, so the caller opens a boundary either way.
  'SpokenMessageReceived',
]);

/** Whether this event opens a new exchange. Takes the EVENT, not its type,
 *  because one boundary cannot be decided from the type alone.
 *
 *  `EventWaitCanceled` is a step for every cause but one. A user stop is the one
 *  resolution with no wake, so nothing else in the transcript reports it. Left
 *  as a step it lands inside whatever turn is current, or rewrites the arming
 *  row far above where the user is reading. As a boundary it reads like the Stop
 *  and Restart turns it belongs with. Nothing resumes out of it, which is why
 *  the park itself is still never a boundary. */
export function isExchangeStartEvent(event: { type: string; cause?: string }): boolean {
  if (event.type === 'EventWaitCanceled') return isUserStoppedWait(event);
  return EXCHANGE_START_TYPES.has(event.type);
}

/** What a boundary does with a turn that is STILL RUNNING under it. See the
 *  glossary's § Continuation handoff for the concept.
 *
 *  - `takes`: the turn's continuation moves here, so its remaining rows read
 *    below this card. That is where they happened.
 *  - `takes-unless-parked`: the same, unless the card above is a divider still
 *    awaiting the user. Its answer belongs to that card.
 *  - `leaves`: the turn keeps its owner, and the reason is on the entry.
 *  - `ends`: the boundary terminates the turn, and returns before this is read.
 *  - `own-arm`: an arm higher up in `foldEvent` decides it. */
export type ContinuationHandoff = 'takes' | 'takes-unless-parked' | 'leaves' | 'ends' | 'own-arm';

/** The handoff decision for every boundary type, with no gaps.
 *
 *  **A missing entry is what the reported bug WAS.** `ContinuationStarted` was
 *  added to `EXCHANGE_START_TYPES` and to nothing else, so the resumed turn
 *  kept writing into the card above the resume. An allow-list cannot say
 *  whether a type was considered and declined, or simply forgotten. A table
 *  can, and `render-order.test.ts` fails if a type reaches here undecided.
 *
 *  `EventWaitCanceled` is in the table and not in the set: a user stop is
 *  decided by its cause (`isExchangeStartEvent`). */
export const BOUNDARY_CONTINUATION_HANDOFF: ReadonlyMap<string, ContinuationHandoff> = new Map<string, ContinuationHandoff>([
  // A queued message. The loop has not picked it up, so the running turn
  // keeps writing above it until a `UserPromptInjected` absorbs it.
  ['MessageReceived', 'leaves'],
  // Opens the thread's own first turn, so there is none to take.
  ['TriggerStarted', 'leaves'],
  ['ResponseAborted', 'ends'],
  ['ResponseCanceled', 'ends'],
  // The resume owns what it resumed. The turn can be anchored elsewhere: a
  // pending event wait wakes the thread first and the turn anchors on that
  // prompt. Leaving it stranded the work above the resume card.
  ['ContinuationStarted', 'takes-unless-parked'],
  // A wake from a detached wait, injected as a ReentryFromWait, which holds no
  // turn of its own (ADR 0049).
  ['UserPromptInjected', 'takes-unless-parked'],
  // A sub-thread finishing mid response. The engine injects the summary as a
  // ReentryFromEngine, minting no id, so only the redirect can find the card.
  ['ChildThreadCompleted', 'takes-unless-parked'],
  // Chat prompts the loop raises in-process. The turn resumes under its own
  // unchanged req_id, so the answer must read below the card that asked.
  ['UserQuestionAsked', 'takes'],
  ['CommandPermissionRequested', 'takes'],
  ['McpPermissionRequested', 'takes'],
  // No producer emits either one today. Decided with their siblings above, so
  // the family behaves alike the day one does.
  ['CredentialRequested', 'takes'],
  ['McpConsentRequested', 'takes'],
  // Coding-agent events fold by the clock rather than by request id, so the
  // continuation already lands below these. A handoff would move nothing.
  ['CodingAgentPermissionRequest', 'leaves'],
  ['MissingHardeningDetected', 'leaves'],
  ['MergeConflictDetected', 'leaves'],
  ['ChangeApplied', 'leaves'],
  ['ChangeDiscarded', 'leaves'],
  ['ChangeReverted', 'leaves'],
  ['ChangeApplyFailed', 'leaves'],
  // All three are settled before the decision below is read.
  ['SpokenMessageReceived', 'own-arm'],
  ['EventWaitCanceled', 'own-arm'],
  ['ChildThreadStopped', 'own-arm'],
]);

/** Does this boundary take the running turn? `previous` held it.
 *
 *  An unlisted type falls back to `leaves`, which is what every boundary did
 *  before the table existed. The mirror test keeps that fallback unreachable. */
function boundaryTakesTheTurn(type: string, previous: Exchange): boolean {
  const rule = BOUNDARY_CONTINUATION_HANDOFF.get(type);
  if (rule === 'takes') return true;
  if (rule === 'takes-unless-parked') return !dividerStillAwaitsUser(previous);
  return false;
}

/** True when `exchange` is a divider still PARKED awaiting a user action: its
 *  resolution (answer / grant) has not landed as a step yet.
 *
 *  A parked divider owns its own post-resolution continuation, so a
 *  `ChildThreadCompleted` landing while it waits must NOT steal the request-id
 *  redirect away. Ordering is handled the other way round: when the resolution
 *  lands, `reanchorResolvedDivider` moves the DIVIDER below that boundary.
 *
 *  The check is on the divider's STATE, not just its type. Once resolved, the
 *  turn is an ordinary in-flight response again, and a child completion must
 *  advance the redirect like any other.
 *
 *  `CredentialRequested` / `McpConsentRequested` have no resolution event in the
 *  ThreadEvent union, so they can never be observed as resolved and stay parked.
 *  Add the resolution arm here if one is ever introduced. */
function dividerStillAwaitsUser(exchange: Exchange): boolean {
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
    case 'CredentialRequested':
    case 'McpConsentRequested':
      return true;
    default:
      return false;
  }
}

/** Pure bookkeeping metadata events that belong to no exchange. Without this
 *  filter, such an event leaks into the new, still-empty exchange a boundary
 *  just started, via the `current.steps.push` fallthrough. That breaks the
 *  single-step shape `exchangeStatus` short-circuits on. It also flips a
 *  trailing CC child-completion row to a phantom 'coding-agent-working' that
 *  survives reloads, grouping being deterministic from the event history.
 *
 *  Two sources, unioned. Every `Thread*` metadata event, derived from
 *  EVENT_CLASSIFICATION so a new one added in Rust needs no edit here.
 *  ThreadArchived is excluded automatically, the contract classifying it
 *  terminal. Plus the non-`Thread`-prefixed bookkeeping events that render
 *  nothing and must never count as a step.
 *
 *  **The bar for the explicit list is that NOTHING which reads an exchange's
 *  steps may depend on the event.** Membership drops it out of `steps`
 *  entirely. `CodingAgentSettingsChanged` stays OUT for exactly that reason: it
 *  draws nothing, but `extractResponseField` (exchange.ts) reads it out of the
 *  steps for the model and effort the response header reports. Whether
 *  `BackgroundBash*` belong here is open; `thread-flows-event-wait.test.ts`
 *  pins `BackgroundBashStarted` as a step of the turn that spawned it. */
const NON_EXCHANGE_METADATA_EVENTS: ReadonlySet<string> = new Set([
  ...Object.entries(EVENT_CLASSIFICATION)
    .filter(([evt, cls]) => cls === 'metadata' && evt.startsWith('Thread'))
    .map(([evt]) => evt),
  'QueuedMessageRemoved',
  // Background worktree-cleanup bookkeeping: EventClass::Metadata in
  // worktree_cleanup.rs, but not `Thread`-prefixed and with no render case.
  'WorktreeCleaned',
]);

/** True for a `ContextCaptured` recording an *auxiliary model call* rather
 *  than an agent's turn: a thread title, an image description, a memory call,
 *  an image generation.
 *
 *  These belong to no exchange and are dropped from the fold. A capture binds
 *  to the step it follows (`bindSnapshotToStep`). A memory classification
 *  landing mid-turn would therefore replace that step's context chip with the
 *  classifier's own few hundred tokens. The rows stay in the event log, which
 *  is where token accounting reads them.
 *
 *  Absent `purpose` means `turn`, so every row written before the field
 *  existed reads as one. */
export function isAuxiliaryCapture(event: { type: string }): boolean {
  if (event.type !== 'ContextCaptured') return false;
  const purpose = (event as { purpose?: string }).purpose;
  return purpose !== undefined && purpose !== 'turn';
}

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
 *  1. Engine resume note. A UPI emitted by chat/rerun.rs right after
 *     ContinuationStarted belongs as a step under the resume initiator. A
 *     Human-mode UPI in the same position is a real correction and stays its
 *     own exchange.
 *  2. Mid-flight injection. The chat fast path emits MessageReceived first with
 *     the client UUID, then sends the injection. The agentic loop later emits a
 *     UPI carrying that UUID in `injected_message_id`.
 *
 *  Returns null when the event is not absorbable, or when an injection's partner
 *  is missing. The caller then starts a new exchange, so the UPI still renders
 *  rather than vanishing. */
function findAbsorbTarget(
  state: GroupFoldState,
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
    const own = exchanges.find(ex =>
      ex.userEvent.type === 'MessageReceived' && ex.userEvent._eventId === event.injected_message_id,
    );
    // The message's OWN card first, always. The absorb RE-ANCHORS that card to
    // where the loop picked the message up. A redirect pointing at wherever
    // the turn later moved would absorb into the wrong one.
    //
    // The fallback is for a CALL, whose turn anchors on the talker's
    // `WorkDelegated` (ADR 0201). That is a step rather than a boundary, so no
    // exchange wears the id and only the fold's redirect can find it.
    return own ?? state.reqIdRedirect.get(event.injected_message_id) ?? null;
  }
  return null;
}

/** Chat-loop events whose `request_event_id` should route them to their
 *  originating exchange. `CodingAgent*` is excluded, since CC reuses one
 *  session across many follow-ups and never re-anchors the field. Routing by
 *  request id would push every follow-up's work back into the first MR.
 *
 *  Response* events are dual-purpose, chat and CC both emitting them. For CC
 *  they carry the session's persistent req_id, so `shouldRouteByRequestId`
 *  filters them out when the channel is CC.
 *
 *  **Every event the chat agentic loop stamps with `meta.request_event_id` must
 *  appear here.** Anything missing falls through to the `current` pointer. It
 *  then leaks into a follow-up MR's empty exchange whenever the loop's events
 *  arrive after the follow-up. That leak flips `exchangeStatus` to 'aborted'
 *  for the follow-up.
 *
 *  `ContextCaptured` is the live event. `ContextAssembled` and
 *  `ContextTokensMeasured` are its retired predecessors, kept so legacy DB rows
 *  route the same way. `MemorySearched` is `MemoryRecalled`'s retired name and
 *  is kept for the same reason: the snapshot endpoint serves the raw
 *  `event_type` column, so the serde alias never reaches this list. */
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
  // Command-guard checkpoint (ADR 0002, Phase 4). Both carry the turn's
  // request_event_id. The revert is emitted at undo time, long after, yet
  // carries the ORIGINAL turn's id. Routing by it lands the revert back in
  // the checkpoint's exchange, so the card renders reverted.
  'CommandCheckpointed',
  'CommandCheckpointReverted',
]);

/** Skip req_id routing for Response* terminals AND context snapshots when their
 *  channel is CC: the session's persistent meta carries the original MR's
 *  req_id for the entire session. Routing Response* back by id would push a
 *  mid-flight cancel or abort to the original exchange, instead of terminating
 *  the active follow-up. Routing context snapshots back by id pulls a
 *  post-apply continuation's snapshots up to the first message, out from
 *  between the change banners. Keep them chronological on CC threads, folded
 *  into `current`. */
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
      // The context-snapshot variants don't declare `channel` in their TS type
      // (the wire payload carries it via EventMeta), so read it through a cast.
      return (event as { channel?: string }).channel !== 'claude_code';
    default:
      return true;
  }
}

/** Read `request_event_id` from any event payload. Rust's `EventMeta::apply()`
 *  adds the field whatever the event type, so the cast is honest about what
 *  arrives at runtime. */
function requestEventIdOf(event: { type: string }): string | undefined {
  return (event as { request_event_id?: string }).request_event_id;
}

/** The caller speaking, in either of the two events one utterance can be.
 *
 *  Every caller utterance is a `SpokenMessageReceived` today, whatever the
 *  talker does with it (ADR 0201). A row written before that carries the words
 *  in a `MessageReceived` with `voice_session_id`, which is the only thing
 *  marking such a message as spoken: ADR 0148 added no channel for voice.
 *
 *  **One predicate, because the reader cannot tell the two apart.** Both draw
 *  the same bubble and both open a boundary, so anything reasoning about "the
 *  caller said something here" has to see both. Splitting the question in two
 *  is how the re-anchor guard came to protect only half of a call. */
export function isCallerUtterance(event: { type: string; voice_session_id?: string }): boolean {
  if (event.type === 'SpokenMessageReceived') return true;
  return event.type === 'MessageReceived' && !!event.voice_session_id;
}

/**
 * How many unclaimed utterances are worth keeping.
 *
 * A row the engine wrote but the client never drew leaves its words here for
 * good. One that has not found a live row within this many later ones never
 * will. Holding it only widens the window where a caller repeating a sentence
 * has the wrong bubble retired.
 */
const UNCLAIMED_UTTERANCE_MEMORY = 8;

/** The words a live row or a persisted event carries, for the match below. */
function claimKey(text: string | undefined): string | undefined {
  const trimmed = text?.trim();
  return trimmed ? trimmed : undefined;
}

/**
 * Retire the live rows whose words the engine has now written down.
 *
 * **A row is claimed by the WORDS it carries, matched against the words the
 * engine wrote.** Never by a count, of either shape. The plan
 * `docs/plans/2026-09-14-the-transcript-shows-a-call-as-it-happens.md` traces
 * the two orderings a count gets wrong, and both are reachable today.
 *
 * One turn of the browser's gate is not one transcription item, so the engine
 * can write TWO rows against one live row. And it writes NO row for words the
 * caller spent answering a question card. Words pair each row with its own
 * sentence, so neither case can mispair.
 *
 * A row with no final words matches nothing, which is the barge-in guarantee
 * by construction. A partial is not words either.
 *
 * **A landing row also retires every EARLIER row of its own stretch.**
 * `call.rs` accumulates a caller stretch across provider items, appending as
 * it goes. So a row drawn part-way through carries a strict PREFIX of the
 * words that land. Matched on equality alone, those rows are orphans: the one
 * reported read `bit` and sat under a "Requesting" header until the hangup
 * swept it.
 *
 * Prefix, never containment. A short fragment appearing in the middle of an
 * unrelated sentence must not retire the bubble somebody is speaking now.
 *
 * **Trimmed on both sides, because one leg of the chain trims**, which is
 * `doer.rs::wake`. Called from two places, because a row and the words that
 * claim it arrive on two transports and `call.rs` emits the row FIRST.
 */
export function claimUtteranceRows(thread: ThreadState): void {
  const owed = thread.unclaimedUtterances;
  if (!owed || owed.length === 0) return;
  // Runs with NO rows too, so the trim reaches a call whose bubbles the client
  // never drew. Returning early there would grow the list for the whole call.
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

/** Are these the words of a row the landing one grew out of?
 *
 *  A strict prefix, and a shorter one: equality is the claim above, and a row
 *  the engine has not finished accumulating is never longer than the row it
 *  ends up in. */
function isEarlierInStretch(text: string | undefined, landed: string): boolean {
  const words = claimKey(text);
  return words !== undefined && words.length < landed.length && landed.startsWith(words);
}

/** Add what the engine just wrote to the unclaimed words, then claim. */
export function recordCallerUtterance(thread: ThreadState, text: string): void {
  const words = claimKey(text);
  // A wordless row claims nothing and can be owed nothing: `call.rs` refuses
  // to hold a transcript with no words, so one here writes no live row either.
  if (!words) return;
  thread.unclaimedUtterances = [...(thread.unclaimedUtterances ?? []), words];
  claimUtteranceRows(thread);
}

/** Claim a standing row with these words, and remember nothing if none match.
 *
 *  For a HISTORY replay, where the words may belong to a call that ended long
 *  ago. Remembering one would let an old sentence retire a bubble somebody is
 *  speaking now, which is why the replay restores the unclaimed list. */
export function offerCallerUtterance(thread: ThreadState, text: string): void {
  const words = claimKey(text);
  const rows = thread.liveUtterances;
  if (!words || !rows || rows.length === 0) return;
  const at = rows.findIndex(row => claimKey(row.text) === words);
  if (at !== -1) thread.liveUtterances = rows.filter((_, i) => i !== at);
}

/** True when this exchange draws a stretch of a call and holds no turn.
 *
 *  A spoken boundary STARTS nothing, so by default it owns nothing: not the
 *  live stream, not a place in the queue. Both the fold and the active-turn
 *  search step over it.
 *
 *  **Unless a running turn continued into it.** Starting a turn and holding
 *  one are different facts, and only the second decides whose status this card
 *  reports. See `Exchange.tookTheTurn`.
 *
 *  A DELEGATED utterance is deliberately not one of these. That one is a turn. */
export function exchangeHoldsNoTurn(exchange: Exchange): boolean {
  if (exchange.tookTheTurn) return false;
  if (isLiveUtteranceRow(exchange.userEvent)) return true;
  const type = exchange.userEvent.type;
  return type === 'SpokenMessageReceived' || type === 'SpokenReplyGenerated';
}

/** A boundary opened by something said on a call, in either direction.
 *
 *  The caller's two, plus a greeting. A greeting opens a boundary only where
 *  there was no exchange to land in. A reader asking "is this exchange a
 *  stretch of a call" wants it either way. */
export function isCallBoundary(event: { type: string; voice_session_id?: string }): boolean {
  return event.type === 'SpokenReplyGenerated' || isCallerUtterance(event);
}

/** Read `tool_use_id` from a CodingAgentTool* event payload. Empty string in
 *  legacy DB rows from before the field existed — normalize to `undefined`. */
export function toolUseIdOf(event: { type: string }): string | undefined {
  const id = (event as { tool_use_id?: string }).tool_use_id;
  return id ? id : undefined;
}

/** Grow a spoken row by a fragment that continues it, or answer null.
 *
 *  The rule is `spokenMerge.ts` and the glossary's § Spoken merge; this places
 *  it. The row keeps the FIRST fragment's identity, so a merged bubble is that
 *  row grown: its render key and its place in the transcript never move as the
 *  rest of the sentence arrives.
 *
 *  **`created` advances to the NEWEST fragment**, which the identity does not.
 *  So the gap is measured between neighbours, exactly as `push_spoken`
 *  measures it, and a long run of quick pieces stays one utterance. Keeping
 *  the first stamp timed a sentence out against its own opening word, and made
 *  the two readers split it differently. The shared fixture cannot see that:
 *  it pins the rule, and this is the caller.
 *
 *  **The AGE grows to span both**, so the merged row still reads where the
 *  first fragment began (ADR 0206). `created` moved, so an age left alone
 *  would slide the bubble down as its own tail arrived.
 *
 *  Adjacency is the caller's job too: anything between two fragments means
 *  they are two things said. */
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
  // `interrupted` describes how the row ENDED, so the newest piece owns it.
  const interrupted = (next as { interrupted?: boolean }).interrupted;
  // Measured from where the merged row now reads back to where the first
  // fragment began. A fragment with no age of its own began at its `created`,
  // so the sum holds whether either piece carries one. Only a reply wears the
  // field: the caller's row is a boundary, ordered by `created` alone.
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

/** The step types that end a turn. A turn holding one is over.
 *
 *  Lives here rather than beside its render consumers, because the FOLD needs
 *  the same answer: a turn that already ended has no continuation to hand to a
 *  later exchange. */
export const TERMINAL_EVENT_TYPES: ReadonlySet<string> = new Set([
  'ResponseGenerated',
  'ResponseFailed',
  'ResponseCanceled',
  'ResponseAborted',
  'CodingAgentIdled',
]);

/** Is this exchange's turn still going, as far as its own steps say? */
function stillRunning(exchange: Exchange): boolean {
  return !exchange.steps.some(({ event }) => TERMINAL_EVENT_TYPES.has(event.type));
}

/** Move a running turn's continuation from one exchange to a later one.
 *
 *  Called when a boundary opens under a turn that keeps going. Everything the
 *  turn emits from here reads BELOW that boundary, which is where it happened.
 *
 *  Two writes, both needed. The loop moves any redirect that pointed at
 *  `previous`, covering a turn that kept an ANCESTOR's req_id. Mapping
 *  `previous`'s OWN anchor id is unconditional, and is the write that matters
 *  when `previous` opened a fresh turn: its continuation streams under that
 *  card's own id, so the moved entries are spurious leftovers. A redundant
 *  entry is harmless, since nothing routes by an unused id. */
function handOverTheTurn(
  state: GroupFoldState,
  previous: Exchange,
  next: Exchange,
  touched: Set<Exchange> | null,
): void {
  // Nothing else lands in `previous`, so a `Thinking` marker left pending
  // there can never resolve on its own events. Record the handoff so rendering
  // finalizes it. See `Exchange.continuationMoved`.
  previous.continuationMoved = true;
  touched?.add(previous);
  for (const [reqId, exchange] of state.reqIdRedirect.entries()) {
    if (exchange === previous) state.reqIdRedirect.set(reqId, next);
  }
  const anchorId = previous.userEvent._eventId;
  if (anchorId) state.reqIdRedirect.set(anchorId, next);
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
 *  not guaranteed to match wall-clock order across concurrent writes.
 *  Delegates to `compareSortKeys` — the incremental cache's append-only check
 *  must agree with this ordering, so there is exactly one comparator. */
export function sortEventsChronologically(
  events: Map<number, StoredEvent>,
): SequencedEvent[] {
  // Parse each `created` once, up front: the sort itself makes O(n log n)
  // comparisons over O(n) events.
  const keyed = [...events.entries()].map(([seq, event]) => ({
    seq,
    event,
    micros: instantMicros(event.created),
  }));
  keyed.sort((a, b) => compareSortKeys(a.micros, a.seq, b.micros, b.seq));
  return keyed.map(({ seq, event }) => ({ seq, event }));
}

/** Event types that, as a step of an unanswered `UserQuestionAsked` divider,
 *  mean the agent has raced past the question. The QuestionCard's buttons then
 *  disable. Mirrors the Rust `ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES`
 *  constant (see
 *  `crates/lucidos-engine/src/engine/thread_events/event_impl.rs`).
 *
 *  **Keep both lists in sync.** Four gates hang off them. Server-side, the
 *  engine constant gates typed-text routing, and, unioned with three extras,
 *  the restart preserve guard (`unanswered_question_exists_sql`). Client-side,
 *  this set gates the click-button affordance and whether `exchangeStatus` may
 *  still read "Needs your answer". A card whose buttons are dead must not be
 *  labelled answerable. A thread whose card is dead must not be preserved as a
 *  resumable checkpoint. */
const QUESTION_OVERTAKEN_STEP_TYPES: ReadonlySet<string> = new Set([
  // Terminal (both agents)
  'ResponseAborted',
  'ResponseCanceled',
  'ResponseFailed',
  'CodingAgentIdled',
  // CC progression
  'CodingAgentTextStreamed',
  'CodingAgentToolCalled',
  'CodingAgentToolResult',
  'CodingAgentPromptSent',
  // Chat-agent progression (symmetry; harmless on CC threads)
  'TextStreamed',
  'ThoughtStreamed',
  'ToolCalled',
  'ToolResult',
]);

function markOvertakenQuestionDividers(exchanges: Exchange[]): void {
  for (const exchange of exchanges) {
    markOvertakenForExchange(exchange);
  }
}

/** Per-exchange half of `markOvertakenQuestionDividers`. The flag depends only
 *  on the exchange's OWN steps, so the incremental path re-runs this on exactly
 *  the exchanges an appended event touched. */
function markOvertakenForExchange(exchange: Exchange): void {
  if (exchange.userEvent.type !== 'UserQuestionAsked') return;
  const toolUseId = exchange.userEvent.tool_use_id;
  if (findQuestionAnswer(exchange, toolUseId)) {
    exchange.questionOvertaken = false;
    return;
  }
  exchange.questionOvertaken = exchange.steps.some(s =>
    QUESTION_OVERTAKEN_STEP_TYPES.has(s.event.type),
  );
}

/** Resumable fold state behind `groupIntoExchanges` and the incremental cache.
 *  Everything the per-event walk reads or writes lives here, so the fold can
 *  stop after any event and continue later with appended events. That is the
 *  basis of the per-thread memoization in `computeExchanges`. */
interface GroupFoldState {
  exchanges: Exchange[];
  current: Exchange | null;
  // tool_use_id to the exchange owning the matching CodingAgentToolCalled step.
  // Queried when a CodingAgentToolResult lands, so it can be re-routed to the
  // call's exchange even if a permission-request boundary intervened.
  toolCallOwners: Map<string, Exchange>;
  // Chat ToolCalled.event_id to owner exchange, the primary routing path for
  // chat ToolResults. Required because an `ask_user_question` call is followed
  // by a `UserQuestionAsked` divider exchange. The request-id redirect moves to
  // that divider. The post-answer `ToolResult` shares the originating MR's
  // req_id, so it would otherwise follow the redirect into the divider. The
  // original call's "Executing" spinner would then stay pending forever.
  // Legacy rows without the field fall through to request-id / `current`.
  chatToolCallOwners: Map<string, Exchange>;
  // tool_use_id to the UserQuestionAsked divider that owns it, and request_id
  // to the CodingAgentPermissionRequest divider. A resolution is neither
  // request-id routed nor a boundary, so by default it follows `current`. When
  // a boundary lands between the divider and its answer, `current` is that
  // boundary: the answer strands there and the divider stays stuck on
  // 'awaiting-answer'. Route the resolution back to its divider by id.
  questionDividerOwners: Map<string, Exchange>;
  permissionDividerOwners: Map<string, Exchange>;
  /** The card a `WorkDelegated` just marked as holding a turn, or null.
   *
   *  Held for exactly one boundary, so a legacy row order can be corrected.
   *  Before ADR 0201 the delegation was written BEFORE the `MessageReceived`
   *  that started the turn, and that message is the real starter. The mark
   *  lands on whatever the delegation followed, and this is how it comes off
   *  again. Cleared by any other boundary, so it can never reach further. */
  lastDelegationHost: Exchange | null;
  /** request_id to the divider that interrupted that turn.
   *
   *  A cancel says so on the divider's own card, so a standalone "Response
   *  canceled" panel under it would be a second telling. Reading the cancel's
   *  TARGET answers that only while the divider still holds the turn, and a
   *  caller speaking moves it on (see `handOverTheTurn`). Remembering the
   *  divider keeps the suppression attached to the card that tells. */
  turnDividers: Map<string, Exchange>;
  /** request_id to the tool call a permission card is holding: the exchange
   *  owning the call step, plus that step's `seq`. Written when the request is
   *  folded, read when its resolution is, so both ends mark the same row. It is
   *  the resolution that needs it. The divider knows the tool identity, but on
   *  a chat lane the call is found positionally and that position is long gone
   *  by then. See `Exchange.blockedStepSeqs`. */
  gatedCalls: Map<string, { exchange: Exchange; seq: number }>;
  // request_event_id to redirect target exchange. Set when a UPI is absorbed
  // mid-flight. The loop emits the UPI when it ingests the queued follow-up, so
  // every event after that answers the absorbed prompt rather than the original
  // request. Without the redirect, the post-injection tools and the final
  // ResponseGenerated stay in the original exchange, and the follow-up panel
  // renders as an empty stub.
  reqIdRedirect: Map<string, Exchange>;
  /** request_event_ids of every ResponseGenerated / ResponseFailed folded so
   *  far. The incremental path uses it to classify a late-arriving abort as
   *  legacy-superseded (terminal-before-abort direction). */
  resolvedReqIds: Set<string>;
  /** request_event_ids of every ResponseAborted folded so far. A terminal
   *  arriving later with a matching id retro-classifies that abort
   *  (abort-before-terminal direction), which the incremental path detects,
   *  falling back to a full rebuild. */
  abortReqIds: Set<string>;
  /** request_event_id of the most recent request-id-routed chat event, i.e. the
   *  active chat turn's req_id. The divider redirect bootstrap reads it to
   *  target the divider directly. It cannot trust `previousCurrent`, which can
   *  be an UNINGESTED queued MessageReceived that intervened. The invariant it
   *  relies on: a chat `ask_user_question` or permission prompt is always
   *  preceded in the same turn by its request-id-routed tool call. Undefined
   *  until the first routed chat event, so the chat-divider redirect is a no-op
   *  on a pure CC thread. */
  lastChatTurnReqId?: string;
  /** Does the server hold events older than the oldest one folded here?
   *
   *  The one thing that tells a page starting mid-turn from a corrupt thread.
   *  Both reach the fold as steps with no boundary behind them, and only this
   *  says which. See `Exchange.continuationFragment`.
   *
   *  Held on the fold state rather than passed down, because
   *  `groupIntoExchangesCached` resumes a fold across calls and must rebuild
   *  when the answer changes. */
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
    gatedCalls: new Map(),
    reqIdRedirect: new Map(),
    resolvedReqIds: new Set(),
    abortReqIds: new Set(),
  };
}

/** What a call leaves in the transcript with no turn behind it, and so belongs
 *  at the BOTTOM rather than in whatever turn is running.
 *
 *  Speech reads back in the order it was said, and a caller's utterance takes
 *  no turn, so it is the bottom without being `current`. These rows are the
 *  answer to it and the session marks around that answer.
 *
 *  `WorkDelegated` is deliberately absent. It has an arm of its own: it lands
 *  on the utterance it delegates rather than at the bottom, and that card
 *  becomes the turn's owner (ADR 0201). */
const CALL_ROW_TYPES: ReadonlySet<string> = new Set([
  'SpokenReplyGenerated',
  'VoiceSessionStarted',
  'VoiceSessionEnded',
]);

/** Everything a call can leave in an exchange without a turn behind it.
 *
 *  The spoken reply is what a reader sees. The session pair and the delegation
 *  marker draw nothing, yet they still fold in as steps. A set naming only the
 *  visible one would answer `false` for most real calls.
 *
 *  A delegation is in the set deliberately, and it is the one row here that
 *  can sit on a card holding a turn (ADR 0201). It still draws nothing, so it
 *  reports no work landing HERE. What says a turn is running is
 *  `Exchange.tookTheTurn`, which `isCallOnly`'s callers read separately.
 *
 *  The caller's own utterance is NOT here, and cannot be: it is an exchange
 *  start type, so the fold gives it a boundary and never a step. */
export const VOICE_ONLY_STEP_TYPES: ReadonlySet<string> = new Set([
  'SpokenReplyGenerated',
  'VoiceSessionStarted',
  'VoiceSessionEnded',
  'WorkDelegated',
]);

/** Steps a call leaves in an exchange that draw no row at all.
 *
 *  `VOICE_ONLY_STEP_TYPES` without the spoken reply, and that gap is the
 *  point. A reply is what the reader sees, so it separates the words either
 *  side of it. The rest is the talker's own bookkeeping.
 *
 *  An explicit list, not a clever predicate. Nothing in an event's shape says
 *  whether the renderer draws it, so it is stated per type. The same choice
 *  `UNANCHORABLE_ASYNC_EVENTS` makes below. */
const DRAWS_NO_ROW: ReadonlySet<string> = new Set([
  'VoiceSessionStarted',
  'VoiceSessionEnded',
  'WorkDelegated',
]);

/** Did the reader meet anything inside this exchange?
 *
 *  Asked of the bubble a new spoken fragment might grow. The transcriber cuts
 *  a sentence wherever the speaker breathes, and the talker files its own
 *  bookkeeping in that gap. A delegation lands within milliseconds of the
 *  words that prompted it.
 *
 *  Reading that marker as a separator cut one sentence into two bubbles under
 *  two headers. Only what the reader SAW may split them. */
function readerMetNothing(exchange: Exchange): boolean {
  return exchange.steps.every(({ event }) => DRAWS_NO_ROW.has(event.type));
}

/** Events that merely LANDED in a turn rather than being produced by it, and
 *  that render nothing of their own.
 *
 *  Two readers, and both need the same claim about CAUSATION.
 *  `isUningestedMessage` below asks whether a turn has landed here, so a row
 *  that only arrived is no evidence. `deepLinkAnchorForEvent`
 *  (exchange-render.ts) would otherwise show the turn "containing" the step,
 *  which no turn produced. A background bash task finishing under an open
 *  question would pulse that question, and the two are causally unrelated.
 *
 *  Deliberately an explicit list rather than a clever predicate. Nothing in the
 *  event's shape reveals causation, so it is stated per type with the reasoning
 *  attached and grows on evidence.
 *
 *  `BackgroundBashStarted` is deliberately NOT here. The turn's own
 *  `run_bash_background` call emits it, so landing there is honest. Only the
 *  COMPLETION floats free, firing whenever the process happens to exit. */
export const UNANCHORABLE_ASYNC_EVENTS: ReadonlySet<string> = new Set([
  'BackgroundBashCompleted',
]);

/** A user message the agentic loop has not picked up yet, in EITHER direction.
 *
 *  **Judged by whether a TURN has landed here, not by whether the exchange is
 *  stepless.** A call files rows of its own into whatever is open. So a stall
 *  the talker spoke, or a delegation it made, leaves a step behind without the
 *  loop having touched the message. Read as ingested, the message takes the
 *  running turn's live stream and badge, and loses its own place in the queue.
 *
 *  A queued message is `current` for as long as the previous turn runs, so an
 *  async row carrying no `request_event_id` lands in it. That is
 *  `UNANCHORABLE_ASYNC_EVENTS`, counted here for the reason a voice row is: it
 *  arrived, the loop did not put it there.
 *
 *  An optimistic one carries `_displayCreated` and no `created`, a persisted
 *  queued one carries `created`. Both wait until a `UserPromptInjected` lands
 *  and is absorbed. */
export function isUningestedMessage(exchange: Exchange): boolean {
  if (exchange.userEvent.type !== 'MessageReceived') return false;
  // A live utterance is no message at all yet. Nothing was sent, so nobody
  // waits on the loop to take it. Counted as awaiting, it would take the
  // running turn's stream the moment a caller drew breath.
  if (isLiveUtteranceRow(exchange.userEvent)) return false;
  return exchange.steps.every(
    s => VOICE_ONLY_STEP_TYPES.has(s.event.type) || UNANCHORABLE_ASYNC_EVENTS.has(s.event.type),
  );
}

/** The narrower half: a message the reader may RETRACT.
 *
 *  A DELEGATED utterance is uningested in the same window and is NOT one of
 *  these. It is speech, and nothing offers to unsay something.
 *  `voice_session_id` is what tells them apart. */
export function isWaitingTypedMessage(exchange: Exchange): boolean {
  if (!isUningestedMessage(exchange)) return false;
  return !(exchange.userEvent as { voice_session_id?: string }).voice_session_id;
}

/** The newest exchange a call row may be filed in, or null.
 *
 *  Read off the array rather than tracked. "The bottom" is a fact about the
 *  array, and a pointer would need clearing everywhere one can appear. Those
 *  places do not all open a boundary: an absorb re-anchors a message by moving
 *  it, and an abort, a cancel and a user stop each push a panel and return
 *  early. A pointer missed every one of them.
 *
 *  Two kinds are stepped over, both because a row filed there is a row nobody
 *  can read. A message still WAITING has had nothing happen in it, and a step
 *  there takes it out of the queue, beyond Stop's reach. A user's Stop-waiting
 *  panel draws no body at all.
 *
 *  Null means nowhere can hold the row. A reply then opens a boundary of its
 *  own, the greeting arm in `foldEvent`, and a session mark is dropped.
 *  Deliberately NOT `current`, which is routinely the waiting message. */
function callRowTarget(exchanges: Exchange[]): Exchange | null {
  const at = liveReplyTargetIndex(exchanges);
  return at === -1 ? null : exchanges[at];
}

/** Where a reply lands, as an index. `-1` means nowhere can hold it.
 *
 *  Shared with `withLiveCallRows`, so the LIVE row is drawn in the block the
 *  persisted one will be filed into. Two rules would drift, and the drift a
 *  reader sees is a bubble jumping between blocks as the row lands. */
function liveReplyTargetIndex(exchanges: Exchange[]): number {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const exchange = exchanges[i];
    if (isWaitingTypedMessage(exchange)) continue;
    if (isTurnlessBoundary(exchange.userEvent)) continue;
    return i;
  }
  return -1;
}

/** Fold an event set into turns.
 *
 *  `paged` says the server holds events older than the oldest one here. That
 *  is what opens a *continuation fragment* for a turn starting off the page.
 *  It defaults to "this set is the whole thread", the truth for a fixture and
 *  for every thread short enough to arrive in one response.
 *
 *  Only a caller reading a THREAD may take the default, and `computeExchanges`
 *  is the one that does, off `hasOlderEvents`. A paged set folded as whole
 *  silently drops every step ahead of its first boundary. */
export function groupIntoExchanges(events: Map<number, StoredEvent>, paged = false): Exchange[] {
  return foldSorted(sortEventsChronologically(events), paged).exchanges;
}

/** Incremental memo entry for one thread's events map. Valid only while events
 *  arrive append-only in sort order. The validation pass in
 *  `groupIntoExchangesCached` falls back to a full rebuild the moment that
 *  contract breaks. Keyed by the Map OBJECT in a WeakMap: handleEvent never
 *  re-sets an existing seq, and `rebuildCorruptedThreadEvents` replaces the
 *  Map, which misses here and discards the stale entry. */
interface IncrementalCache {
  fold: GroupFoldState;
  /** Entries folded so far. New events are exactly the iteration-order
   *  suffix past this count (Map preserves insertion order). */
  processedCount: number;
  /** Sort key (created instant, seq) of the last folded event. Appended
   *  events must not sort before it. */
  lastCreatedMicros: number | null;
  lastSeq: number;
  /** Flipped false when an event lacks `created` (legacy rows). The sort
   *  comparator is then no longer a total order to append-check against, so
   *  this map full-computes on every call. */
  cacheable: boolean;
}

const incrementalCache = new WeakMap<Map<number, StoredEvent>, IncrementalCache>();

/** The sort comparator of `sortEventsChronologically`, as a key compare:
 *  created (when both present) with seq as tiebreak, else seq.
 *
 *  Exported so `render-order.ts` reads the transcript's order off the same
 *  comparator the fold sorted it with. */
export function compareSortKeys(
  aMicros: number | null,
  aSeq: number,
  bMicros: number | null,
  bSeq: number,
): number {
  // Instants, never the raw strings: `instantMicros` documents why a lexical
  // compare of a server timestamp is wrong. Callers parse ONCE per event
  // rather than once per comparison, this running O(n log n) times per fold.
  // Same-millisecond events, and the legacy missing-`created` case, fall
  // through to the `seq` tiebreak.
  if (aMicros !== null && bMicros !== null && aMicros !== bMicros) {
    return aMicros < bMicros ? -1 : 1;
  }
  return aSeq - bSeq;
}

/** Full rebuild: run the one-shot fold and store its state for continuation. */
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

/** Memoized `groupIntoExchanges`. The result is always deep-equal to the
 *  from-scratch pass, pinned by incremental-grouping.test.ts. The array is a
 *  fresh copy each call, so signal subscribers fire on identity. The Exchange
 *  objects inside stay identity-stable across appends, so per-exchange
 *  memoization holds. */
function groupIntoExchangesCached(events: Map<number, StoredEvent>, paged: boolean): Exchange[] {
  const cache = incrementalCache.get(events);
  if (!cache) return rebuildIncrementalCache(events, paged);
  // A cold open fills this Map and only then learns the thread is paged, so
  // the answer can change under an unchanged Map. The fragments the fold owes
  // depend on it, so a different answer is a different fold.
  if (cache.fold.paged !== paged) return rebuildIncrementalCache(events, paged);
  if (!cache.cacheable) return groupIntoExchanges(events, paged);
  if (events.size === cache.processedCount) return [...cache.fold.exchanges];
  if (events.size < cache.processedCount) return rebuildIncrementalCache(events, paged);

  // New events are the insertion-order suffix. Sort the batch with the full
  // comparator so two events arriving in one frame fold in sorted order.
  const appended: SequencedEvent[] = [];
  let i = 0;
  for (const [seq, event] of events) {
    if (i++ < cache.processedCount) continue;
    appended.push({ seq, event });
  }
  appended.sort((a, b) =>
    compareSortKeys(instantMicros(a.event.created), a.seq, instantMicros(b.event.created), b.seq),
  );

  // Validation pass. Every appended event must keep the fold resumable.
  // `batchAbortReqIds` covers the abort-then-terminal pair arriving INSIDE one
  // batch. The abort is not in `cache.fold.abortReqIds` yet, that set being fed
  // only by foldEvent, so checking the cache set alone would miss the
  // retro-classification.
  let prevMicros = cache.lastCreatedMicros;
  let prevSeq = cache.lastSeq;
  const batchAbortReqIds = new Set<string>();
  for (const { seq, event } of appended) {
    const micros = instantMicros(event.created);
    if (micros === null) {
      // Legacy row without a timestamp — give up on caching this map.
      cache.cacheable = false;
      return groupIntoExchanges(events, paged);
    }
    if (compareSortKeys(micros, seq, prevMicros, prevSeq) < 0) {
      // Out-of-order arrival (e.g. a refresh replay delivering a missed
      // event): its sorted position is in the middle, not the end.
      return rebuildIncrementalCache(events, paged);
    }
    const reqId = requestEventIdOf(event);
    if (event.type === 'ResponseAborted' && reqId) {
      batchAbortReqIds.add(reqId);
    }
    if (
      (event.type === 'ResponseGenerated' || event.type === 'ResponseFailed') &&
      reqId &&
      (cache.fold.abortReqIds.has(reqId) || batchAbortReqIds.has(reqId))
    ) {
      // Legacy rerun-in-place: this terminal retro-classifies an
      // already-folded (or same-batch) ResponseAborted as a superseded step.
      return rebuildIncrementalCache(events, paged);
    }
    prevMicros = micros;
    prevSeq = seq;
  }

  const touched = new Set<Exchange>();
  for (const { seq, event } of appended) {
    const reqId = requestEventIdOf(event);
    const superseded =
      event.type === 'ResponseAborted' && !!reqId && cache.fold.resolvedReqIds.has(reqId);
    foldEvent(cache.fold, seq, event, superseded, touched);
  }
  for (const exchange of touched) {
    markOvertakenForExchange(exchange);
    // In-place mutation is invisible to identity-based memo comparison, so
    // bump the captured-at-render revision to make memoized components render.
    exchange.revision = (exchange.revision ?? 0) + 1;
  }
  cache.processedCount = events.size;
  cache.lastCreatedMicros = prevMicros;
  cache.lastSeq = prevSeq;
  return [...cache.fold.exchanges];
}

/** One-shot fold over an already-sorted event list. Runs the legacy
 *  rerun-in-place pre-pass, folds every event, and applies the
 *  question-divider marking. Both `groupIntoExchanges` and the cache rebuild
 *  go through here. */
function foldSorted(sorted: SequencedEvent[], paged: boolean): GroupFoldState {
  // Legacy rerun-in-place. When a ResponseAborted shares request_event_id with
  // a later ResponseGenerated or ResponseFailed, the rerun re-used the original
  // exchange. Do not split at those aborts: supersededAbortIndices in
  // exchangeStatus deflates the verdict to the later success.
  //
  // Two passes: record the request_event_id of every resolving terminal, then
  // mark the aborts that match one. Position is deliberately not compared, so a
  // terminal BEFORE the abort suppresses it too.
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

  const state = newFoldState(paged);
  for (const { seq, event } of sorted) {
    foldEvent(state, seq, event, legacySupersededAbortSeqs.has(seq), null);
  }
  markOvertakenQuestionDividers(state.exchanges);
  return state;
}

/** Re-anchor `exchange` to the position it should occupy now that the agent has
 *  engaged with it: as far down as it can go WITHOUT crossing something the
 *  caller said.
 *
 *  The end of the timeline when nothing was said after it, which is the common
 *  case for both callers. Otherwise just above the first utterance that
 *  followed. Used by the two paths where an exchange was created earlier than
 *  it was engaged with: the mid-flight `UserPromptInjected` absorb and
 *  `reanchorResolvedDivider`. No-op when it is already there.
 *
 *  All-or-nothing was wrong both ways. Moving to the end regardless prints the
 *  caller's own words out of the order they were said. Refusing to move at all
 *  leaves the exchange above a card it was engaged with AFTER, which is the
 *  whole reason a re-anchor exists. One utterance in the way should not cost
 *  the move against every other boundary.
 *
 *  Position is not part of an Exchange's own state, so callers relying on
 *  position-derived props need no `touched` bump. The render pass recomputes
 *  `isLast` / `hasPriorActive` / `priorModel` / `priorEffort` per exchange and
 *  `chatExchangePropsEqual` compares each, so a reorder re-renders on its own. */
function reanchorBelowSpeech(exchanges: Exchange[], exchange: Exchange): void {
  const from = exchanges.indexOf(exchange);
  if (from === -1) return;
  let wall = exchanges.length;
  for (let i = from + 1; i < exchanges.length; i++) {
    if (isCallerUtterance(exchanges[i].userEvent)) {
      wall = i;
      break;
    }
  }
  // `wall - 1` is where it lands once the splice closes the gap it left.
  const to = wall - 1;
  if (to <= from) return;
  exchanges.splice(from, 1);
  exchanges.splice(to, 0, exchange);
}

/** Did the caller speak after `exchange` opened? The one thing a re-anchor may
 *  not cross, see `reanchorResolvedDivider`.
 *
 *  Both halves of an utterance count (`isCallerUtterance`). The reader sees the
 *  same bubble either way, so moving a card below a delegated one reorders
 *  speech just as visibly. */
function callerSpokeAfter(exchanges: Exchange[], exchange: Exchange): boolean {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    if (exchanges[i] === exchange) return false;
    if (isCallerUtterance(exchanges[i].userEvent)) return true;
  }
  return false;
}

/** A divider just received its resolution (answer / permission grant). A
 *  boundary exchange appended while the card sat on screen leaves the divider
 *  no longer last, yet still OWNING the turn's continuation via
 *  `reqIdRedirect`. The usual shape is a spawned sub-thread emitting
 *  `ChildThreadCompleted`. Left in place, every post-answer step renders ABOVE
 *  that intervening card. Live work then reads mid-timeline while the bottom of
 *  the thread is a stepless card frozen on 'Requesting', as if stuck.
 *
 *  So re-anchor the divider to its RESOLUTION point: move it to the end and
 *  make it `current`. Same move as the mid-flight `UserPromptInjected` absorb
 *  above, for a different reason: that one follows the message the loop just
 *  picked up, this one follows the card the reader just answered.
 *
 *  Gated on the divider being a `reqIdRedirect` target, which is exactly the
 *  in-process chat dividers whose continuation routes back here by request id.
 *  A CC `CodingAgentPermissionRequest` is never a redirect target, CC events
 *  not being request-id routed. Its continuation flows through `current` to the
 *  intervening boundary, so moving the card would strand it. No-op with no
 *  boundary between, the divider being already last and already `current`.
 *
 *  **A caller's utterance stops the move outright**, both halves of it. The
 *  intervening boundary this was written for is a sub-thread finishing, which
 *  nobody said out loud. An utterance is speech, and so is what the card holds:
 *  every spoken line must read back in the order it was said. Moving the card
 *  below a later utterance reorders its own spoken rows. Handing it `current`
 *  sends the NEXT reply there too, rows early. The continuation still finds the
 *  card, routing by request id rather than by position, so what is given up is
 *  where it renders.
 *
 *  The hold costs the card nothing else, because an utterance never took
 *  `current` in the first place: the boundary branch hands it back. So the
 *  card keeps the continuation that routes chronologically too. */
function reanchorResolvedDivider(
  state: GroupFoldState,
  divider: Exchange,
  current: Exchange | null,
): Exchange | null {
  let ownsContinuation = false;
  for (const target of state.reqIdRedirect.values()) {
    if (target === divider) {
      ownsContinuation = true;
      break;
    }
  }
  if (!ownsContinuation) return current;
  reanchorBelowSpeech(state.exchanges, divider);
  if (callerSpokeAfter(state.exchanges, divider)) return current;
  // The resolved divider is the live turn again (see `Exchange.continuationMoved`).
  divider.continuationMoved = false;
  return divider;
}

/** The exchange holding the chat turn a permission request interrupted.
 *
 *  Normally `previousCurrent`, and NOT reliably so: a queued follow-up
 *  `MessageReceived` folding between the call and the card makes `current` that
 *  uningested MR instead, which holds no tool call at all. `lastChatTurnReqId`
 *  is the turn's own id, tracked for exactly this shape, and the divider
 *  redirect below resolves it the same way. It falls back to `previousCurrent`
 *  on a thread with no routed chat event, which is every pure coding-agent
 *  thread. */
function chatTurnOwner(state: GroupFoldState, previousCurrent: Exchange | null): Exchange | null {
  const reqId = state.lastChatTurnReqId;
  if (!reqId) return previousCurrent;
  return state.reqIdRedirect.get(reqId)
    ?? findExchangeByAnchorId(state.exchanges, reqId)
    ?? previousCurrent;
}

/** The step `seq` of the tool call a permission request is about, and the
 *  exchange holding it. `null` when the call cannot be located. That degrade
 *  covers a legacy row carrying no ids, and an orphan request whose call never
 *  folded.
 *
 *  Two lookups, because the two lanes carry different identity. A coding-agent
 *  request shares its `tool_use_id` with the call, on both Claude Code and
 *  Codex, so `toolCallOwners` finds the exchange across any boundary between
 *  them. A chat `ToolCalled` has no such id, so the chat lanes take the last
 *  call step of the turn the request interrupted (`chatTurnOwner`). That is
 *  exact because a gated call never joins a parallel run, so it is always the
 *  one call in flight when its request arrives (ADR 0246). */
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

/** Move a gated call out of its held state once the user has decided. Allowing
 *  it returns the row to the ordinary pending shimmer, which is truthful from
 *  here on: the tool starts running now. Denying it ends the row instead.
 *
 *  A no-op for a resolution whose request was never marked, which covers an
 *  orphan resolution and a request folded before this bookkeeping existed. */
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

/** Fold one event into the state. `isLegacySupersededAbort` is decided by the
 *  caller. `touched` collects every exchange this event mutated or created, so
 *  the incremental path can re-run the question-divider marking on exactly
 *  those. */
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
    const reqId = requestEventIdOf(event);
    if ((event.type === 'ResponseGenerated' || event.type === 'ResponseFailed') && reqId) {
      state.resolvedReqIds.add(reqId);
    }
    if (event.type === 'ResponseAborted' && reqId) {
      state.abortReqIds.add(reqId);
    }
  }
  // The walk body runs as a closure so its many early exits all funnel
  // through the single `state.current = current` sync below.
  const step = (): void => {
    if (NON_EXCHANGE_METADATA_EVENTS.has(event.type)) return;
    if (isAuxiliaryCapture(event)) return;

    // A release marks its held row delivered, so it joins the exchange that
    // holds the row, wherever the transcript has moved on to since.
    if (event.type === 'HeldMessageReleased') {
      const holder = exchanges.find(ex => ex.steps.some(s =>
        s.event.type === 'MessageHeld' && s.event._eventId === event.held_message_id));
      if (holder) {
        holder.steps.push({ seq, event });
        touched?.add(holder);
        return;
      }
    }

    // Walked once, and only for a row a call leaves behind. Every other event
    // short-circuits on the set membership and pays nothing.
    const callTarget = CALL_ROW_TYPES.has(event.type) ? callRowTarget(exchanges) : null;
    const reqId = shouldRouteByRequestId(event) ? requestEventIdOf(event) : undefined;
    // Remember the active chat turn's req_id (see `lastChatTurnReqId`) so the
    // divider redirect bootstrap below targets it directly, rather than
    // inferring it from `previousCurrent`.
    if (reqId) state.lastChatTurnReqId = reqId;
    const owner = reqId
      ? (reqIdRedirect.get(reqId) ?? findExchangeByAnchorId(exchanges, reqId))
      : null;

    // ResponseAborted is dual-purpose. It terminates the originating exchange,
    // so the partial-response panel reads 'Aborted'. It also opens a boundary
    // exchange whose userEvent is the abort itself, rendered as the AbortPanel.
    // The boundary always sits chronologically last, so the panel appears below
    // any newer MessageReceived in the timeline.
    if (event.type === 'ResponseAborted' && !isLegacySupersededAbort) {
      const target = owner ?? current;
      if (target && target.userEvent.type !== 'ResponseAborted') {
        target.steps.push({ seq, event });
        touched?.add(target);
        current = { userEvent: event, userSeq: seq, steps: [] };
        exchanges.push(current);
        touched?.add(current);
        return;
      }
    }
    // ResponseCanceled mirrors the abort dual-purpose pattern. Keep the cancel
    // as a step on the originating exchange, so its response panel reads
    // 'Canceled'. Also open a boundary exchange, so a separate 'Response
    // canceled' panel renders below the truncated reply. Skip the boundary only
    // when the question resolved as Canceled, the card already carrying the
    // attribution via its Cancel-as-picked button. Any other resolution leaves
    // the picked option visible, so the boundary panel must render.
    //
    // A `superseded_by_followup` cancel (a Codex mid-turn follow-up redirect)
    // is the exception. The user steered rather than Stopped, so it renders
    // neutrally like the chat or CC follow-up. Keep it as a step, so step
    // resolution and model extraction still see a terminator, but open NO
    // boundary: there must be no standalone 'Response canceled' panel.
    if (event.type === 'ResponseCanceled') {
      const target = owner ?? current;
      if (target && target.userEvent.type !== 'ResponseCanceled') {
        if (event.cause === 'superseded_by_followup') {
          target.steps.push({ seq, event });
          touched?.add(target);
          return;
        }
        target.steps.push({ seq, event });
        touched?.add(target);
        // The divider that interrupted this turn, which may no longer be where
        // the turn is showing: a caller speaking moves the continuation on.
        const teller = (reqId ? state.turnDividers.get(reqId) : undefined) ?? target;
        if (teller.userEvent.type === 'UserQuestionAsked') {
          // A question dismissed, or replaced by a follow-up, already says so
          // on its own card. A standalone "Response canceled" panel under it
          // would be a second telling.
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
    // Re-route ToolResult by tool_use_id when a permission boundary stranded
    // it from its call's exchange. Legacy events (no id) fall through.
    if (event.type === 'CodingAgentToolResult') {
      const id = toolUseIdOf(event);
      const callOwner = id ? toolCallOwners.get(id) : undefined;
      if (callOwner && callOwner !== current) {
        callOwner.steps.push({ seq, event });
        touched?.add(callOwner);
        return;
      }
    }
    // Chat ToolResult routing, the primary path: the chat agentic loop stamps
    // `tool_called_event_id` on every live emit, and the post-restart recovery
    // sweep does the same on synthetic backfills. See the `chatToolCallOwners`
    // declaration above for the rationale.
    //
    // The tracking map only knows the ToolCalled events seen so far in the
    // walk, so a synthetic ToolResult arriving before its ToolCalled misses
    // this branch. Neither producer emits that ordering: the live loop calls
    // before it results, and recovery emits after reading the orphan ToolCalled
    // out of the DB. Legacy rows without the field fall through to the
    // request_id / `current` routing.
    if (event.type === 'ToolResult') {
      const tcId = (event as { tool_called_event_id?: string }).tool_called_event_id;
      const callOwner = tcId ? chatToolCallOwners.get(tcId) : undefined;
      if (callOwner) {
        callOwner.steps.push({ seq, event });
        touched?.add(callOwner);
        return;
      }
    }
    // Route a question answer / permission resolution back to its divider by
    // id (see questionDividerOwners declaration). Without this, a boundary that
    // intervened between the divider and its resolution strands the answer in
    // the boundary exchange, leaving the divider stuck on 'awaiting-answer'.
    // Legacy / orphan resolutions with no matching divider fall through.
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
      // Ahead of the divider routing below, which returns early. The held row
      // must settle whether or not its card is still reachable by id.
      settleGatedCall(state, event.request_id, event.allowed, touched);
      const dividerOwner = permissionDividerOwners.get(event.request_id);
      if (dividerOwner) {
        dividerOwner.steps.push({ seq, event });
        touched?.add(dividerOwner);
        current = reanchorResolvedDivider(state, dividerOwner, current);
        return;
      }
    }
    // **The talker asked for the doer, which STARTS a turn** (ADR 0201). The
    // row is a step rather than a boundary: the caller's words already opened
    // the card, and a second one would split one utterance from its answer.
    //
    // Two writes make that card the turn's owner. `tookTheTurn` is what stops
    // the status machinery reading it as speech nobody is working on. The
    // redirect is how the turn's own events find it, since the anchor they
    // carry is this row's id and no exchange wears it.
    //
    // **Only onto a caller's utterance**, which is what the delegation is FOR.
    // A greeting is not one, so a row written before ADR 0201 leaves it alone:
    // there the `MessageReceived` behind the delegation is the turn's starter,
    // and marking whatever happened to be open gave a greeting a turn it never
    // held. `lastDelegationHost` takes the mark back off the rest.
    //
    // The card need NOT be empty. The talker routinely stalls before it asks,
    // and that stall is its own turn, so its row is already a step here.
    if (event.type === 'WorkDelegated' && current && isCallerUtterance(current.userEvent)) {
      current.steps.push({ seq, event });
      current.tookTheTurn = true;
      state.lastDelegationHost = current;
      if (event._eventId) reqIdRedirect.set(event._eventId, current);
      touched?.add(current);
      return;
    }
    const absorbTarget = findAbsorbTarget(state, current, exchanges, event);
    if (absorbTarget) {
      // A queued mid-flight message is ingested here, the UPI being the moment
      // the loop picked it up. Boundaries created while it sat in the queue
      // leave the optimistic MR panel positioned ABOVE them. The agent only
      // engages with the message now, so its panel and reply belong BELOW those
      // boundaries. Re-anchor to the ingestion point by moving it to the end.
      // No-op when it is already last, the common case.
      //
      // **A caller's utterance holds this move**, as it holds the divider's
      // (`reanchorResolvedDivider`). What waited here was SAID, and so is what
      // the talker stalled with while it waited, which `callRowTarget` files
      // right here. Moving it below a later utterance prints both after words
      // the caller said afterwards, and no audio is kept to correct that.
      //
      // Only the MOVE is held. The exchange still takes `current`, so the
      // turn's own bookkeeping keeps landing on it. What the call leaves
      // behind never reaches `current`, going to the bottom by name instead.
      reanchorBelowSpeech(exchanges, absorbTarget);
      absorbTarget.steps.push({ seq, event });
      touched?.add(absorbTarget);
      // It owns the turn again, so a handoff recorded while it sat in the queue
      // no longer holds. See `Exchange.continuationMoved`.
      absorbTarget.continuationMoved = false;
      // The redirect that handoff wrote no longer holds either, or the turn's
      // own terminal settles a card the loop has moved off. See
      // `handOverTheTurn`.
      const ownId = absorbTarget.userEvent._eventId;
      if (ownId) reqIdRedirect.set(ownId, absorbTarget);
      current = absorbTarget;
      if (event.type === 'UserPromptInjected') {
        const absorbedReqId = requestEventIdOf(event);
        if (absorbedReqId) reqIdRedirect.set(absorbedReqId, absorbTarget);
      }
    } else if (isExchangeStartEvent(event) && !isLegacySupersededAbort) {
      // A LEGACY delegated message, written after its own `WorkDelegated`.
      // That message is the turn's starter, so the mark the delegation left on
      // the card above comes off again. Any other boundary just spends the
      // slot, which is what keeps the correction to one row.
      const delegationHost = state.lastDelegationHost;
      state.lastDelegationHost = null;
      if (delegationHost && event.type === 'MessageReceived' && isCallerUtterance(event)) {
        delegationHost.tookTheTurn = undefined;
        touched?.add(delegationHost);
      }
      // One thing the caller said is one boundary, however many turns the
      // transcriber cut it into. A fragment continuing the row right above
      // grows it rather than opening a second bubble under a second header.
      //
      // Only the LAST one, and only while the reader has met nothing in it.
      // Something they SAW between two fragments means the caller said two
      // things. The talker's own bookkeeping is not that, and reading it as a
      // separator is what split a sentence around a delegation marker nobody
      // could see.
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
      // **The user's Stop-waiting boundary takes no ownership of the turn.**
      //
      // Every other boundary here ends the turn or takes it over, so becoming
      // `current` is right for them. A stop is neither. A subscription does not
      // hold its thread's turn (ADR 0049), and the Stop waiting button has no
      // idle guard. So the user can press it mid-flight on an unrelated turn,
      // and that turn keeps running afterwards.
      //
      // Whatever routes CHRONOLOGICALLY must therefore keep landing in the turn
      // that produced it: every coding-agent event, and on a chat thread a
      // `TodoListWritten` or a background-bash pair. Folded into this boundary
      // they would draw nothing, the stop panel rendering no response body, and
      // the running turn's pending `Thinking` marker would shimmer forever.
      //
      // Restoring `current` is the whole handling it needs, and is why it is
      // `own-arm` in `BOUNDARY_CONTINUATION_HANDOFF`. It continues nothing, so
      // there is no continuation to redirect and no handoff to record.
      //
      // A `ChildThreadStopped` is handled the same way: it wakes nothing, so a
      // turn the parent is running keeps writing where it was.
      if (isTurnlessBoundary(event)) {
        current = previousCurrent;
        return;
      }
      // **A caller's utterance STARTS no turn, and still takes the running
      // one's continuation.** The two are different questions, and conflating
      // them is what drew a doer's steps above words said before them.
      //
      // It starts nothing because the talker decides whether the doer is
      // wanted. It takes the continuation because the clock says so: every
      // step the turn emits from here happened AFTER these words, so it reads
      // below them (ADR 0201). The turn itself carries on untouched.
      if (event.type === 'SpokenMessageReceived') {
        // A call nobody delegated from has no turn to take, and the utterances
        // are then all there is. A turn that already ENDED has none either:
        // moving it would hand this card a continuation nobody is writing, and
        // the card would then sit "Requesting" for ever. Nothing to hand over,
        // and `current` stays on this one so whatever lands next has somewhere
        // to go.
        if (previousCurrent && !exchangeHoldsNoTurn(previousCurrent) && stillRunning(previousCurrent)) {
          handOverTheTurn(state, previousCurrent, current, touched);
          // It holds one now, so the status machinery must stop stepping over
          // it. See `Exchange.tookTheTurn`.
          current.tookTheTurn = true;
        }
        return;
      }
      // Register divider exchanges so their resolution can route back here by
      // id even if a boundary intervenes before the answer lands.
      if (event.type === 'UserQuestionAsked' && event.tool_use_id) {
        questionDividerOwners.set(event.tool_use_id, current);
      } else if (
        (event.type === 'CodingAgentPermissionRequest'
          || event.type === 'CommandPermissionRequested'
          || event.type === 'McpPermissionRequested')
        && event.request_id
      ) {
        permissionDividerOwners.set(event.request_id, current);
        // The card holds a tool call that has ALREADY opened a step row, one
        // event earlier and so in `previousCurrent`. Mark that row, or it
        // shimmers "In progress" over a tool blocked on a human.
        const gated = gatedCallOf(state, event, previousCurrent);
        if (gated) {
          state.gatedCalls.set(event.request_id, gated);
          (gated.exchange.blockedStepSeqs ??= new Set()).add(gated.seq);
          touched?.add(gated.exchange);
        }
      }
      // A turn INTERRUPTED by a boundary that then resumes under its own,
      // unchanged req_id. Without the redirect, everything after the boundary
      // routes back to the pre-boundary exchange, which sits ABOVE the card, so
      // the continuation renders first. `BOUNDARY_CONTINUATION_HANDOFF` decides
      // it per type, and carries the reason for every entry.
      //
      // The gated tool call and its result still re-route to the MR exchange by
      // tool_called_event_id, so only the genuine continuation moves below a
      // permission card. An IDLE wake is unaffected either way: the engine
      // starts a fresh turn anchored on the injection, which finds this
      // exchange.
      if (previousCurrent && boundaryTakesTheTurn(event.type, previousCurrent)) {
        handOverTheTurn(state, previousCurrent, current, touched);
      }
      // For a chat in-process divider, the post-answer continuation carries the
      // ACTIVE turn's req_id. That is `previousCurrent`'s anchor only when
      // `previousCurrent` IS that turn's exchange. An UNINGESTED queued
      // follow-up MessageReceived that intervened makes it the queued MR
      // instead. The bootstrap above then anchors on the WRONG id, and the
      // divider strands terminal-less on a persistent 'aborted'.
      //
      // Redirect the turn's real req_id, tracked as `lastChatTurnReqId`, to the
      // divider directly. Additive and idempotent in the common no-queue case,
      // where it already equals `previousCurrent`'s anchor. CC dividers never
      // set it, so this is a no-op for them. ChildThreadCompleted is excluded:
      // the `previousCurrent` logic above governs its continuation routing.
      if (
        (event.type === 'UserQuestionAsked'
          || event.type === 'CommandPermissionRequested'
          || event.type === 'McpPermissionRequested')
        && state.lastChatTurnReqId
      ) {
        // Whoever held that req_id just lost the turn. Normally that is
        // `previousCurrent`, marked above, but the queued-follow-up shape makes
        // them different exchanges. The handoff mark belongs on the one the
        // redirect is moved OFF. See `Exchange.continuationMoved`.
        const priorOwner = reqIdRedirect.get(state.lastChatTurnReqId)
          ?? findExchangeByAnchorId(exchanges, state.lastChatTurnReqId);
        if (priorOwner && priorOwner !== current) {
          priorOwner.continuationMoved = true;
          touched?.add(priorOwner);
        }
        reqIdRedirect.set(state.lastChatTurnReqId, current);
        // The card that will tell the user if this turn is cancelled, however
        // far the continuation travels afterwards.
        state.turnDividers.set(state.lastChatTurnReqId, current);
      }
    } else if (event.type === 'CodingAgentUserMessageSent') {
      // Legacy: old data has this instead of MessageReceived for CC follow-ups.
      // New data emits both for the same user message, so skip creating a
      // duplicate exchange if one already exists.
      if (current && current.userEvent.type === 'MessageReceived' && current.steps.length === 0) {
        // MessageReceived already started this exchange — skip the duplicate
        return;
      }
      const text = (event as { text: string }).text;
      current = { userEvent: { type: 'MessageReceived', text } as StoredEvent, userSeq: seq, steps: [] };
      exchanges.push(current);
      touched?.add(current);
    } else if (event.type === 'CodingAgentPromptSent' && !current) {
      // Legacy engine-spawned CC threads emit a bare CodingAgentPromptSent as
      // the first content event. Promote it to a synthetic boundary so the
      // panel renders. Without this, every following step is dropped and the
      // thread shows the "Messages could not be displayed" empty state. Modern
      // threads always have a proper boundary first, so `current` is non-null
      // and the step branch below takes them.
      current = { userEvent: event, userSeq: seq, steps: [] };
      exchanges.push(current);
      touched?.add(current);
    } else if (event.type === 'SpokenReplyGenerated' && !callTarget) {
      // A call opens with a greeting, and it is said before anything has
      // started a turn. So there is no exchange for the row to land in, and
      // the `current` fallthrough below drops it silently. No audio is kept,
      // which makes a dropped spoken turn gone rather than merely unrendered:
      // a call where nobody delegates leaves a thread rendering nothing at all.
      //
      // Promoted to a boundary of its own, as the legacy prompt above is. It
      // takes NO steps: the words are in the event, and its initiator panel
      // draws them. Anything landing after it belongs to it as usual.
      //
      // Gated on there being nowhere to put it, which covers more than an
      // empty thread. A message the loop has not picked up yet is the other
      // way a transcript holds nothing a reply may go in.
      //
      // It takes `current` only when nothing else holds it. A turn that has
      // started but not yet emitted a step is one of those nowheres, and the
      // reply must not take its ownership: whatever that turn routes
      // chronologically would then land under a greeting.
      //
      // The caller's own utterance never reaches here. It is a boundary
      // wherever it lands, so the branch above has already taken it.
      const greeting: Exchange = { userEvent: event, userSeq: seq, steps: [] };
      exchanges.push(greeting);
      touched?.add(greeting);
      if (!current) current = greeting;
    } else if (CALL_ROW_TYPES.has(event.type)) {
      // A call row goes where the call is, and `callTarget` is the only place
      // that can hold one. Never `current`, which is routinely a message still
      // waiting to be picked up.
      //
      // With nowhere at all, the row is dropped. Only a session mark reaches
      // that: a reply took the boundary above, and a mark draws nothing, so
      // there is nothing to lose and nowhere to lose it from.
      if (callTarget) {
        // Filed by the clock, like every other row, and a reply's clock is
        // when its words BEGAN. The target is the BOTTOM exchange rather than
        // `current`, so it may already hold steps stamped after them.
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
      // Nowhere to put the step, which on a PAGED thread means the boundary
      // that opens this turn is older than the page. Hold it in a fragment
      // rather than dropping it. See `Exchange.continuationFragment`.
      const target = current ?? openContinuationFragment(state, seq, event, touched);
      if (!target) return;
      current = target;
      appendStep(current, seq, event);
      touched?.add(current);
      if (event.type === 'CodingAgentToolCalled') {
        const id = toolUseIdOf(event);
        if (id) toolCallOwners.set(id, current);
      }
      if (event.type === 'ToolCalled' && event._eventId) {
        chatToolCallOwners.set(event._eventId, current);
      }
    }
  };
  step();
  state.current = current;
}

/** Open a continuation fragment to hold a step whose turn starts off the page.
 *
 *  `null` on a thread served whole, where a step with no boundary behind it is
 *  the corruption the transcript already reports. The one signal separating the
 *  two is `GroupFoldState.paged`.
 *
 *  Its `userEvent` is the step itself. Nothing loaded names the turn, and a key
 *  the render can hold still across a re-fold has to come from somewhere real. */
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
  /** True when the event landed in this thread (event was not duplicate-by-seq
   *  and the thread existed). Mirrors the historical boolean return. */
  applied: boolean;
  /** True when any shape-relevant `thread.meta` field changed value. The
   *  `updatedAt` tick is intentionally excluded — see `applyAggregateToMeta`.
   *  Callers gate the global `threadMap` signal flush on this; events-only
   *  arrivals (streaming tokens, tool calls) bump the per-thread signal
   *  via `bumpThreadEvents` instead. */
  metaChanged: boolean;
  /** True when this persisted event replaced an optimistic user message.
   *  The focused thread uses this to keep the viewport pinned across the
   *  pending-row -> real-event swap. */
  clearedPendingUserMessage: boolean;
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

  // Backend-computed snapshot is the source of truth for thread.meta. Live
  // SSE attaches a per-event aggregate on persisted events; transient events
  // (e.g. ChildrenCountChanged from fanout) may also carry one when the
  // backend updated other projection fields out-of-band. fetchThreadEvents
  // replay applies a single currentAggregate after the loop (in
  // applyEventRows), so per-row calls here legitimately have no aggregate.
  if (aggregate) {
    const prevStatus = thread.meta.status;
    if (applyAggregateToMeta(thread.meta, aggregate)) metaChanged = true;
    if (thread.meta.status === 'running' && prevStatus !== 'running' && created) {
      thread.meta.lastRevivedAt = created;
      metaChanged = true;
    }
  }

  if (seq !== null) {
    if (thread.events.has(seq)) return { applied: false, metaChanged, clearedPendingUserMessage: false };
    if (!created) {
      // Best-effort diagnostic, not a user-intent action: this is an
      // SSE-ingest path, no toast is appropriate. The event is still stored
      // below regardless, so the UI stays correct — only drawer sort ordering
      // for this row may be approximate. A toast would surface a backend bug
      // the user can't act on; the warning is for the developer console.
      console.warn(`[handleEvent] persisted event ${event.type} (seq=${seq}) missing created timestamp — this indicates a backend bug`);
    }
    const stored: StoredEvent = { ...(event as ThreadEvent), created, ...(eventId ? { _eventId: eventId } : {}) };
    // CONTRACT: `thread.events` is append-only with deduped seqs, so the
    // `has(seq)` guard above is load-bearing. `groupIntoExchangesCached` keys
    // its memo on this Map object and detects new work by size plus
    // insertion-order suffix. An in-place re-set of an existing seq, a
    // delete(), or a clear() would serve STALE exchanges with no failure
    // signal. To rewrite a thread's events wholesale, replace the Map object
    // instead: a new Map misses the WeakMap and triggers a clean rebuild.
    thread.events.set(seq, stored);
    thread.streamingBuffer = '';
    // Update updatedAt only for events that the backend updates last_activity for.
    // Must stay in sync with update_thread_projection() in event_bus.rs.
    // Tick-only write — does not mark metaChanged (see `applyAggregateToMeta`).
    if (created && updatesLastActivity(event.type)) thread.meta.updatedAt = created;
    // The caller's words landed, so the row promising them has done its job.
    // Here rather than on the call's own clock: the engine holds a transcript
    // across the talker's decision, so `user_turn_ended` can precede this row
    // by a second or more. Dropping it there would reopen the silence.
    //
    // Taken by the WORDS a row carries, never by its count. See
    // `claimUtteranceRows`, which is the whole of the rule.
    if (isCallerUtterance(event)) {
      recordCallerUtterance(thread, (event as { text?: string }).text ?? '');
    }
    // The talker's row is a slot, so its own row landing settles it outright.
    //
    // Words cannot pair these two: the client concatenates the reply's deltas
    // and `done_transcript` joins the response's content parts with a space,
    // so the same reply reaches the two sides differently. The residue is
    // narrow and bounded by the sweep below, and the plan's non-goals name it.
    if (event.type === 'SpokenReplyGenerated') thread.liveReply = undefined;
    // The call is over, so nobody is owed a live row any more. This is the
    // backstop for the one utterance the engine writes no row for: words the
    // caller spent ANSWERING a question card, which `call.rs` drops because
    // the answer's own row already carries them. Left standing, such a row
    // shimmers on an idle thread for the life of the page.
    //
    // Safe because `call.rs` flushes whatever it still holds BEFORE it emits
    // this, on one bus and in order. So every row the engine did write has
    // already been claimed above.
    if (event.type === 'VoiceSessionEnded') {
      if (thread.liveUtterances?.length) thread.liveUtterances = [];
      thread.liveReply = undefined;
      thread.unclaimedUtterances = [];
    }
    // A real MessageReceived from the backend removes the matching optimistic
    // pending message by event_id.
    if ((event.type === 'MessageReceived' || event.type === 'UserPromptInjected') && thread.pendingUserMessages.length > 0) {
      if (eventId) {
        const idx = thread.pendingUserMessages.findIndex(p => p.eventId === eventId);
        if (idx !== -1) {
          thread.pendingUserMessages.splice(idx, 1);
          clearedPendingUserMessage = true;
        }
      } else {
        // Fallback for events without event_id (e.g. scheduled tasks, old data):
        // remove the oldest pending message (FIFO order)
        thread.pendingUserMessages.shift();
        clearedPendingUserMessage = true;
      }
    }
    // FreeText answers emit no MessageReceived, the backend routing typed text
    // straight to UserQuestionAnswered, so the optimistic pending message must
    // be cleared here too. Match by text, which the backend forwards verbatim.
    // A non-match indicates drift: let the safety timer clean it up rather
    // than shifting the wrong one.
    if (event.type === 'UserQuestionAnswered' && event.answer.kind === 'FreeText' && thread.pendingUserMessages.length > 0) {
      const text = event.answer.text;
      const idx = thread.pendingUserMessages.findIndex(p => p.text === text);
      if (idx !== -1) {
        thread.pendingUserMessages.splice(idx, 1);
        clearedPendingUserMessage = true;
      }
    }
    // Project the chat-agent Todo list into meta — replace-whole-list per call.
    // Replay re-establishes the same final state because every TodoListWritten
    // flows through this branch; live SSE updates it incrementally.
    if (event.type === 'TodoListWritten') {
      thread.meta.latestTodoList = event.items;
      // Replaced with the items, never merged: `todo_write` is
      // replace-whole-list and the notes are part of that list.
      thread.meta.latestTodoNotes = event.notes ?? null;
      metaChanged = true;
    }
    // Project the thread's live *event waits* into meta, the same way and for
    // the same reason as the Todo list above: the waiting indicator is
    // always mounted, so re-deriving this per render would walk the events Map
    // on every flush. Replay rebuilds the identical set because every
    // EventWait* flows through here in order.
    if (eventWaitProjection(thread.meta, event)) {
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
      // `CumulativeTextUpdated` carries the FULL accumulated text for the turn,
      // not a delta: the engine re-sends its whole `raw_buffer` on every flush
      // (agentic_loop/run.rs). So this REPLACES. Appending would double-render
      // whenever two flushes land before the paired persisted TextStreamed
      // resets the buffer, which `should_flush` makes routine. The `typeof`
      // guard keeps a payload with no text from appending "undefined".
      thread.streamingBuffer = event.text;
    }
    // Transient events are the thread's own active work, so updatedAt keeps the
    // drawer timestamp current during a long coding-agent session.
    // ChildrenCountChanged and CodingAgentDiffChanged are excluded as
    // out-of-band aggregate refreshes rather than fresh activity. Bumping
    // updatedAt for them would churn the drawer's "X ago", the aggregate
    // already carrying the thread's own unchanged last_activity.
    // Tick-only write, so it does not mark metaChanged.
    if (created && event.type !== 'ChildrenCountChanged' && event.type !== 'CodingAgentDiffChanged') {
      thread.meta.updatedAt = created;
    }
  }
  return { applied: true, metaChanged, clearedPendingUserMessage };
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
