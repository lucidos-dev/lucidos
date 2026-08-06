import { EVENT_CLASSIFICATION } from '../../generated/thread-lifecycle';
import { eventWaitProjection } from './event-waits';
import { findQuestionAnswer, modeToInitiator } from './exchange';
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
 *  frame. The pending-message path bypasses the cache (it folds an augmented
 *  COPY of the events map, so the cache never sees synthetic seqs). */
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

  // Fast path: when every pending message sorts strictly after the last folded
  // real event — ALWAYS true for chat (synthetic seqs are MAX_SAFE_INTEGER-based
  // and chat synthetics carry no `created`, so the sort falls to seq) and the
  // normal case for CC (pending.created is "now", after every persisted event) —
  // the augmented fold is exactly the cached real-event fold with one fresh
  // trailing exchange per pending message. Reuse the incremental cache and append
  // instead of copying the whole events Map + re-sorting + re-folding the entire
  // history on EVERY send and EVERY streamed token while the optimistic pending
  // row is live (the "sending lags on a big thread" cost). Checking only the
  // earliest pending (smallest seq, earliest send) suffices: later pending
  // messages have strictly larger seqs and same-or-later timestamps.
  const base = groupIntoExchangesCached(thread.events);
  const cache = incrementalCache.get(thread.events);
  const first = synthetic[0];
  const canAppendTrailing =
    !!cache &&
    cache.cacheable &&
    compareSortKeys(first.event.created, first.seq, cache.lastCreated, cache.lastSeq) >= 0;

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

/** Event types that begin a new exchange in the timeline. Includes user-initiated
 *  events (MessageReceived, UserPromptInjected), system-initiated events that
 *  spawn a fresh round of work (engine restart, auto-hardening, auto-merge),
 *  the abort/resume boundary pair, change lifecycle events
 *  (apply/discard/revert/fail), the ActionRequired family
 *  (UserQuestionAsked, CodingAgentPermissionRequest, CredentialRequested,
 *  McpConsentRequested) — each agent pause is its own auditable boundary with
 *  an actor, not a step inside the prior agent response — and ChildThreadCompleted
 *  where a sidequest result lands in the parent as a rich card. */
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

export function isExchangeStartEvent(type: string): boolean {
  return EXCHANGE_START_TYPES.has(type);
}

/** True when `exchange` is a divider still PARKED awaiting a user action — its
 *  resolution (answer / grant) hasn't landed as a step yet.
 *
 *  A parked divider owns its own post-resolution continuation (the resolution
 *  and the agent's reply route back to it by id), so a `ChildThreadCompleted`
 *  landing while it waits must NOT steal the request-id redirect away — the
 *  reply belongs with the card the user is answering, not below an unrelated
 *  child completion. Ordering is handled the other way round: when the
 *  resolution lands, `reanchorResolvedDivider` moves the DIVIDER below that
 *  boundary, so the reply stays with its card AND still reads last.
 *
 *  The check is on the divider's STATE, not just its type: once resolved, the
 *  turn is an ordinary in-flight response again and a child completion must
 *  advance the redirect like any other (otherwise the post-completion work
 *  routes back up into the answered card while the stepless completion card
 *  sits last spinning 'Requesting' — the same stuck-looking thread, reached by
 *  the child finishing AFTER the answer instead of before it).
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
 *  filter, such an event arriving after a boundary started a new (still-empty)
 *  exchange leaks into that exchange's steps via the `current.steps.push`
 *  fallthrough — breaking the absorbed-UPI single-step shape that
 *  exchangeStatus relies on to short-circuit to 'done', and on a CC thread
 *  flipping a trailing child-completion card to a phantom
 *  'coding-agent-working' (real thread 276f5580: a background `WorktreeCleaned`
 *  landed in the `ChildThreadCompleted` exchange an hour after the thread
 *  idled, so the UI showed "Working" forever — persisting across reloads since
 *  grouping is deterministic from the event history).
 *
 *  Two sources, unioned: every `Thread*` metadata event — derived from
 *  EVENT_CLASSIFICATION so a new one added in Rust is picked up without an edit
 *  (ThreadArchived is excluded automatically: the contract classifies it
 *  terminal, not metadata) — plus the non-`Thread`-prefixed pure-bookkeeping
 *  events that also render nothing and must never count as a step. Only add an
 *  event to the explicit list if it has NO render case in exchange-render.ts;
 *  metadata events that DO render a panel (CodingAgentSettingsChanged,
 *  BackgroundBash*) must stay OUT of this set. */
const NON_EXCHANGE_METADATA_EVENTS: ReadonlySet<string> = new Set([
  ...Object.entries(EVENT_CLASSIFICATION)
    .filter(([evt, cls]) => cls === 'metadata' && evt.startsWith('Thread'))
    .map(([evt]) => evt),
  'QueuedMessageRemoved',
  // Background worktree-cleanup bookkeeping — emitted as EventClass::Metadata
  // by worktree_cleanup.rs, but not `Thread`-prefixed and with no render case.
  'WorktreeCleaned',
]);

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
 *  1. Engine resume note — UPI emitted by chat/rerun.rs right after ContinuationStarted
 *     belongs as a step under the resume initiator. A Human-mode UPI in the same
 *     position is a real correction and stays its own exchange.
 *  2. Mid-flight injection — chat fast-path emits MessageReceived first (with the
 *     client UUID) then sends the injection; the agentic loop later emits UPI
 *     carrying that UUID in `injected_message_id`. Without absorption the user
 *     sees a duplicate "Auto-prompt sent" panel below their own message.
 *
 *  Returns null when the event is not absorbable, or when an injection's partner
 *  is missing — the caller falls back to starting a new exchange so the UPI still
 *  renders rather than vanishing. */
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
 *  originating exchange. Excludes `CodingAgent*` because CC reuses one session
 *  across many follow-ups and never re-anchors the field — routing CC events
 *  by request id would push every follow-up's work back into the first MR.
 *
 *  Response* events are dual-purpose (chat AND CC emit them). For CC they
 *  carry the session's persistent req_id (same reason CodingAgent* events do),
 *  so they're filtered out by `shouldRouteByRequestId` when channel is CC.
 *
 *  Every event the chat agentic loop stamps with `meta.request_event_id` must
 *  appear here — anything missing falls through to the `current` pointer in
 *  `groupIntoExchanges` and silently leaks into a follow-up MR's empty
 *  exchange when the loop's events arrive after the follow-up was emitted.
 *  That leak flips `exchangeStatus` to 'aborted' for the follow-up via the
 *  `threadIdle && hasSteps && !isComplete` branch — observed on real thread
 *  9b5a05aa (a stray ContextAssembled) and again on ad178d6a where the current
 *  `ContextCaptured` landed in the empty follow-up exchange, flashing 'Aborted'
 *  before the follow-up's own events arrived and flipped it to 'Working'.
 *  `ContextCaptured` is the live event; `ContextAssembled` /
 *  `ContextTokensMeasured` are its retired predecessors, kept here so legacy
 *  DB rows route the same way (the render switch still handles all three). */
const REQUEST_ID_ROUTED_TYPES: ReadonlySet<string> = new Set([
  'ThoughtStreamed',
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
  // request_event_id — the checkpoint is emitted mid-turn; the revert is
  // emitted later (at undo time) but is stamped with the ORIGINAL turn's
  // request_event_id, so routing by it lands the revert back in the
  // checkpoint's exchange (the card then renders reverted, live and on reload).
  // Without this it would leak into the latest exchange via the `current`
  // pointer and the card would never flip.
  'CommandCheckpointed',
  'CommandCheckpointReverted',
]);

/** Skip req_id routing for Response* terminals AND context snapshots when their
 *  channel is CC: the session's persistent meta carries the original MR's
 *  req_id for the entire session. Routing Response* back by id would push a
 *  mid-flight cancel/abort to the original exchange instead of terminating the
 *  active follow-up. Routing context snapshots (ContextCaptured and its retired
 *  predecessors) back by id pulls a post-apply continuation's snapshots out
 *  from between the change banners and up to the first message — the same
 *  reason CodingAgent* events are excluded from the routed set entirely (real
 *  thread 76b4ee76: a CC session applied a change, kept working under the same
 *  req_id, and its second turn vanished from the timeline). Keep them
 *  chronological (folded into `current`) on CC threads. */
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

/** Read `request_event_id` from any event payload. The field is added to the
 *  wire payload by Rust's `EventMeta::apply()` regardless of the event type,
 *  so the cast is honest about what arrives at runtime. */
function requestEventIdOf(event: { type: string }): string | undefined {
  return (event as { request_event_id?: string }).request_event_id;
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
  return [...events.entries()]
    .sort(([aSeq, aEvt], [bSeq, bEvt]) =>
      compareSortKeys(aEvt.created, aSeq, bEvt.created ?? null, bSeq),
    )
    .map(([seq, event]) => ({ seq, event }));
}

/** Event types whose appearance as a step of a `UserQuestionAsked` divider
 *  exchange (without a matching `UserQuestionAnswered`) means the agent has
 *  raced past the question and the QuestionCard's buttons should disable.
 *  Mirrors the Rust `ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES` constant
 *  (see `crates/lucidos-engine/src/engine/thread_events/event_impl.rs`).
 *
 *  Keep both lists in sync. One question, four gates now hang off them:
 *  server-side, the engine constant gates typed-text routing (a follow-up
 *  becomes a fresh message instead of a `FreeText` answer) and, unioned with
 *  three extras, the restart preserve guard (`unanswered_question_exists_sql`);
 *  client-side, this set gates the click-button affordance AND whether
 *  `exchangeStatus` may still read "Needs your answer" (via the
 *  `questionOvertaken` flag it sets below). They must agree: a card whose
 *  buttons are dead must not be labelled answerable, and a thread whose card is
 *  dead must not be preserved as a resumable checkpoint. */
const QUESTION_OVERTAKEN_STEP_TYPES: ReadonlySet<string> = new Set([
  // Terminal (both agents)
  'ResponseAborted',
  'ResponseCanceled',
  'ResponseFailed',
  'CodingAgentIdled',
  // CC progression — the parallel-tool-call race the dev thread hit
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

/** Per-exchange half of `markOvertakenQuestionDividers`. The flag depends
 *  only on the exchange's OWN steps, so the incremental path re-runs this on
 *  exactly the exchanges an appended event touched — full-pass equivalent
 *  without walking every exchange per frame. */
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

/** Resumable fold state behind `groupIntoExchanges` / the incremental cache.
 *  Everything the per-event walk reads or writes lives here so the fold can
 *  stop after any event and continue later with appended events — the basis
 *  of the per-thread memoization in `computeExchanges` that keeps a
 *  streaming token from re-sorting and re-walking the whole history every
 *  frame. */
interface GroupFoldState {
  exchanges: Exchange[];
  current: Exchange | null;
  // tool_use_id → exchange that owns the matching CodingAgentToolCalled step.
  // Populated as calls are appended (always via the default `current.steps.push`
  // branch — CodingAgent* events aren't absorbed or request-id routed) and
  // queried when a CodingAgentToolResult lands so we can re-route it to the
  // call's exchange even if a permission request boundary intervened.
  toolCallOwners: Map<string, Exchange>;
  // Chat ToolCalled.event_id → owner exchange. The chat agentic loop stamps
  // `tool_called_event_id` on every live `ToolResult` (and the post-restart
  // recovery sweep does the same on synthetic backfills), so this map is the
  // primary routing path for chat ToolResults. Required because an
  // `ask_user_question` call is followed by a `UserQuestionAsked` divider
  // exchange: the request-id redirect moves to the divider, and the live
  // post-answer `ToolResult` (sharing the originating MR's req_id) would
  // otherwise follow the redirect into the divider — leaving the original
  // call's "Executing …" spinner pending forever. Legacy DB rows without the
  // field fall through to the request-id / `current` routing.
  chatToolCallOwners: Map<string, Exchange>;
  // tool_use_id → the UserQuestionAsked divider exchange that owns it, and
  // request_id → the CodingAgentPermissionRequest divider. A UserQuestionAnswered
  // / CodingAgentPermissionResolved is neither request-id routed nor an exchange
  // boundary, so by default it follows the `current` pointer. When a boundary
  // (e.g. ChildThreadCompleted from a spawned CC sub-thread finishing while the
  // question was on screen) lands between the divider and its answer, `current`
  // is that boundary — the answer strands there and the divider stays stuck on
  // 'awaiting-answer' forever (real thread 3e54cacb). Route the resolution back
  // to its divider by id, mirroring the CodingAgentToolResult re-routing.
  questionDividerOwners: Map<string, Exchange>;
  permissionDividerOwners: Map<string, Exchange>;
  // request_event_id → redirect target exchange. Set when a UPI is absorbed
  // mid-flight: the loop emits the UPI when it actually ingests the queued
  // follow-up, so every event after that point is part of the answer to the
  // absorbed prompt — not the original request. Without redirecting, the
  // post-injection tools and the final ResponseGenerated all stay in the
  // original exchange and the follow-up panel renders as an empty stub.
  reqIdRedirect: Map<string, Exchange>;
  /** request_event_ids of every ResponseGenerated / ResponseFailed folded so
   *  far. The incremental path uses it to classify a late-arriving abort as
   *  legacy-superseded (terminal-before-abort direction). */
  resolvedReqIds: Set<string>;
  /** request_event_ids of every ResponseAborted folded so far. A terminal
   *  arriving later with a matching id retro-classifies that abort
   *  (abort-before-terminal direction) — the incremental path detects it and
   *  falls back to a full rebuild. */
  abortReqIds: Set<string>;
  /** request_event_id of the most recent request-id-routed chat event — i.e.
   *  the active chat turn's req_id. The divider redirect bootstrap reads it to
   *  redirect the turn's post-answer continuation to the divider, instead of
   *  trusting `previousCurrent` (which can be an UNINGESTED queued
   *  MessageReceived that intervened between the turn and the question — real
   *  thread 194474de). Invariant it relies on: a chat `ask_user_question` /
   *  permission prompt is always preceded in the same turn by its
   *  request-id-routed tool call, so at divider-creation time this holds the
   *  turn's real req_id. Undefined until the first routed chat event (e.g. a
   *  pure CC thread never sets it, so the chat-divider redirect is a no-op
   *  there — CC events aren't request-id routed). */
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
    reqIdRedirect: new Map(),
    resolvedReqIds: new Set(),
    abortReqIds: new Set(),
  };
}

export function groupIntoExchanges(events: Map<number, StoredEvent>): Exchange[] {
  return foldSorted(sortEventsChronologically(events)).exchanges;
}

/** Incremental memo entry for one thread's events map. Valid only while the
 *  events arrive append-only in sort order — the validation pass in
 *  `groupIntoExchangesCached` falls back to a full rebuild the moment that
 *  contract breaks. Keyed by the Map OBJECT in a WeakMap: handleEvent never
 *  re-sets an existing seq, and the wholesale-rebuild paths
 *  (`rebuildCorruptedThreadEvents`) replace the Map, which naturally misses
 *  here and discards the stale entry. */
interface IncrementalCache {
  fold: GroupFoldState;
  /** Entries folded so far. New events are exactly the iteration-order
   *  suffix past this count (Map preserves insertion order). */
  processedCount: number;
  /** Sort key (created, seq) of the last folded event — appended events must
   *  not sort before it. */
  lastCreated: string | null;
  lastSeq: number;
  /** Flipped false when an event lacks `created` (legacy rows): the sort
   *  comparator is no longer a total order we can do append-checks against,
   *  so this map full-computes on every call — exactly the pre-cache
   *  behavior. */
  cacheable: boolean;
}

const incrementalCache = new WeakMap<Map<number, StoredEvent>, IncrementalCache>();

/** The sort comparator of `sortEventsChronologically`, as a key compare:
 *  created (when both present) with seq as tiebreak, else seq. */
function compareSortKeys(
  aCreated: string | undefined,
  aSeq: number,
  bCreated: string | null,
  bSeq: number,
): number {
  // ISO-8601 UTC timestamps (fixed-width, Zulu) sort identically under a plain
  // lexical `<`/`>` compare, which is ~10–50× cheaper than the Intl collator
  // that `String.localeCompare` spins up — and this comparator runs O(n log n)
  // times per fold (every thread open + every full recompute). Equal timestamps
  // (and the legacy missing-`created` case) fall through to the `seq` tiebreak.
  if (aCreated && bCreated && aCreated !== bCreated) {
    return aCreated < bCreated ? -1 : 1;
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
    lastCreated: last?.event.created ?? null,
    lastSeq: last?.seq ?? Number.MIN_SAFE_INTEGER,
    cacheable,
  });
  return [...fold.exchanges];
}

/** Memoized `groupIntoExchanges`. Result is always deep-equal to the
 *  from-scratch pass (pinned by incremental-grouping.test.ts); the returned
 *  array is a fresh copy each call (so signal subscribers fire on identity)
 *  while the Exchange objects inside stay identity-stable across appends
 *  (so per-exchange memoization holds). */
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
  appended.sort((a, b) => compareSortKeys(a.event.created, a.seq, b.event.created ?? null, b.seq));

  // Validation pass — every appended event must keep the fold resumable.
  // `batchAbortReqIds` covers the abort-then-terminal pair arriving INSIDE
  // one batch: the abort isn't in `cache.fold.abortReqIds` yet (that set is
  // only fed by foldEvent), so checking the cache set alone would miss the
  // retro-classification and fold the abort as a real boundary where the
  // from-scratch pass marks it superseded.
  let prevCreated = cache.lastCreated;
  let prevSeq = cache.lastSeq;
  const batchAbortReqIds = new Set<string>();
  for (const { seq, event } of appended) {
    if (!event.created) {
      // Legacy row without a timestamp — give up on caching this map.
      cache.cacheable = false;
      return groupIntoExchanges(events);
    }
    if (compareSortKeys(event.created, seq, prevCreated, prevSeq) < 0) {
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
    prevCreated = event.created;
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
    // In-place mutation is invisible to identity-based memo comparison —
    // bump the captured-at-render revision so memoized components re-render.
    exchange.revision = (exchange.revision ?? 0) + 1;
  }
  cache.processedCount = events.size;
  cache.lastCreated = prevCreated;
  cache.lastSeq = prevSeq;
  return [...cache.fold.exchanges];
}

/** One-shot fold over an already-sorted event list. Runs the legacy
 *  rerun-in-place pre-pass, folds every event, and applies the
 *  question-divider marking — the from-scratch path `groupIntoExchanges` and
 *  the cache rebuild both go through here. */
function foldSorted(sorted: SequencedEvent[]): GroupFoldState {
  // Legacy rerun-in-place: when a ResponseAborted shares request_event_id
  // with a later ResponseGenerated/ResponseFailed in the same thread, the
  // rerun re-used the original exchange (pre-Phase-5.3 behavior). Don't split
  // at those aborts — supersededAbortIndices in exchangeStatus deflates the
  // verdict to the later success.
  //
  // Single forward pass: record request_event_ids of every later resolving
  // terminal first, then mark aborts that match. O(N) instead of O(N²).
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

/** Re-anchor `exchange` to the end of the timeline — the position it should
 *  occupy now that the agent has actually engaged with it. Used by the two
 *  "the exchange was created earlier than it was engaged with" paths: the
 *  mid-flight `UserPromptInjected` absorb and `reanchorResolvedDivider`. No-op
 *  when it is already last, which is the common case for both.
 *
 *  Position is not part of an Exchange's own state, so callers relying on
 *  position-derived props must not need a `touched` bump: the render pass
 *  recomputes `isLast` / `hasPriorActive` / `priorModel` / `priorEffort` for
 *  every exchange and `chatExchangePropsEqual` compares each of them, so a
 *  reorder re-renders the affected panels on its own. */
function moveExchangeToEnd(exchanges: Exchange[], exchange: Exchange): void {
  const idx = exchanges.indexOf(exchange);
  if (idx === -1 || idx === exchanges.length - 1) return;
  exchanges.splice(idx, 1);
  exchanges.push(exchange);
}

/** A divider just received its resolution (answer / permission grant). If a
 *  boundary exchange was appended while the card sat on screen — the shape is
 *  a spawned sub-thread finishing and emitting `ChildThreadCompleted` between
 *  the question and the answer (real thread 8144b43e) — the divider is no
 *  longer last, yet it still OWNS the turn's continuation via `reqIdRedirect`.
 *  Left in place, every post-answer step renders ABOVE that intervening card:
 *  the user sees the agent's live work in the middle of the timeline while the
 *  bottom of the thread is a stepless completion card frozen on 'Requesting',
 *  which reads as a stuck agent. (Both halves are one bug — `exchangeStatus`
 *  keeps a stepless `ChildThreadCompleted` spinning precisely while it is
 *  `isLast` on a running thread, because it normally expects the continuation
 *  to land in it.)
 *
 *  So re-anchor the divider to its RESOLUTION point: move it to the end and
 *  make it `current`. Same move, same reason as the mid-flight `UserPromptInjected`
 *  absorb above — the exchange is repositioned to where the agent actually
 *  engages with it, so its forthcoming reply renders below the boundaries that
 *  landed while it waited (and the superseded card falls to `!isLast` → 'done').
 *
 *  Gated on the divider being a `reqIdRedirect` target, which is exactly the
 *  in-process chat dividers (`UserQuestionAsked`, `CommandPermissionRequested`,
 *  `McpPermissionRequested`) whose continuation routes back here by request id.
 *  A CC `CodingAgentPermissionRequest` is never a redirect target — CC events
 *  aren't request-id routed, so its continuation flows through `current` to the
 *  intervening boundary and moving the card would strand it. No-op in the
 *  common case: with no boundary between, the divider is already last and
 *  already `current`. */
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

/** Fold one event into the state. `isLegacySupersededAbort` is decided by
 *  the caller (the one-shot path precomputes it positionally-blind over the
 *  whole array; the incremental path derives the terminal-before-abort
 *  direction from `resolvedReqIds`-so-far and full-rebuilds on the other
 *  direction). `touched` collects every exchange this event mutated or
 *  created so the incremental path can re-run the question-divider marking
 *  on exactly those. */
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

    const reqId = shouldRouteByRequestId(event) ? requestEventIdOf(event) : undefined;
    // Remember the active chat turn's req_id (see `lastChatTurnReqId`) so the
    // divider redirect bootstrap below can target it directly, rather than
    // inferring it from `previousCurrent` (wrong when a queued follow-up
    // intervened).
    if (reqId) state.lastChatTurnReqId = reqId;
    const owner = reqId
      ? (reqIdRedirect.get(reqId) ?? findExchangeByAnchorId(exchanges, reqId))
      : null;

    // ResponseAborted is dual-purpose: it terminates the originating exchange
    // (so the partial-response panel reads 'Aborted ⚠') AND opens a new
    // boundary exchange whose userEvent is the abort itself, rendered as the
    // AbortPanel. The boundary always sits chronologically last so the panel
    // appears below any newer MessageReceived in the timeline.
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
    // ResponseCanceled mirrors the abort dual-purpose pattern: keep the
    // cancel as a step on the originating exchange (so its response panel
    // reads 'Canceled ✕') AND open a new boundary exchange so a separate
    // 'Response canceled' panel renders below the truncated reply.
    // Skip the boundary only when the question card already carries the
    // cancel attribution via its Cancel-as-picked button — i.e. the question
    // resolved as Canceled. Any non-Canceled resolution leaves the picked
    // option as the visible state, so the boundary panel must render.
    //
    // A `superseded_by_followup` cancel (Codex mid-turn follow-up redirect) is
    // the exception: the user steered, not Stopped, so it renders neutrally like
    // the chat/CC follow-up. Keep it as a step on the originating exchange (so
    // step resolution / model extraction still see a terminator) but do NOT open
    // a boundary — there must be no standalone 'Response canceled' panel.
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
          const answered = findQuestionAnswer(target, target.userEvent.tool_use_id);
          if (answered?.answer.kind === 'Canceled') {
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
    // Chat ToolResult routing. The chat agentic loop stamps
    // `tool_called_event_id` on every live emit (and the post-restart
    // recovery sweep does the same on synthetic backfills), so this branch
    // is the primary routing path — see the `chatToolCallOwners`
    // declaration above for the full rationale (in short: an
    // `ask_user_question` call's post-answer ToolResult would otherwise
    // follow the req_id redirect into the question divider and the original
    // call's "Executing …" spinner would stay pending forever). The
    // tracking map only knows about ToolCalled events seen so far in the
    // walk; a synthetic ToolResult arriving before its ToolCalled would
    // miss this branch, which is fine — neither the live loop (call before
    // result by construction) nor recovery (emits AFTER reading the orphan
    // ToolCalled out of the DB) produces that ordering. Legacy DB rows
    // without the field fall through to the request_id / `current` routing.
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
      // A queued mid-flight message is ingested here (the UPI is the moment the
      // loop actually picked it up). If boundary exchanges — a question card,
      // another message — were created while it sat in the queue, the optimistic
      // MR panel is now positioned ABOVE them, but the agent only engages with
      // this message now, so its panel + forthcoming reply belong BELOW those
      // boundaries. Re-anchor it to the ingestion point by moving it to the end,
      // otherwise the reply renders above the intervening question card (real
      // thread ab70366f). No-op when it's already last (the common case: no
      // boundary intervened), which keeps the simple absorb paths unchanged.
      moveExchangeToEnd(exchanges, absorbTarget);
      absorbTarget.steps.push({ seq, event });
      touched?.add(absorbTarget);
      // It owns the turn again — a handoff recorded while it sat in the queue
      // (a divider raised with this uningested message as `previousCurrent`)
      // no longer holds. See `Exchange.continuationMoved`.
      absorbTarget.continuationMoved = false;
      current = absorbTarget;
      if (event.type === 'UserPromptInjected') {
        const absorbedReqId = requestEventIdOf(event);
        if (absorbedReqId) reqIdRedirect.set(absorbedReqId, absorbTarget);
      }
    } else if (isExchangeStartEvent(event.type) && !isLegacySupersededAbort) {
      const previousCurrent = current;
      current = { userEvent: event, userSeq: seq, steps: [] };
      exchanges.push(current);
      touched?.add(current);
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
      }
      // Chat agent's `ask_user_question` AND the chat command guard's
      // `IrreversibleDanger` permission prompt both run in-process inside the
      // agentic loop: every event in the turn carries the originating MR's
      // req_id, and the turn RESUMES (more thinking + the reply) after the
      // user answers / grants. Without a redirect here, those post-resolution
      // TextStreamed / ResponseGenerated / ToolResult / ThoughtStreamed route
      // back to the MR exchange via shouldRouteByRequestId and the agent's
      // reply renders ABOVE the question / permission card instead of below it
      // (the gated tool call + its result still re-route to the MR exchange by
      // tool_called_event_id, so the "Running …" step stays merged above the
      // card — only the genuine continuation moves below). Move any existing
      // redirect that targets the previous current to the new exchange;
      // bootstrap from the previous current's anchor _eventId (the turn's
      // req_id for an MR) when no redirect existed yet — the first interruption
      // of this turn. CC's `CodingAgentPermissionRequest` is deliberately
      // excluded: CC events are never request-id routed, so it needs no redirect
      // (its resolution + resumption route by `current` / tool_use_id instead).
      //
      // `ChildThreadCompleted` is the same shape from the other direction: when
      // a spawned sub-thread finishes WHILE the parent is mid-response, the
      // engine injects the child's summary into the running loop as a
      // WakeFromChild (no new req_id — the turn keeps streaming under the
      // originating MR's id) and emits the completion card as a boundary. The
      // post-completion continuation must group UNDER the card; without
      // advancing the redirect it routes back to the pre-completion exchange,
      // which sits ABOVE the card, so the continued "Thinking / Running …"
      // renders before the card it follows (real thread 4d193da8). EXCEPTION:
      // when the turn is parked at a question / permission divider that is
      // STILL awaiting the user (real thread 3e54cacb), that divider owns its
      // own post-answer continuation — leave the redirect on it so the reply
      // stays with the card the user is answering, not below an unrelated child
      // completion. The divider is then moved BELOW the card when the answer
      // lands (`reanchorResolvedDivider`), which is what keeps the continuation
      // rendering last. An ALREADY-resolved divider gets no exception — see
      // `dividerStillAwaitsUser`.
      const advancesRedirect =
        event.type === 'UserQuestionAsked'
        || event.type === 'CommandPermissionRequested'
        || event.type === 'McpPermissionRequested'
        || (event.type === 'ChildThreadCompleted'
          && !!previousCurrent
          && !dividerStillAwaitsUser(previousCurrent));
      if (advancesRedirect && previousCurrent) {
        // The turn now belongs to `current`, so nothing else will land in
        // `previousCurrent` — a `Thinking` marker left pending there can never
        // resolve on its own events. Record the handoff so rendering finalizes
        // it instead of shimmering an old step half a screen above the work
        // (real thread 8144b43e). See `Exchange.continuationMoved`.
        previousCurrent.continuationMoved = true;
        touched?.add(previousCurrent);
        // Move any redirect that pointed at the previous current (the turn kept
        // an ANCESTOR's req_id — e.g. a mid-response child completion keeps the
        // originating MR's id) AND, unconditionally, map the previous current's
        // OWN anchor id → current. Both are needed: when `previousCurrent`
        // itself opened a fresh turn (a ChildThreadCompleted whose parent turn
        // had already finished — its continuation streams under the card's OWN
        // id), the moved entries are SPURIOUS leftovers and the real reply
        // routes by `previousCurrent`'s id. Gating the bootstrap on "no entry
        // moved" dropped that mapping, so the post-answer reply misrouted back
        // into the card (rendered above the question) and the divider flashed
        // 'aborted' (real thread 2e98b44a). Setting both keeps the
        // keep-ancestor-id and fresh-turn cases correct; a redundant entry is
        // harmless (nothing routes by an unused id).
        for (const [reqId, exchange] of reqIdRedirect.entries()) {
          if (exchange === previousCurrent) {
            reqIdRedirect.set(reqId, current);
          }
        }
        const anchorId = previousCurrent.userEvent._eventId;
        if (anchorId) reqIdRedirect.set(anchorId, current);
      }
      // For a chat in-process divider, the post-answer continuation carries the
      // ACTIVE turn's req_id — which is `previousCurrent`'s anchor only when
      // `previousCurrent` IS that turn's exchange. When an UNINGESTED queued
      // follow-up MessageReceived intervened, `previousCurrent` is the queued MR
      // and the bootstrap above anchors on the WRONG id, so the reply +
      // ResponseGenerated route back to the original message exchange and the
      // divider strands terminal-less → a persistent 'aborted' (real thread
      // 194474de). Redirect the turn's real req_id (tracked as
      // `lastChatTurnReqId`, set by the divider-raising tool call) to the divider
      // directly. Additive and idempotent in the common no-queue case: there
      // `lastChatTurnReqId` already equals `previousCurrent`'s anchor. CC
      // dividers never set `lastChatTurnReqId` (CC events aren't request-id
      // routed), so this is a no-op for them. ChildThreadCompleted is excluded —
      // its continuation routing is governed by the `previousCurrent` logic above.
      if (
        (event.type === 'UserQuestionAsked'
          || event.type === 'CommandPermissionRequested'
          || event.type === 'McpPermissionRequested')
        && state.lastChatTurnReqId
      ) {
        // Whoever held that req_id just lost the turn — normally
        // `previousCurrent` (marked above), but the queued-follow-up shape has
        // them be different exchanges, and the handoff mark belongs on the one
        // the redirect is actually moved OFF. See `Exchange.continuationMoved`.
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
      // New data emits both MessageReceived and CodingAgentUserMessageSent for the same
      // user message — skip creating a duplicate exchange if one already exists.
      if (current && current.userEvent.type === 'MessageReceived' && current.steps.length === 0) {
        // MessageReceived already started this exchange — skip the duplicate
        return;
      }
      const text = (event as { text: string }).text;
      current = { userEvent: { type: 'MessageReceived', text } as StoredEvent, userSeq: seq, steps: [] };
      exchanges.push(current);
      touched?.add(current);
    } else if (event.type === 'CodingAgentPromptSent' && !current) {
      // Legacy engine-spawned CC threads (merge-conflict, hardening) created
      // before MergeConflictDetected/MissingHardeningDetected boundary events
      // existed emit a bare CodingAgentPromptSent as the first content event.
      // Promote it to a synthetic boundary so the panel renders — without this
      // every following step is dropped and the thread shows the "Messages
      // could not be displayed" empty state. Modern threads always have a
      // proper boundary first, so `current` is non-null and we fall through
      // to the step branch below.
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
    // CONTRACT: `thread.events` is append-only with deduped seqs — the
    // `has(seq)` guard above is load-bearing. The incremental grouping
    // cache (`groupIntoExchangesCached`) keys its memo on this Map object
    // and detects new work by size + insertion-order suffix; an in-place
    // re-set of an existing seq, a delete(), or a clear() would serve
    // STALE exchanges with no failure signal. To rewrite a thread's
    // events wholesale, replace the Map object instead (see
    // `rebuildCorruptedThreadEvents`) — a new Map misses the WeakMap and
    // triggers a clean rebuild.
    thread.events.set(seq, stored);
    thread.streamingBuffer = '';
    // Update updatedAt only for events that the backend updates last_activity for.
    // Must stay in sync with update_thread_projection() in event_bus.rs.
    // Tick-only write — does not mark metaChanged (see `applyAggregateToMeta`).
    if (created && updatesLastActivity(event.type)) thread.meta.updatedAt = created;
    // When a real MessageReceived event arrives from the backend,
    // remove the matching optimistic pending message by event_id (UUID).

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
    // FreeText answers don't emit a MessageReceived (the backend routes typed
    // text straight to UserQuestionAnswered), so the optimistic pending message
    // added by sendMessage() must be cleared here too. Match by text — the
    // backend forwards user input verbatim. A non-match indicates drift; let
    // the safety timer clean it up rather than silently shifting the wrong one.
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
      metaChanged = true;
    }
    // Project the thread's live *event waits* into meta, the same way and for
    // the same reason as the Todo list above: the subscription indicator is
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
      // (agentic_loop/run.rs). So this REPLACES. Appending double-renders
      // whenever two flushes land before the paired persisted TextStreamed
      // resets the buffer, which `should_flush` makes routine (a completed
      // heading line and a following blank line are two flushes in a row).
      // The `typeof` guard also keeps a payload with no text from appending the
      // literal "undefined", which the previous `+=` did.
      thread.streamingBuffer = event.text;
    }
    // Transient events (streaming text, tool calls) represent the thread's
    // own active work — update updatedAt so the drawer timestamp stays
    // current during long-running Claude Code sessions. ChildrenCountChanged
    // and CodingAgentDiffChanged are excluded because they are out-of-band
    // aggregate refreshes, not fresh activity by the receiving thread. Bumping
    // updatedAt to broadcast NOW() would churn the drawer's "X ago" timestamp
    // even though the aggregate already carries the thread's own unchanged
    // last_activity for applyAggregateToMeta to overlay.
    // Tick-only write — does not mark metaChanged.
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
