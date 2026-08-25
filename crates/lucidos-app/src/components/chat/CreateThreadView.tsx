import type { VNode } from 'preact';
import { useRef, useEffect, useLayoutEffect, useState } from 'preact/hooks';
import {
  activeExchanges,
  activeStreamingBuffer,
  threadsLoaded,
  focusedThreadId,
  threadMap,
  isRenderedThreadIdle,
  cancelingThreadIds,
  removingQueuedMessageIds,
  queuedMessageRemovalKey,
  promptAnimating,
} from '../../store/store';
import { welcomeSuggestionsDismissed } from '../../store/actions/preferences';
import { awayFromBottom, notAtTop, scrollToBottom, scrollToTop, setActiveScrollElement, getActiveScrollElement, isElementVisible, makeScrollObservers, honourAnchoredMutation, isOtherNavigationScroll, markAnchorScroll, followIsCarrying } from './scrollState';
import { ChatExchange } from './ChatExchange';
import { ChevronUpIcon, ChevronDownIcon } from '../shared/icons';
import { WelcomeMessage } from './WelcomeMessage';
import type { Exchange } from '../../store/thread-events';
import { exchangeStatus as getExchangeStatus, exchangeResponseModel, exchangeReasoningEffort, exchangeKey, continuableAbortIndex, queuedFollowupRun, isChangeLifecycleEvent } from '../../store/thread-events';
import { isActive as isStatusActive } from '../../store/exchange-status';
import { forceIOSRepaint, SCROLLER_PINNED_ATTR } from '../../utils/iosRepaint';

/** First line of a change's description and its file count, keyed by change_id.
 *  Harvested from the `ChangeProposed` events riding a thread's coding-agent
 *  turns as non-rendered steps. A later lifecycle card for the same change_id
 *  is a SEPARATE exchange carrying neither. It would otherwise fetch the
 *  `Change` row on open and pop the body in late. Seeding from this in-thread
 *  data paints the body at full height immediately. */
type ProposedChangeInfo = { description?: string; fileCount?: number };

function buildProposedChangeInfo(exchanges: Exchange[]): Map<string, ProposedChangeInfo> {
  const map = new Map<string, ProposedChangeInfo>();
  for (const ex of exchanges) {
    for (const { event } of ex.steps) {
      // Per-commit ChangeProposed emits carry an empty change_id, which the
      // truthiness check skips. The aggregate proposal carries the real id and
      // the full file list.
      if (event.type === 'ChangeProposed' && event.change_id && !map.has(event.change_id)) {
        map.set(event.change_id, { description: event.description, fileCount: event.files?.length });
      }
    }
  }
  return map;
}

const NO_PROPOSED_CHANGE_INFO = new Map<string, ProposedChangeInfo>();

/** The matched event of a delivery, keyed by the `EventWaitDelivered`'s own
 *  event id, which is what the anchor's `UserPromptInjected.delivered_event_id`
 *  names.
 *
 *  Resolved at thread level because the two events land in DIFFERENT exchanges.
 *  The delivery is not an exchange-start type, so it attaches to whatever
 *  exchange was open, and the injection after it starts a new one. A
 *  `ChatExchange` therefore cannot see its own delivery's payload. Reading
 *  `threadMap` to find one would resubscribe every exchange to the store and
 *  undo the memo (see `chatExchangePropsEqual`).
 *
 *  The payload is stringified HERE, once per grouping pass, for two reasons. A
 *  string is a primitive the memo compares without a deep walk, and the
 *  formatting is a pure function of the value. */
type DeliveredEventInfo = { eventType: string; eventId?: string; payloadJson?: string };

function buildDeliveredEventInfo(exchanges: Exchange[]): Map<string, DeliveredEventInfo> {
  const map = new Map<string, DeliveredEventInfo>();
  for (const ex of exchanges) {
    for (const { event } of ex.steps) {
      if (event.type !== 'EventWaitDelivered' || !event._eventId) continue;
      map.set(event._eventId, {
        eventType: event.event_type,
        // The event that MATCHED, not this delivery's own id: it is what the
        // delivery card's jump navigates to. Absent on a delivery the engine wrote
        // without one.
        eventId: event.event_id,
        payloadJson: formatDeliveredPayload(event.payload),
      });
    }
  }
  return map;
}

/** Pretty-print a delivered payload for the disclosure, or return undefined
 *  when there is nothing worth expanding. An empty object is the common shape
 *  for a marker event, and a disclosure opening onto `{}` is a worse affordance
 *  than none. */
export function formatDeliveredPayload(payload: unknown): string | undefined {
  if (payload === null || payload === undefined) return undefined;
  if (typeof payload === 'object' && Object.keys(payload as object).length === 0) return undefined;
  try {
    return JSON.stringify(payload, null, 2);
  } catch {
    // Cyclic or otherwise unserializable: the event NAME is still the answer to
    // "why is this thread talking again", so drop only the payload rather than
    // the row.
    return undefined;
  }
}

const NO_DELIVERED_EVENT_INFO = new Map<string, DeliveredEventInfo>();

/** The `EventWaitDelivered` id this exchange is the delivery for, if it is one
 *  at all. Exported shape of the "is this a delivery" test, so the cheap
 *  has-any check and the per-exchange lookup can't drift apart. */
function deliveryEventId(ex: Exchange): string | undefined {
  const ev = ex.userEvent;
  return ev.type === 'UserPromptInjected' ? ev.delivered_event_id : undefined;
}

/** Threads the last model/effort across exchanges so each child sees its predecessors' state. */
export function renderExchanges(
  exchanges: Exchange[],
  threadId: string,
  streamingBuffer: string,
  /** Windowing: emit DOM only for exchanges at this index or later. The loop
   *  iterates the FULL array, so every index-based decision and the prior
   *  model/effort accumulator stay correct. Only `nodes.push` is gated. A large
   *  thread therefore renders and markdown-parses just its visible tail, and
   *  older exchanges materialize as the user scrolls up (see ThreadView's
   *  `renderCount`). Default 0 renders all, which is what the tests and the
   *  deep-link path use. */
  renderFromIndex = 0,
): VNode[] {
  // Compute once which abort exchange (if any) gets the Continue button: the
  // most recent ResponseAborted the user may actually resume from. See
  // `continuableAbortIndex` for the three ways that comes back empty, the
  // sharpest being a switch teardown the engine is already auto-resuming.
  const continuableIdx = continuableAbortIndex(exchanges);
  // Lifted once for the whole list and passed as props to every ChatExchange.
  // These reads subscribe the PARENT to `threadMap` and `cancelingThreadIds`
  // instead of the 29+ child ChatExchanges. ChatExchange is `memo`d, so a
  // meta-shape change wakes only this render pass and the memo skips every
  // exchange whose prop fingerprint is unchanged. On a 29-exchange thread that
  // is 28 times fewer markdown re-parses per SSE event.
  const thread = threadMap.value.get(threadId);
  const threadMeta = thread?.meta;
  const threadIsCC = threadMeta?.channel === 'claude_code';
  const threadCodingAgent = threadMeta?.codingAgent ?? 'claude-code';
  // Quiescent by raw status, but false while an optimistic resume is in flight
  // (just-answered question / un-ingested follow-up) — see isRenderedThreadIdle.
  const threadIdle = isRenderedThreadIdle(thread);
  // Backend says the thread is parked on / resuming from a question or
  // permission card. A just-answered divider whose resume `running` aggregate
  // hasn't reached the client yet must NOT flash "Aborted" — see exchangeStatus.
  const threadAwaitingAnswer = threadMeta?.status === 'waiting_for_user_answer';
  const threadCanceling = cancelingThreadIds.value.has(threadId);
  // When the agent is busy (running, or paused on a question), chat follow-ups
  // typed meanwhile are queued. The queue window includes optimistic messages
  // AND persisted-but-not-yet-injected MessageReceived events, so derive the
  // active exchange and queued set once at the thread level. Queued exchanges
  // then render immediately after the active turn as user bubbles only instead
  // of stealing the live stream or the active 'last' role.
  const threadBusy = threadMeta?.status === 'running' || threadMeta?.status === 'waiting_for_user_answer';
  const queuedRun = queuedFollowupRun(exchanges, threadBusy, threadIsCC);
  const activeIdx = queuedRun.activeIndex;
  const removingQueued = removingQueuedMessageIds.value;
  const removedQueuedIndices = new Set(queuedRun.queuedOrder.filter((i) => {
    const messageId = exchanges[i]?.userEvent._eventId;
    return !!messageId && removingQueued.has(queuedMessageRemovalKey(threadId, messageId));
  }));
  const queuedOrder = queuedRun.queuedOrder.filter(i => !removedQueuedIndices.has(i));
  const queuedIndices = new Set<number>(queuedOrder);
  const queuedCount = queuedOrder.length;
  const nodes: VNode[] = [];
  let lastModel: string | undefined;
  let lastEffort: string | undefined;

  // Only scan for ChangeProposed seeds when the thread actually has a
  // change-lifecycle card to seed — the common (chat) thread pays nothing.
  const hasChangePanel = exchanges.some(ex => isChangeLifecycleEvent(ex.userEvent));
  const proposedChangeInfo = hasChangePanel ? buildProposedChangeInfo(exchanges) : NO_PROPOSED_CHANGE_INFO;
  // Same shape, same reason: only a thread that actually holds an event
  // delivery pays for the scan, so an ordinary thread pays nothing.
  const hasEventDelivery = exchanges.some(ex => deliveryEventId(ex) !== undefined);
  const deliveredEventInfo = hasEventDelivery ? buildDeliveredEventInfo(exchanges) : NO_DELIVERED_EVENT_INFO;

  const renderOne = (ex: Exchange, i: number): VNode => {
    // The active exchange plays the 'last' role (gets the stream, reads
    // 'streaming'/'working'); queued follow-ups after it are explicitly flagged.
    const isLast = i === activeIdx;
    const isQueued = queuedIndices.has(i);
    // Keep the legacy status fallback for non-queued exchanges; queued display
    // is driven by queuedRun so persisted follow-ups don't depend on
    // exchangeStatus' single-last queued branch.
    const priorActive = i > 0 && isStatusActive(getExchangeStatus(exchanges[i - 1], '', /* isLast */ false, /* hasPriorActive */ false, threadIsCC, threadIdle, threadAwaitingAnswer));
    // Seed the change-lifecycle card's body from the in-thread ChangeProposed
    // so it paints at final height on first open (see buildProposedChangeInfo).
    const seedChangeId = isChangeLifecycleEvent(ex.userEvent)
      ? (ex.userEvent as { change_id?: string }).change_id
      : undefined;
    const proposedSeed = seedChangeId ? proposedChangeInfo.get(seedChangeId) : undefined;
    // Undefined when this is not a delivery, and ALSO when the delivery it
    // names is outside the loaded window. Both fall back to the injected prose,
    // which is the honest thing to show when the structured half is not in hand.
    const matchedEvent = deliveredEventInfo.get(deliveryEventId(ex) ?? '');
    return (
      <ChatExchange
        // Key by the stable event id, not userSeq, so an optimistic pending
        // message reconciles IN PLACE when its persisted event arrives. A
        // userSeq key changes on that swap, remounting the node and making the
        // just-sent follow-up flicker away and reappear. See `exchangeKey`.
        key={exchangeKey(ex)}
        exchange={ex}
        // Captured as a primitive at render time: the incremental grouping
        // cache mutates Exchange objects in place, so the memo can't see a
        // change through the identity-stable object — see Exchange.revision.
        revision={ex.revision ?? 0}
        streamingBuffer={isLast ? streamingBuffer : ''}
        isLast={isLast}
        isQueued={isQueued}
        threadId={threadId}
        hasPriorActive={priorActive}
        priorModel={lastModel}
        priorEffort={lastEffort}
        isContinuableAbort={i === continuableIdx}
        threadIsCC={threadIsCC}
        threadCodingAgent={threadCodingAgent}
        threadIdle={threadIdle}
        threadAwaitingAnswer={threadAwaitingAnswer}
        threadCanceling={threadCanceling}
        proposedChangeDesc={proposedSeed?.description}
        proposedChangeFileCount={proposedSeed?.fileCount}
        matchedEventType={matchedEvent?.eventType}
        matchedEventId={matchedEvent?.eventId}
        matchedPayloadJson={matchedEvent?.payloadJson}
      />
    );
  };

  const advance = (ex: Exchange): void => {
    lastModel = exchangeResponseModel(ex) ?? lastModel;
    lastEffort = exchangeReasoningEffort(ex) ?? lastEffort;
  };

  const renderQueued = (): void => {
    if (queuedCount === 0) return;
    if (queuedCount === 1) {
      const i = queuedOrder[0];
      const ex = exchanges[i];
      if (i >= renderFromIndex) nodes.push(renderOne(ex, i));
      advance(ex);
      return;
    }

    const queuedNodes: VNode[] = [];
    for (const j of queuedOrder) {
      const ex = exchanges[j];
      if (j >= renderFromIndex) queuedNodes.push(renderOne(ex, j));
      advance(ex);
    }
    // Queued followups ride the active turn at the tail, so they're virtually
    // always in-window; bail if windowing happened to exclude them all.
    if (queuedNodes.length === 0) return;
    nodes.push(
      <details
        class="queued-message-group"
        key={`queued-${queuedOrder.join('-')}`}
        /* Expanding the group unfolds the queued bubbles below the active turn,
           at the bottom of the thread. It used to snap the transcript down to
           them; it no longer does. A disclosure is not the "take me to the live
           edge" gesture, and growing content under the reader is exactly what
           must not move them (see scrollState's header). The chevron rises on
           the growth, which is the way down. */
      >
        <summary class="queued-message-group-summary">
          <span class="queued-message-group-label">{`Queued (${queuedCount})`}</span>
          <span class="exchange-status-queued">{'○'}</span>
        </summary>
        <div class="queued-message-group-body">
          {queuedNodes}
        </div>
      </details>
    );
  };

  for (let i = 0; i < exchanges.length;) {
    if (queuedRun.queuedIndices.has(i)) {
      i++;
      continue;
    }

    const ex = exchanges[i];
    if (i >= renderFromIndex) nodes.push(renderOne(ex, i));
    advance(ex);

    if (i === activeIdx) {
      renderQueued();
    }

    i++;
  }

  return nodes;
}

// --- Scroll anchoring for turn-control changes (full response, steps) ---
//
// When toggling global signals (stepsExpanded, detailsExpanded), ALL ChatExchange
// components re-render, changing content height. Without compensation, the scroll
// position stays at the same absolute offset but the viewport shows different
// content — the classic "scroll jump."
//
// iOS Safari PWA: WKWebView's compositor adjusts scroll position asynchronously
// when DOM nodes change, and `overflow-anchor` is unsupported (WebKit #171099).
// We freeze the container (`overflow:hidden`) during DOM changes so the browser
// can't touch scrollTop, then compensate and unfreeze.
//
// The scroll container is found via `anchor.closest('.thread-content')` rather
// than by id. Ids are banned on pane chrome (.claude/rules/frontend-css.md),
// and walking up from the anchor cannot pick the wrong element even while a
// layout swap is committing.

/** Where `el`'s top sits inside the transcript's scrollable content, measured to
 *  the SUBPIXEL. This is `offsetTop`, in doubles.
 *
 *  `offsetTop` itself cannot be used for the correction, and the two rects are
 *  taken in one call against the CONTAINER rather than the offset parent. Both
 *  are load-bearing: ADR 0078. Nothing between a `.chat-exchange` and the
 *  transcript is transformed. */
function contentOffsetTop(container: HTMLElement, el: HTMLElement): number {
  return el.getBoundingClientRect().top - container.getBoundingClientRect().top + container.scrollTop;
}

/* --- What the reader is actually looking at -------------------------------
 *
 * The correction holds ONE element still, so which element it is decides who
 * is held. The pressed turn was the obvious candidate and is the wrong one. A
 * coding-agent turn runs to several phone screens, so a reader reading its
 * tail has its top far above the viewport. Each step row revealed between that
 * top and their first visible line pushes them down by its height. That is the
 * reported drift, of a screen or two back up the thread.
 *
 * So the anchor is the reader's OWN topmost line: the first row with any part
 * still on screen. Browsers pick their scroll anchor the same way, and a row
 * ENTIRELY above the edge is excluded for the reason the turn is. Anything
 * revealed between it and the screen displaces the reader by that much.
 */

/** A turn's response is a run of ROWS, one per rendered event, each keyed by
 *  that event (`renderResponseEvents`). So a row keeps its identity across the
 *  reveal, and it is the finest thing worth holding: the reader's own line. */
const ANCHOR_ROW_SELECTOR = '.response-content > *';
/** A turn's two halves, for the reader parked on one with no row of its own to
 *  offer, such as a plain message bubble. */
const ANCHOR_PANEL_SELECTOR = '.initiator-panel, .response-panel';
/** How much of a row has to be on screen before it counts as the reader's own.
 *
 *  A SEAM correction rests a row's bottom exactly on the reader's edge, and that
 *  is the very boundary this scan tests. The next press measures that row a hair
 *  either side of it. Which side it lands on decides the reader's line, and a
 *  sliver that thin shows them nothing, so it must not be allowed to decide.
 *
 *  It must therefore EXCEED the residue the correction itself leaves, and the
 *  floor for that is a whole pixel: the target is rounded to one (ADR 0078,
 *  `reachableScrollTop`), so a pixel of error is expressible by design. Device
 *  pixel snapping adds a fraction on top. At 1px the boundary was straddled by
 *  1.13px on WebKit. It held the row above, putting a 40-step run the reader had
 *  just revealed back under their eye. Two clears the rounding with room, and is
 *  still nothing to read.
 *
 *  Deliberately NOT the 1px of `isWhereWeHeldIt` in scrollState. That one asks
 *  whether the reader has moved, and nothing rounds against it. */
const ANCHOR_SLIVER_PX = 2;

/** Index of the first element in `list` that still REACHES `top`, i.e. whose
 *  bottom edge is below the transcript's top edge by more than a sliver. In
 *  document order, so the answer is the topmost one the reader can see, whether
 *  it starts on screen or straddles the edge. Elements with no box are skipped,
 *  as they are in `recordAnchor`. */
function firstIndexReaching(
  list: ArrayLike<Element>,
  top: number,
  accept: (el: HTMLElement) => boolean = () => true,
): number {
  for (let i = 0; i < list.length; i++) {
    const el = list[i] as HTMLElement;
    if (typeof el.getBoundingClientRect !== 'function') continue;
    const r = el.getBoundingClientRect();
    if (r.height <= 0 || r.bottom <= top + ANCHOR_SLIVER_PX) continue;
    if (accept(el)) return i;
  }
  return -1;
}

function firstRowReaching(
  list: ArrayLike<Element>,
  top: number,
  accept: (el: HTMLElement) => boolean = () => true,
): HTMLElement | null {
  const i = firstIndexReaching(list, top, accept);
  return i < 0 ? null : (list[i] as HTMLElement);
}

/** The first line of the transcript the reader can actually READ, as a viewport
 *  y. The container's own top edge, unless a scrollport-pinned child is drawn
 *  over it. On mobile the sticky thread title covers the top of the transcript.
 *  A row hidden behind it is no more the reader's line than one scrolled off.
 *  It tracks the app header's hide-on-scroll through its own transform, so its
 *  measured bottom is right in both states. Zero height means it is not drawn,
 *  which is every desktop layout.
 *
 *  The LOWEST of them, not the first. The marker invites a second pinned row,
 *  and reading only one would leave the edge above chrome that is drawn over
 *  it. `publishPinnedShift` in utils/iosRepaint.ts serves them all for the same
 *  reason. */
function readerTopEdge(container: HTMLElement): number {
  const top = container.getBoundingClientRect().top;
  let edge = top;
  for (const child of Array.from(container.children) as HTMLElement[]) {
    if (typeof child.getBoundingClientRect !== 'function') continue;
    if (!child.matches?.(`[${SCROLLER_PINNED_ATTR}]`)) continue;
    const r = child.getBoundingClientRect();
    if (r.height > 0 && r.bottom > edge) edge = r.bottom;
  }
  return edge;
}

/** How far the reader's first readable line sits below the container's own top,
 *  i.e. what the mobile sticky title covers.
 *
 *  Read at the moment a seam is written, never carried over from before the
 *  mutation. The title tracks the app header's hide-on-scroll through its own
 *  transform. A reading taken a frame earlier can describe a different edge,
 *  and the seam would land off by that much. */
function readerEdgeInset(container: HTMLElement): number {
  if (typeof container.getBoundingClientRect !== 'function' || !container.children) return 0;
  return readerTopEdge(container) - container.getBoundingClientRect().top;
}

/* --- What the correction owes a reader whose own line is REMOVED ------------
 *
 * Holding an element where it was only holds the READER while nothing between
 * that element and their edge changes. It is exact for their own line, which is
 * why that line is the first choice. It is wrong by the removed height for
 * anything else, and hiding the log removes a whole run at once.
 *
 * A coding-agent turn is the shape that makes it hurt: dozens of tool calls in
 * a row with nothing said between them, so the reader is parked INSIDE the run.
 * Anchored on the turn's response panel, they move by the whole run above them.
 * That is the reported jump of several screens.
 *
 * The run collapses to ONE point, the seam: the bottom of the nearest surviving
 * row above them, which is also the top of the nearest one below. The reader
 * belongs on it, so the correction puts the seam on their edge rather than
 * holding anything where it was.
 */
type AnchorChoice =
  /** Put this element back exactly where it was. */
  | { kind: 'hold'; el: HTMLElement }
  /** Land the reader on the seam their own line collapsed into. Resolved after
   *  the mutation, by `resolveSeam`. */
  | { kind: 'seam'; rows: ArrayLike<Element>; lineIndex: number };

/** Which row names the seam, and which of its edges it is.
 *
 *  Walked outward from the reader's own line and read AFTER the mutation. So
 *  the test is whether a row is still THERE, rather than whether a selector
 *  says the reveal spares it.
 *
 *  That distinction is the whole point. The step log owns the rows carrying its
 *  marker, but the full-response control removes PROSE. A rule keyed on the
 *  marker picks a row that the reveal has just taken away.
 *
 *  Nothing survives between the answer and the reader by construction, which is
 *  what makes its edge the seam. ABOVE is preferred: the reader has read past
 *  it, so landing under it leaves the collapse behind them. */
function resolveSeam(
  rows: ArrayLike<Element>,
  lineIndex: number,
): { el: HTMLElement; atBottom: boolean } | null {
  const drawn = (el: HTMLElement) =>
    el.isConnected
    && typeof el.getBoundingClientRect === 'function'
    && el.getBoundingClientRect().height > 0;
  for (let i = lineIndex - 1; i >= 0; i--) {
    const el = rows[i] as HTMLElement;
    if (drawn(el)) return { el, atBottom: true };
  }
  for (let i = lineIndex + 1; i < rows.length; i++) {
    const el = rows[i] as HTMLElement;
    if (drawn(el)) return { el, atBottom: false };
  }
  return null;
}

/** The reader's own topmost line, and the turn it belongs to.
 *
 *  It walks the transcript's children from the reader's edge DOWN, taking the
 *  first that owns a row reaching that edge. The walk is what a turn's shape
 *  forces. A turn ends in chrome below its last row, so a reader resting there
 *  is inside a turn whose every row is above them. Asking that one turn alone
 *  answered "no line", and the correction fell back to the turn's own top: a
 *  point screens above them, with the whole reveal landing between the two.
 *
 *  It stops at the first turn holding a row, which is the one the reader is
 *  looking into. So it reads the rows of at most one turn that has any. A turn
 *  with none is a change or a divider, skipped for the same reason: there is
 *  nothing in it to hold.
 *
 *  A scrollport-PINNED child is never a turn. It rides the reader's edge
 *  whatever the offset, so it matches every scan and describes nobody's reading
 *  position. It is what SETS that edge instead (`readerTopEdge`). */
function readersLine(
  container: HTMLElement,
  top: number,
): { turn: HTMLElement; rows: ArrayLike<Element>; lineIndex: number } | null {
  const children = container.children;
  for (let i = 0; i < children.length; i++) {
    const turn = children[i] as HTMLElement;
    if (typeof turn.getBoundingClientRect !== 'function') continue;
    const r = turn.getBoundingClientRect();
    if (r.height <= 0 || r.bottom <= top + ANCHOR_SLIVER_PX) continue;
    if (turn.matches?.(`[${SCROLLER_PINNED_ATTR}]`)) continue;
    const rows = turn.querySelectorAll?.(ANCHOR_ROW_SELECTOR);
    if (!rows || rows.length === 0) continue;
    const lineIndex = firstIndexReaching(rows, top);
    if (lineIndex >= 0) return { turn, rows, lineIndex };
  }
  return null;
}

/** What the correction may act on, best first, ending with `fallback`.
 *
 *  More than one, because a reveal can take the reader's own line away. Hiding
 *  the log removes step rows, and the reader may be parked on one. So the list
 *  carries their line, the seam it would collapse into, the panel and turn
 *  around them, and the pressed turn. `restore` takes the first the mutation
 *  left standing, which is the finest answer still on offer.
 *
 *  A container with no `children` is a test double, as elsewhere in the scroll
 *  code, and gets the fallback alone. */
function anchorCandidates(container: HTMLElement, fallback: HTMLElement): AnchorChoice[] {
  const out: AnchorChoice[] = [];
  const hold = (el: HTMLElement | null) => {
    if (el && !out.some(c => c.kind === 'hold' && c.el === el)) out.push({ kind: 'hold', el });
  };
  if (typeof container.getBoundingClientRect === 'function' && container.children) {
    const top = readerTopEdge(container);
    const found = readersLine(container, top);
    // No row reaches the reader's edge anywhere below it: they are in the
    // transcript's tail, past the newest turn's last row. Hold that turn.
    const turn = found ? found.turn : firstRowReaching(
      container.children,
      top,
      el => !el.matches?.(`[${SCROLLER_PINNED_ATTR}]`),
    );
    if (found) {
      const { rows, lineIndex } = found;
      const line = rows[lineIndex] as HTMLElement;
      hold(line);
      // A seam answers only where NOTHING SURVIVING sits between the reader's
      // edge and the line the reveal took. Two shapes qualify. Their edge is
      // inside the line, so there is no room for anything. Or the line has a
      // row above it in this same turn, so the only thing between is the margin
      // those two rows share.
      //
      // The line being this turn's FIRST row is the shape that does not. The
      // reader is then above the turn's whole body, on their own message or on
      // the response header, and those survive. A seam there drags them down to
      // the first row that lived.
      //
      // The sliver is the same one the scan takes, for the same reason: a
      // reader parked on a row's top measures a hair either side of it. A
      // strict test refused the seam on Chromium while allowing it on mobile.
      if (lineIndex > 0 || line.getBoundingClientRect().top <= top + ANCHOR_SLIVER_PX) {
        out.push({ kind: 'seam', rows, lineIndex });
      }
    }
    const panels = turn?.querySelectorAll?.(ANCHOR_PANEL_SELECTOR);
    if (panels) hold(firstRowReaching(panels, top));
    hold(turn);
  }
  hold(fallback);
  return out;
}

/** The correction to write, snapped to a whole pixel because that is all a
 *  scroll offset can hold. The tween deliberately does the opposite, and both
 *  measurements are in ADR 0078.
 *
 *  Rounding also makes the clamp deficit below measurable. An integer target
 *  minus the integer offset the container can reach is the clamp and nothing
 *  else. A fractional target would leave a sub-pixel remainder on every reveal
 *  for the debt's own slack to absorb. */
function reachableScrollTop(target: number): number {
  return Math.round(target);
}

/** Where `top` will actually come to rest, the browser clamping a scroll offset
 *  to the container's own extent.
 *
 *  DERIVED, never read back from the write. Reading `scrollTop` in the write's
 *  own task, on a container whose children changed a moment earlier, does not
 *  reliably answer with the new value. The debt below was measured that way,
 *  and on WebKit it drifted the reader by several hundred pixels in one run out
 *  of a few. An intermittent debt is worse than none, and the deficit is a
 *  property of the geometry, so the geometry is what is read. */
function landingScrollTop(container: HTMLElement, top: number): number {
  const max = Math.max(0, container.scrollHeight - container.clientHeight);
  return Math.min(Math.max(top, 0), max);
}

/* --- The correction the clamp ate, carried to the next reveal ---------------
 *
 * A reveal that SHRINKS the transcript can leave the anchor unreachable. With
 * less content below it than the viewport is tall, no offset puts it back, so
 * the browser clamps and the turn slides.
 *
 * That much is geometry. What is not is the ROUND TRIP. The reverse toggle
 * restores its own delta from wherever the clamp left the reader. It therefore
 * lands short of where they started, and every pair of taps drifts again. The
 * clamp was never chosen by the reader. So the deficit is remembered and paid
 * back by the next anchored mutation on the same container.
 *
 * It is dropped the moment the reader scrolls. `debtAt` records where our write
 * landed, and a container sitting anywhere else has been moved by somebody
 * whose position now counts. `debtHeight` is the other half of that test, and
 * what keeps the debt inside ONE thread. The transcript element is REUSED
 * across threads. So an offset `useScrollMemory` restores could land on the
 * remembered one by coincidence and collect a debt it never earned. Same
 * element, same offset AND same content height is a coincidence not worth
 * engineering further against.
 *
 * One container's worth, not a per-container map: a reveal is transcript-wide
 * and there is one transcript. */
let anchorDebtEl: HTMLElement | null = null;
let anchorDebt = 0;
let anchorDebtAt = -1;
let anchorDebtHeight = -1;

/** What a previous reveal still owes this container, or 0. */
function carriedAnchorDebt(container: HTMLElement): number {
  if (anchorDebtEl !== container || container.scrollHeight !== anchorDebtHeight) return 0;
  // 1px of slack for a browser re-rounding a fractional offset, matching
  // `isWhereWeHeldIt` in scrollState.ts.
  return Math.abs(container.scrollTop - anchorDebtAt) <= 1 ? anchorDebt : 0;
}

/** Retire the debt the moment the reader takes the container somewhere else.
 *
 *  Watched as an EVENT, and the comparison above cannot stand in for it. That
 *  one is asked once, at the next reveal, so it sees only where the reader
 *  ENDED UP. The clamped offset IS the live edge of the shrunk transcript. A
 *  reader who scrolled up to read comes back to it, to the pixel. Asked per
 *  scroll event, the trip away is seen even though the return hides it, and a
 *  bottom the reader chose stays theirs.
 *
 *  Our own correction's echo is not a trip away. A `scrollTop` write dispatches
 *  its event a frame later. By then the recorded offset IS where we put the
 *  container, so the same 1px of slack ignores it. */
function anchorDebtScrolled(): void {
  if (anchorDebtEl && Math.abs(anchorDebtEl.scrollTop - anchorDebtAt) > 1) clearAnchorDebt();
}

function clearAnchorDebt(): void {
  if (anchorDebtEl && typeof anchorDebtEl.removeEventListener === 'function') {
    anchorDebtEl.removeEventListener('scroll', anchorDebtScrolled);
  }
  anchorDebtEl = null;
  anchorDebt = 0;
  anchorDebtAt = -1;
  anchorDebtHeight = -1;
}

/** Record what the clamp will eat out of `wanted`, and the state it leaves the
 *  container in. A debt inside the same 1px of slack is no debt at all.
 *
 *  It takes the TARGET rather than a deficit, so the clamp is derived once, in
 *  `landingScrollTop`. The debt and the position it is watched against come
 *  from one reading. Neither can be a `scrollTop` the browser has yet to apply.
 *
 *  Clears first unconditionally. Re-recording on the same container cannot then
 *  leave two watchers on it. A debt moving to another container takes its
 *  watcher off the old one. */
function rememberAnchorDebt(container: HTMLElement, wanted: number): void {
  clearAnchorDebt();
  const landed = landingScrollTop(container, wanted);
  const debt = wanted - landed;
  if (Math.abs(debt) <= 1) return;
  anchorDebtEl = container;
  anchorDebt = debt;
  anchorDebtAt = landed;
  anchorDebtHeight = container.scrollHeight;
  // A container with no `addEventListener` is a test double, as elsewhere in the
  // scroll code; the offset comparison above still covers it.
  if (typeof container.addEventListener === 'function') {
    container.addEventListener('scroll', anchorDebtScrolled, { passive: true });
  }
}

/** Keep the reader visually still while `fn` mutates the DOM. `anchor` is the
 *  turn the control was pressed on, and the LAST resort: see
 *  `anchorCandidates`, which prefers the reader's own topmost line. */
export function withScrollAnchor(anchor: Element | null | undefined, fn: () => void) {
  const container = anchor?.closest('.thread-content') as HTMLElement | null;
  if (!container || !anchor) { fn(); return; }

  // Blur focused element inside container — iOS auto-scrolls to keep focus visible.
  const focused = document.activeElement;
  if (focused instanceof HTMLElement && container.contains(focused)) {
    focused.blur();
  }

  // Measured BEFORE the mutation, all of them, because which one survives it is
  // not knowable yet. Each carries its own reading, so whichever `restore` picks
  // has a pair taken of the same element.
  const candidates = anchorCandidates(container, anchor as HTMLElement);
  // Only a HOLD needs a reading from before: it is the pair the correction
  // subtracts. A seam is read out of the layout the mutation leaves.
  const offsetsBefore = candidates.map(c => (c.kind === 'hold' ? contentOffsetTop(container, c.el) : 0));
  const scrollBefore = container.scrollTop;
  // Read BEFORE the mutation, while the container is still where the last
  // correction left it: after `fn` a shrink may have clamped it somewhere else.
  const carried = carriedAnchorDebt(container);
  const overflowBefore = container.style.overflow;
  let restored = false;

  // Freeze: prevent browser from adjusting scroll during DOM changes.
  container.style.overflow = 'hidden';

  const restore = () => {
    if (restored) return;
    restored = true;
    observer.disconnect();

    // A reader being CARRIED to the live edge asked for the opposite of an
    // anchor correction: hold me on the newest content, not on what I was
    // looking at. Correcting them and then letting `honourAnchoredMutation`
    // bring them back down moves the transcript UP and then DOWN for one tap.
    // The freeze has kept the container at `scrollBefore` through the mutation,
    // so skipping the correction leaves the live-edge write as the ONE motion.
    //
    // `followIsCarrying` and not the bare armed flag. This and
    // `honourAnchoredMutation` are ONE decision split across the DOM/layout
    // line, and the follow stands down on an idle thread. An armed reader
    // clicking around a finished thread would otherwise get neither the
    // correction nor the snap, and drift on whatever grew above them.
    const riding = followIsCarrying();
    // The first candidate the mutation left standing. A detached element does
    // not say so: it measures as a zero rect, which reads as a turn that moved
    // to the top of the thread and moves the reader somewhere meaningless. With
    // every candidate gone there is nothing to hold them on, so leave them where
    // the freeze kept them. The next-frame re-check stands down on this same
    // test, and a correction that asks for no move is never scheduled at all.
    //
    // Resolved ONCE, so the next-frame re-check cannot pick a different row and
    // write somewhere else. A seam that resolves to nothing is no answer, so it
    // falls through to the candidates behind it.
    const settled = ((): { el: HTMLElement; hold: number | null; atBottom: boolean } | null => {
      for (let i = 0; i < candidates.length; i++) {
        const c = candidates[i];
        if (c.kind === 'hold') {
          if (c.el.isConnected) return { el: c.el, hold: offsetsBefore[i], atBottom: false };
          continue;
        }
        const seam = resolveSeam(c.rows, c.lineIndex);
        if (seam) return { el: seam.el, hold: null, atBottom: seam.atBottom };
      }
      return null;
    })();
    const anchored = settled !== null;
    const el = settled ? settled.el : null;
    // Where the correction wants the container, read from the layout the
    // mutation left. One definition, because the next-frame re-check has to ask
    // the same question a frame later.
    //
    // `carried` repays what a previous reveal's clamp ate, and belongs to the
    // HOLD arm alone: it is a deficit in the offset that arm derives from. A
    // seam is read fresh out of the new layout and owes nothing to the old one.
    const targetNow = (): number => {
      if (!settled) return scrollBefore;
      const offset = contentOffsetTop(container, settled.el);
      if (settled.hold !== null) return reachableScrollTop(scrollBefore + carried + (offset - settled.hold));
      const height = settled.atBottom ? settled.el.getBoundingClientRect().height : 0;
      return reachableScrollTop(offset + height - readerEdgeInset(container));
    };
    // A riding reader owes and is owed nothing: the live edge is always
    // reachable, and they are about to be put on it. Nor does a reader whose
    // every candidate left the DOM, since declining to correct is declining to
    // know what the clamp would have eaten.
    const wanted = targetNow();
    // The unfreeze is the one step that MUST happen, so it goes in a `finally`.
    // `restored` is already true and the observer already gone, so the rAF
    // safety net below cannot lift the freeze a second time: a throw between
    // here and the unfreeze would leave the transcript unscrollable until a
    // reload. `markAnchorScroll` fans out to subscribers, which is the first
    // extensible call inside the freeze. The throw still propagates, since a
    // subscriber failing silently is its own bug (`frontend.md`).
    try {
      if (riding || !anchored) {
        clearAnchorDebt();
      } else {
        markAnchorScroll(container, wanted);
        rememberAnchorDebt(container, wanted);
      }
    } finally {
      container.style.overflow = overflowBefore;
    }

    // The overflow freeze plus a large DOM shrink can leave iOS WKWebView
    // showing a blanked layer texture. The whole `.thread-content` renders
    // black until a scroll forces a repaint, so trigger it proactively.
    forceIOSRepaint(container);

    // Tell the transcript that THIS correction was ours, so it cannot read as
    // the reader scrolling away and retire their standing follow. Only a scroll
    // may do that (ADR 0064). It also lands a riding reader on the live edge.
    // Placed after the unfreeze and the repaint nudge, so any write it makes
    // fights neither. Still inside this frame, so the reader never sees the
    // position the mutation left them at.
    honourAnchoredMutation(container);

    // iOS may adjust after unfreeze, so re-check in the next frame. Skipped
    // while the app is driving this container's scroll: a tween may be in
    // flight, and re-asserting a pre-tween offset against it is a frame of
    // jitter for a correction the tween makes moot. Skipped for a RIDING reader
    // for the stronger version of the same reason: there was no correction to
    // re-assert, and asserting one would drag them off the live edge.
    if (wanted !== scrollBefore && !riding && el) {
      requestAnimationFrame(() => {
        if (!el.isConnected || isOtherNavigationScroll(container)) return;
        const target = targetNow();
        if (Math.abs(container.scrollTop - target) > 1) {
          markAnchorScroll(container, target);
          // Re-assert the debt against where this write comes to rest. Without
          // it, a target the clamp cannot reach would be re-written every frame
          // AND recorded against a stale position.
          rememberAnchorDebt(container, target);
          honourAnchoredMutation(container);
        }
      });
    }
  };

  const observer = new MutationObserver(() => restore());
  observer.observe(container, { childList: true, subtree: true });

  fn();

  // Synchronous check (if DOM changed synchronously). Asked of the best
  // candidate alone: it is the reader's own line, so anything that moved the
  // rest moved it too. A seam names no element yet, and never leads the list
  // while a line does, so there is nothing to ask it.
  const best = candidates[0];
  if (!restored && best?.kind === 'hold'
      && contentOffsetTop(container, best.el) !== offsetsBefore[0]) restore();

  // After Preact's Promise microtask render
  queueMicrotask(() => queueMicrotask(restore));

  // Final safety net — ensure overflow is always restored
  requestAnimationFrame(restore);
}

/** Wire a transcript element to the shared scroll signals. Attaches the scroll
 *  and resize observers (`makeScrollObservers`). Registers it as the active
 *  scroll target, so the chevrons and deep links know which transcript to move.
 *  Nothing here reacts to content: no layout effect snaps the container to the
 *  bottom on arrival (ADR 0064).
 *
 *  Listener setup tracks the actual DOM element via a ref, not just the `ready`
 *  boolean. When the element changes, listeners are detached from the old one
 *  and reattached to the new one on the next render. That prevents dead
 *  listeners feeding scroll events to a detached node. */
export function useScrollObservers(ref: preact.RefObject<HTMLDivElement>, ready: boolean) {
  const listenerRef = useRef<{ el: HTMLDivElement; cleanup: () => void } | null>(null);

  // Check on every render whether the target element has changed.
  // Runs after commit (ref.current is set), so we always see the current element.
  useEffect(() => {
    const el = ready ? ref.current : null;
    const prev = listenerRef.current;

    // Same element (or both null) — nothing to do
    if (el === (prev?.el ?? null)) return;

    // Cleanup old listeners
    prev?.cleanup();
    listenerRef.current = null;

    if (!el) return;

    // `detachGestures` unwires the reader-gesture listeners the observers came
    // with, which is what tells `onScroll` a scroll was the READER's rather
    // than the platform's. Removed with the `scroll` listener below, so a
    // recycled transcript never leaves a detached element feeding the signal.
    const { onScroll, onResize, detachGestures } = makeScrollObservers(el);

    el.addEventListener('scroll', onScroll, { passive: true });
    const ro = new ResizeObserver(onResize);
    ro.observe(el);
    // The container is `position: absolute; inset: 0` (chat.css), so its own
    // box never resizes when children grow. Observing it alone would miss every
    // in-thread size change. Observe each child too, and re-observe on
    // childList changes so new exchanges join in.
    function observeChildren() {
      for (const child of Array.from(el!.children)) {
        ro.observe(child);
      }
    }
    const mo = new MutationObserver(observeChildren);
    mo.observe(el, { childList: true });
    observeChildren();

    // Register as the active scroll target so scrollToBottom() moves the
    // transcript the user can actually see, never one laid out at zero size.
    if (isElementVisible(el)) {
      setActiveScrollElement(el);
    }

    listenerRef.current = {
      el,
      cleanup: () => {
        el.removeEventListener('scroll', onScroll);
        detachGestures();
        ro.disconnect();
        mo.disconnect();
        // Only clear if we're still the active element (another instance
        // may have already registered itself during component transitions).
        if (getActiveScrollElement() === el) {
          setActiveScrollElement(null);
        }
      },
    };
  });

  // Cleanup on unmount
  useEffect(() => () => {
    listenerRef.current?.cleanup();
    listenerRef.current = null;
    notAtTop.value = false;
    awayFromBottom.value = false;
  }, []);
}

/** Whether the welcome surface (`WelcomeMessage`) shows on the empty compose
 *  view. One rule: show it until the user dismisses it. The dismissal is stored
 *  in the DB-backed `welcome_suggestions_dismissed` preference, so it sticks
 *  across reloads and devices. `WelcomeMessage` still adapts its body to whether
 *  an LLM provider is configured (provider-setup CTA vs. starter prompts), but
 *  that's content, not gating — both are gated the same way here.
 *  Pure so the gating is unit-testable without rendering the hook-heavy view. */
export function showWelcomeSurface(opts: {
  isEmpty: boolean;
  welcomeDismissed: boolean;
}): boolean {
  return opts.isEmpty && !opts.welcomeDismissed;
}

export function CreateThreadView() {
  const exchanges = activeExchanges.value;
  const streamingBuffer = activeStreamingBuffer.value;
  const threadId = focusedThreadId.value || '';
  const loaded = threadsLoaded.value;
  const areaRef = useRef<HTMLDivElement>(null);
  const isUp = awayFromBottom.value;
  const isNotAtTop = notAtTop.value;

  // Subscribe to the prompt's FLIP move (ThreadPane): the welcome's entrance is
  // sequenced AFTER this move finishes — see the reveal effect below.
  const animating = promptAnimating.value;

  const isEmpty = exchanges.length === 0;
  // Show the welcome until it is dismissed. `welcomeSuggestionsDismissed()`
  // reads the DB-backed preference and fails closed while preferences load, so
  // a returning user who dismissed it never sees a flash. A fresh workspace
  // gets it back once preferences settle and read unset. Reactive on both
  // `exchanges` and the preferences signal.
  const showWelcome = showWelcomeSurface({
    isEmpty,
    welcomeDismissed: welcomeSuggestionsDismissed(),
  });

  // Sequenced welcome entrance. The surface stays hidden on its CSS
  // `opacity: 0` base until the prompt textarea finishes sliding up, then
  // `.welcome-revealing` plays the enter animation. `promptAnimating` is the
  // authoritative end-of-move signal: ThreadPane flips it true for the FLIP
  // move and false on the transform `transitionend`.
  //
  // The rAF defer wins the race against a move about to START. Entering
  // compose-empty runs this layout effect BEFORE ThreadPane sets
  // `promptAnimating`, so re-checking the live signal a frame later lets a
  // just-started move re-gate the reveal. With no move at all, the welcome
  // reveals on the next frame.
  const [welcomeRevealed, setWelcomeRevealed] = useState(false);
  useLayoutEffect(() => {
    if (!showWelcome) { setWelcomeRevealed(false); return; }
    if (welcomeRevealed || animating) return;
    const raf = requestAnimationFrame(() => {
      if (promptAnimating.value) return; // a move just started — wait for it
      setWelcomeRevealed(true);
    });
    return () => cancelAnimationFrame(raf);
  }, [animating, showWelcome, welcomeRevealed]);

  // hasContent: true exactly when the thread-content div will be in the DOM.
  // useScrollObservers depends on this. If we only pass `loaded`, the effect
  // runs when threadsLoaded becomes true but the div might not exist yet (no
  // exchanges, no welcome). Later when exchanges arrive, the effect never
  // re-runs → no scroll listener → button broken.
  const hasContent = loaded && (showWelcome || !isEmpty);

  useScrollObservers(areaRef, hasContent);

  return (
    <div class="thread-content-wrap">
      {hasContent && (
        <>
          <div class={`thread-content visible${welcomeRevealed ? ' welcome-revealing' : ''}`} ref={areaRef}>

            {showWelcome ? (
              <WelcomeMessage />
            ) : (
              renderExchanges(exchanges, threadId, streamingBuffer)
            )}
          </div>
          {!isEmpty && (
            <ScrollControls
              showUp={isNotAtTop}
              showDown={isUp}
              // `scrollToTop`, not a raw `scrollTo` on the ref. The up chevron
              // is a navigation. The module owning navigations is what ends the
              // ride, supersedes a deep-link claim and settles the chevron
              // signal (ADR 0064).
              onScrollUp={scrollToTop}
              onScrollDown={scrollToBottom}
            />
          )}
        </>
      )}
    </div>
  );
}

// Buttons stay mounted so the CSS `.visible` class can drive the fade transition.
export function ScrollControls({ showUp, showDown, onScrollUp, onScrollDown }: {
  showUp: boolean;
  showDown: boolean;
  onScrollUp: () => void;
  onScrollDown: () => void;
}) {
  return (
    <>
      <button class={`scroll-to-top${showUp ? ' visible' : ''}`} onClick={onScrollUp} aria-label="Scroll to top">
        <ChevronUpIcon />
      </button>
      <button class={`scroll-to-bottom${showDown ? ' visible' : ''}`} onClick={onScrollDown} aria-label="Scroll to bottom">
        <ChevronDownIcon />
      </button>
    </>
  );
}
