import { EVENT_CLASSIFICATION } from '../../generated/thread-lifecycle';
import { instantMicros } from '../../utils/isoInstant';
import { eventWaitProjection } from './event-waits';
import { findQuestionAnswer, modeToInitiator } from './exchange';
import { isUserStoppedWait } from './thread-event-types';
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
  if (thread.pendingUserMessages.length === 0) {
    return filterRemovedQueuedExchanges(groupIntoExchangesCached(thread.events), thread.events);
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
    const seq = Number.MAX_SAFE_INTEGER - pendingCount + i;
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
  const base = groupIntoExchangesCached(thread.events);
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
  return filterRemovedQueuedExchanges(groupIntoExchanges(augmented), augmented);
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
    return !(ex.userEvent.type === 'MessageReceived' && ex.steps.length === 0 && id && removed.has(id));
  });
}

/** Event types that begin a new exchange in the timeline. Each agent pause is
 *  its own auditable boundary with an actor, never a step inside the prior
 *  agent response.
 *
 *  One boundary is decided by the EVENT rather than by its type and so is not
 *  in this set: see `isExchangeStartEvent`. */
const EXCHANGE_START_TYPES: ReadonlySet<string> = new Set([
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
    return exchanges.find(ex =>
      ex.userEvent.type === 'MessageReceived' && ex.userEvent._eventId === event.injected_message_id,
    ) ?? null;
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

/** A turn of a call: what Lucidos said out loud, or what the caller said when
 *  the talker answered it alone.
 *
 *  Neither is an exchange boundary in the ordinary case. A spoken turn lands
 *  inside whatever turn was running when it was said, which is where it
 *  happened. This names them for the one case where there is no such turn. */
export function isSpokenTurn(event: { type: string }): boolean {
  return event.type === 'SpokenReplyGenerated' || event.type === 'SpokenMessageReceived';
}

/** Read `tool_use_id` from a CodingAgentTool* event payload. Empty string in
 *  legacy DB rows from before the field existed — normalize to `undefined`. */
export function toolUseIdOf(event: { type: string }): string | undefined {
  const id = (event as { tool_use_id?: string }).tool_use_id;
  return id ? id : undefined;
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
}

function newFoldState(): GroupFoldState {
  return {
    exchanges: [],
    current: null,
    toolCallOwners: new Map(),
    chatToolCallOwners: new Map(),
    questionDividerOwners: new Map(),
    permissionDividerOwners: new Map(),
    gatedCalls: new Map(),
    reqIdRedirect: new Map(),
    resolvedReqIds: new Set(),
    abortReqIds: new Set(),
  };
}

export function groupIntoExchanges(events: Map<number, StoredEvent>): Exchange[] {
  return foldSorted(sortEventsChronologically(events)).exchanges;
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
 *  created (when both present) with seq as tiebreak, else seq. */
function compareSortKeys(
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
function rebuildIncrementalCache(events: Map<number, StoredEvent>): Exchange[] {
  const sorted = sortEventsChronologically(events);
  let cacheable = true;
  for (const { event } of sorted) {
    if (!event.created) {
      cacheable = false;
      break;
    }
  }
  const fold = foldSorted(sorted);
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
function groupIntoExchangesCached(events: Map<number, StoredEvent>): Exchange[] {
  const cache = incrementalCache.get(events);
  if (!cache) return rebuildIncrementalCache(events);
  if (!cache.cacheable) return groupIntoExchanges(events);
  if (events.size === cache.processedCount) return [...cache.fold.exchanges];
  if (events.size < cache.processedCount) return rebuildIncrementalCache(events);

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
      return groupIntoExchanges(events);
    }
    if (compareSortKeys(micros, seq, prevMicros, prevSeq) < 0) {
      // Out-of-order arrival (e.g. a refresh replay delivering a missed
      // event): its sorted position is in the middle, not the end.
      return rebuildIncrementalCache(events);
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
      return rebuildIncrementalCache(events);
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
function foldSorted(sorted: SequencedEvent[]): GroupFoldState {
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

  const state = newFoldState();
  for (const { seq, event } of sorted) {
    foldEvent(state, seq, event, legacySupersededAbortSeqs.has(seq), null);
  }
  markOvertakenQuestionDividers(state.exchanges);
  return state;
}

/** Re-anchor `exchange` to the end of the timeline, the position it should
 *  occupy now that the agent has engaged with it. Used by the two paths where
 *  an exchange was created earlier than it was engaged with: the mid-flight
 *  `UserPromptInjected` absorb and `reanchorResolvedDivider`. No-op when it is
 *  already last, the common case for both.
 *
 *  Position is not part of an Exchange's own state, so callers relying on
 *  position-derived props need no `touched` bump. The render pass recomputes
 *  `isLast` / `hasPriorActive` / `priorModel` / `priorEffort` per exchange and
 *  `chatExchangePropsEqual` compares each, so a reorder re-renders on its own. */
function moveExchangeToEnd(exchanges: Exchange[], exchange: Exchange): void {
  const idx = exchanges.indexOf(exchange);
  if (idx === -1 || idx === exchanges.length - 1) return;
  exchanges.splice(idx, 1);
  exchanges.push(exchange);
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
 *  make it `current`. Same move and reason as the mid-flight
 *  `UserPromptInjected` absorb above.
 *
 *  Gated on the divider being a `reqIdRedirect` target, which is exactly the
 *  in-process chat dividers whose continuation routes back here by request id.
 *  A CC `CodingAgentPermissionRequest` is never a redirect target, CC events
 *  not being request-id routed. Its continuation flows through `current` to the
 *  intervening boundary, so moving the card would strand it. No-op with no
 *  boundary between, the divider being already last and already `current`. */
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
  moveExchangeToEnd(state.exchanges, divider);
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
 *  exact because the chat agentic loop is sequential: one call at a time. */
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
        if (target.userEvent.type === 'UserQuestionAsked') {
          // A question dismissed, or replaced by a follow-up, already says so
          // on its own card. A standalone "Response canceled" panel under it
          // would be a second telling.
          const answered = findQuestionAnswer(target, target.userEvent.tool_use_id);
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
    const absorbTarget = findAbsorbTarget(current, exchanges, event);
    if (absorbTarget) {
      // A queued mid-flight message is ingested here, the UPI being the moment
      // the loop picked it up. Boundaries created while it sat in the queue
      // leave the optimistic MR panel positioned ABOVE them. The agent only
      // engages with the message now, so its panel and reply belong BELOW those
      // boundaries. Re-anchor to the ingestion point by moving it to the end.
      // No-op when it is already last, the common case.
      moveExchangeToEnd(exchanges, absorbTarget);
      absorbTarget.steps.push({ seq, event });
      touched?.add(absorbTarget);
      // It owns the turn again, so a handoff recorded while it sat in the queue
      // no longer holds. See `Exchange.continuationMoved`.
      absorbTarget.continuationMoved = false;
      current = absorbTarget;
      if (event.type === 'UserPromptInjected') {
        const absorbedReqId = requestEventIdOf(event);
        if (absorbedReqId) reqIdRedirect.set(absorbedReqId, absorbTarget);
      }
    } else if (isExchangeStartEvent(event) && !isLegacySupersededAbort) {
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
      // also absent from `advancesRedirect` below. It continues nothing, so
      // there is no continuation to redirect and no handoff to record.
      if (isUserStoppedWait(event)) {
        current = previousCurrent;
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
      // Three shapes reach this advance, all one thing: a turn INTERRUPTED by a
      // boundary that then resumes under its own, unchanged req_id. Without the
      // redirect, everything after the boundary routes back to the pre-boundary
      // exchange, which sits ABOVE the card, so the continuation renders first.
      //
      // 1. A chat `ask_user_question` or command-guard permission prompt, both
      //    in-process in the agentic loop. The gated tool call and its result
      //    still re-route to the MR exchange by tool_called_event_id, so only
      //    the genuine continuation moves below the card. CC's
      //    `CodingAgentPermissionRequest` is excluded, never being routed.
      // 2. `ChildThreadCompleted`, a sub-thread finishing mid response. The
      //    engine injects the summary as a ReentryFromEngine, minting no id.
      // 3. An unabsorbed `UserPromptInjected`, injected as a ReentryFromWait
      //    from a detached wait, which holds no turn of its own (ADR 0049).
      //
      // EXCEPTION for shapes 2 and 3: a turn parked at a divider STILL awaiting
      // the user keeps its redirect, so the reply stays with the card being
      // answered. A resolved divider gets no exception, see
      // `dividerStillAwaitsUser`. An IDLE wake is unaffected: the engine starts
      // a fresh turn anchored on the injection, which finds this exchange.
      const advancesRedirect =
        event.type === 'UserQuestionAsked'
        || event.type === 'CommandPermissionRequested'
        || event.type === 'McpPermissionRequested'
        || ((event.type === 'ChildThreadCompleted' || event.type === 'UserPromptInjected')
          && !!previousCurrent
          && !dividerStillAwaitsUser(previousCurrent));
      if (advancesRedirect && previousCurrent) {
        // The turn now belongs to `current`, so nothing else lands in
        // `previousCurrent`. A `Thinking` marker left pending there can never
        // resolve on its own events. Record the handoff so rendering finalizes
        // it. See `Exchange.continuationMoved`.
        previousCurrent.continuationMoved = true;
        touched?.add(previousCurrent);
        // Two writes, both needed. Move any redirect that pointed at the
        // previous current, covering a turn that kept an ANCESTOR's req_id.
        // Then map the previous current's OWN anchor id to `current`,
        // unconditionally. When `previousCurrent` itself opened a fresh turn,
        // its continuation streams under that card's own id. The moved entries
        // are then spurious leftovers, and only the second write routes the
        // real reply. A redundant entry is harmless: nothing routes by an
        // unused id.
        for (const [reqId, exchange] of reqIdRedirect.entries()) {
          if (exchange === previousCurrent) {
            reqIdRedirect.set(reqId, current);
          }
        }
        const anchorId = previousCurrent.userEvent._eventId;
        if (anchorId) reqIdRedirect.set(anchorId, current);
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
    } else if (isSpokenTurn(event) && !current) {
      // A call opens with a greeting, and it is said before anything has
      // started a turn. So there is no exchange for the row to land in, and
      // the `current` fallthrough below drops it silently. No audio is kept,
      // which makes a dropped spoken turn gone rather than merely unrendered:
      // a call where nobody delegates leaves a thread rendering nothing at all.
      //
      // Promoted to a boundary of its own, as the legacy prompt above is. It
      // takes NO steps: the words are in the event, and its initiator panel
      // draws them. Anything landing after it belongs to it as usual.
      current = { userEvent: event, userSeq: seq, steps: [] };
      exchanges.push(current);
      touched?.add(current);
    } else if (owner) {
      owner.steps.push({ seq, event });
      touched?.add(owner);
      if (event.type === 'ToolCalled' && event._eventId) {
        chatToolCallOwners.set(event._eventId, owner);
      }
    } else if (current) {
      current.steps.push({ seq, event });
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
