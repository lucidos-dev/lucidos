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
import { awayFromBottom, notAtTop, scrollToBottom, scrollToTop, setActiveScrollElement, getActiveScrollElement, isElementVisible, makeScrollObservers, honourAnchoredMutation, isNavigationScroll, followIsCarrying } from './scrollState';
import { ChatExchange } from './ChatExchange';
import { ChevronUpIcon, ChevronDownIcon } from '../shared/icons';
import { WelcomeMessage } from './WelcomeMessage';
import type { Exchange } from '../../store/thread-events';
import { exchangeStatus as getExchangeStatus, exchangeResponseModel, exchangeReasoningEffort, exchangeKey, continuableAbortIndex, queuedFollowupRun, isChangeLifecycleEvent } from '../../store/thread-events';
import { isActive as isStatusActive } from '../../store/exchange-status';
import { forceIOSRepaint } from '../../utils/iosRepaint';

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

/** The correction to write, snapped to a whole pixel because that is all a
 *  scroll offset can hold. The tween deliberately does the opposite, and both
 *  measurements are in ADR 0078.
 *
 *  Rounding also makes the clamp deficit below measurable. An integer target
 *  minus the integer the container settled at is the clamp and nothing else. A
 *  fractional target would leave a sub-pixel remainder on every reveal for the
 *  debt's own slack to absorb. */
function reachableScrollTop(target: number): number {
  return Math.round(target);
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

/** Record what the clamp ate, and the state it left the container in. A debt
 *  inside the same 1px of slack is no debt at all.
 *
 *  Clears first unconditionally. Re-recording on the same container cannot then
 *  leave two watchers on it. A debt moving to another container takes its
 *  watcher off the old one. */
function rememberAnchorDebt(container: HTMLElement, debt: number): void {
  clearAnchorDebt();
  if (Math.abs(debt) <= 1) return;
  anchorDebtEl = container;
  anchorDebt = debt;
  anchorDebtAt = container.scrollTop;
  anchorDebtHeight = container.scrollHeight;
  // A container with no `addEventListener` is a test double, as elsewhere in the
  // scroll code; the offset comparison above still covers it.
  if (typeof container.addEventListener === 'function') {
    container.addEventListener('scroll', anchorDebtScrolled, { passive: true });
  }
}

/** Keep `anchor` visually pinned while `fn` mutates the DOM. */
export function withScrollAnchor(anchor: Element | null | undefined, fn: () => void) {
  const container = anchor?.closest('.thread-content') as HTMLElement | null;
  if (!container || !anchor) { fn(); return; }

  // Blur focused element inside container — iOS auto-scrolls to keep focus visible.
  const focused = document.activeElement;
  if (focused instanceof HTMLElement && container.contains(focused)) {
    focused.blur();
  }

  const el = anchor as HTMLElement;
  const offsetBefore = contentOffsetTop(container, el);
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
    // A mutation that took the anchor OUT of the DOM leaves nothing to hold the
    // reader on, and a detached element does not say so. It measures as a zero
    // rect. The correction then reads as a turn that moved to the top of the
    // thread and moves the reader somewhere meaningless. Leave them where the
    // freeze kept them. The next-frame re-check stands down on this same test,
    // and a zero delta keeps it from being scheduled at all.
    const anchored = el.isConnected;
    const delta = anchored ? contentOffsetTop(container, el) - offsetBefore : 0;
    // `carried` repays what a previous reveal's clamp ate; what THIS one cannot
    // reach is recorded for the next. A riding reader owes and is owed nothing:
    // the live edge is always reachable, and they are about to be put on it. Nor
    // does a reader whose anchor left the DOM, since declining to correct is
    // declining to know what the clamp would have eaten.
    const wanted = reachableScrollTop(scrollBefore + carried + delta);
    if (riding || !anchored) {
      clearAnchorDebt();
    } else {
      container.scrollTop = wanted;
      rememberAnchorDebt(container, wanted - container.scrollTop);
    }
    container.style.overflow = overflowBefore;

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
    if (delta !== 0 && !riding) {
      requestAnimationFrame(() => {
        if (!el.isConnected || isNavigationScroll(container)) return;
        const target = reachableScrollTop(scrollBefore + carried + (contentOffsetTop(container, el) - offsetBefore));
        if (Math.abs(container.scrollTop - target) > 1) {
          container.scrollTop = target;
          // Re-assert the debt against where this write actually landed. Without
          // it, a target the clamp cannot reach would be re-written every frame
          // AND recorded against a stale position.
          rememberAnchorDebt(container, target - container.scrollTop);
          honourAnchoredMutation(container);
        }
      });
    }
  };

  const observer = new MutationObserver(() => restore());
  observer.observe(container, { childList: true, subtree: true });

  fn();

  // Synchronous check (if DOM changed synchronously)
  if (!restored && contentOffsetTop(container, el) !== offsetBefore) restore();

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
