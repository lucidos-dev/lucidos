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
import { awayFromBottom, notAtTop, scrollToBottom, setActiveScrollElement, getActiveScrollElement, isElementVisible, makeScrollObservers } from './scrollState';
import { ChatExchange } from './ChatExchange';
import { ChevronUpIcon, ChevronDownIcon } from '../shared/icons';
import { WelcomeMessage } from './WelcomeMessage';
import type { Exchange } from '../../store/thread-events';
import { exchangeStatus as getExchangeStatus, exchangeResponseModel, exchangeReasoningEffort, exchangeKey, continuableAbortIndex, queuedFollowupRun, isChangeLifecycleEvent } from '../../store/thread-events';
import { isActive as isStatusActive } from '../../store/exchange-status';
import { forceIOSRepaint } from '../../utils/iosRepaint';

/** First line of a change's description + its file count, keyed by change_id —
 *  harvested from the `ChangeProposed` events that ride a thread's coding-agent
 *  turns (as non-rendered steps). A later `ChangeApplied`/`Discarded`/`Reverted`
 *  card for the same change_id is a SEPARATE exchange that carries no
 *  description/file count of its own, so it would otherwise fetch the `Change`
 *  row on open and pop the body in late (the open-path jump). Seeding the body
 *  from this in-thread data paints it at full height immediately. */
type ProposedChangeInfo = { description?: string; fileCount?: number };

function buildProposedChangeInfo(exchanges: Exchange[]): Map<string, ProposedChangeInfo> {
  const map = new Map<string, ProposedChangeInfo>();
  for (const ex of exchanges) {
    for (const { event } of ex.steps) {
      // Per-commit ChangeProposed emits carry an empty change_id (see
      // exchangeChangeId) — the truthiness check skips them; the aggregate
      // proposal carries the real id + the full file list.
      if (event.type === 'ChangeProposed' && event.change_id && !map.has(event.change_id)) {
        map.set(event.change_id, { description: event.description, fileCount: event.files?.length });
      }
    }
  }
  return map;
}

const NO_PROPOSED_CHANGE_INFO = new Map<string, ProposedChangeInfo>();

/** The matched event of a detached wake, keyed by the `EventWaitDelivered`'s own
 *  event id, which is what the wake's `UserPromptInjected.delivered_event_id`
 *  names.
 *
 *  Resolved at thread level because the two events land in DIFFERENT exchanges:
 *  the delivery is not an exchange-start type, so it attaches to whatever
 *  exchange was open, and the injection immediately after it starts a new one.
 *  A `ChatExchange` therefore cannot see its own wake's payload, and having it
 *  read `threadMap` to go find one would resubscribe every exchange to the
 *  store and undo the memo (see `chatExchangePropsEqual`).
 *
 *  The payload is stringified HERE, once per grouping pass, for two reasons: a
 *  string is a primitive the memo can compare without a deep walk, and the
 *  formatting is a pure function of the value with nothing per-render about it. */
type DeliveredEventInfo = { eventType: string; payloadJson?: string };

function buildDeliveredEventInfo(exchanges: Exchange[]): Map<string, DeliveredEventInfo> {
  const map = new Map<string, DeliveredEventInfo>();
  for (const ex of exchanges) {
    for (const { event } of ex.steps) {
      if (event.type !== 'EventWaitDelivered' || !event._eventId) continue;
      map.set(event._eventId, {
        eventType: event.event_type,
        payloadJson: formatDeliveredPayload(event.payload),
      });
    }
  }
  return map;
}

/** Pretty-print a delivered payload for the disclosure, or return undefined when
 *  there is nothing worth expanding. An empty object is the common shape for a
 *  marker event, and a disclosure that opens onto `{}` is a worse affordance
 *  than no disclosure at all. */
export function formatDeliveredPayload(payload: unknown): string | undefined {
  if (payload === null || payload === undefined) return undefined;
  if (typeof payload === 'object' && Object.keys(payload as object).length === 0) return undefined;
  try {
    return JSON.stringify(payload, null, 2);
  } catch {
    // Cyclic or otherwise unserializable: the event NAME is still the answer to
    // "why did this thread wake", so drop only the payload rather than the row.
    return undefined;
  }
}

const NO_DELIVERED_EVENT_INFO = new Map<string, DeliveredEventInfo>();

/** The `EventWaitDelivered` id this exchange is the detached wake for, if it is
 *  one at all. Exported shape of the "is this a wake" test, so the cheap
 *  has-any check and the per-exchange lookup can't drift apart. */
function wakeDeliveryId(ex: Exchange): string | undefined {
  const ev = ex.userEvent;
  return ev.type === 'UserPromptInjected' ? ev.delivered_event_id : undefined;
}

/** Threads the last model/effort across exchanges so each child sees its predecessors' state. */
export function renderExchanges(
  exchanges: Exchange[],
  threadId: string,
  streamingBuffer: string,
  /** Windowing: emit DOM only for exchanges at this index or later. The loop
   *  still iterates the FULL array so every index-based decision (activeIdx,
   *  queued run, continuable abort) and the prior model/effort accumulator
   *  stays correct — only `nodes.push` is gated. A large thread thus renders (and
   *  markdown-parses) just its visible tail; older exchanges materialize as the
   *  user scrolls up (see ThreadView's renderCount). Default 0 = render all (the
   *  pre-windowing behavior, used by tests and the deep-link "render all" path). */
  renderFromIndex = 0,
): VNode[] {
  // Compute once which abort exchange (if any) gets the Continue button: the
  // most recent ResponseAborted the user may actually resume from. See
  // `continuableAbortIndex` for the three ways that comes back empty, the
  // sharpest being a switch teardown the engine is already auto-resuming.
  const continuableIdx = continuableAbortIndex(exchanges);
  // Lifted once for the whole list and passed as props to every ChatExchange:
  // these reads subscribe the PARENT (this function, called from ThreadView)
  // to threadMap + cancelingThreadIds instead of the 29+ child ChatExchanges.
  // Combined with ChatExchange being `memo`d, a meta-shape change wakes only
  // this one render pass and the memo skips per-exchange function bodies for
  // every exchange whose prop fingerprint (userSeq + steps.length + last step
  // seq + threadIsCC + threadIdle + threadCanceling + streamingBuffer + …) is
  // unchanged. On a 29-exchange thread that's 28× fewer markdown re-parses
  // per SSE event compared with each child subscribing independently.
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
  // Same shape, same reason: only a thread that actually holds a detached wake
  // pays for the scan, so an ordinary thread pays nothing.
  const hasEventWake = exchanges.some(ex => wakeDeliveryId(ex) !== undefined);
  const deliveredEventInfo = hasEventWake ? buildDeliveredEventInfo(exchanges) : NO_DELIVERED_EVENT_INFO;

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
    // Undefined when this is not a wake, and ALSO when the delivery it names is
    // outside the loaded window. Both fall back to the injected prose, which is
    // the honest thing to show when the structured half is not in hand.
    const wakeDelivery = deliveredEventInfo.get(wakeDeliveryId(ex) ?? '');
    return (
      <ChatExchange
        // Key by the stable event id (not userSeq) so an optimistic pending
        // message reconciles IN PLACE when its persisted event arrives — a
        // userSeq key changes on that swap (MAX_SAFE_INTEGER → real DB seq),
        // remounting the node and making the just-sent follow-up flicker away
        // then reappear. See exchangeKey.
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
        wakeEventType={wakeDelivery?.eventType}
        wakePayloadJson={wakeDelivery?.payloadJson}
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

// --- Scroll anchoring for toggle changes (More/Less, Show/Hide Steps) ---
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
// than by id: ids are banned on pane chrome (see .claude/rules/frontend-css.md),
// and walking up from the anchor cannot pick the wrong element even while a
// layout swap is committing.

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
  const offsetBefore = el.offsetTop;
  const scrollBefore = container.scrollTop;
  const overflowBefore = container.style.overflow;
  let restored = false;

  // Freeze: prevent browser from adjusting scroll during DOM changes.
  container.style.overflow = 'hidden';

  const restore = () => {
    if (restored) return;
    restored = true;
    observer.disconnect();

    const delta = el.offsetTop - offsetBefore;
    container.scrollTop = scrollBefore + delta;
    container.style.overflow = overflowBefore;

    // The overflow freeze + large DOM shrink (hiding steps drops every tool-call
    // row across the thread) can leave iOS WKWebView showing a blanked layer
    // texture — the whole .thread-content (sticky title bar included) renders
    // black until a scroll forces a repaint. Trigger that repaint proactively.
    forceIOSRepaint(container);

    // iOS may adjust after unfreeze — re-check in next frame.
    if (delta !== 0) {
      requestAnimationFrame(() => {
        if (!el.isConnected) return;
        const target = scrollBefore + (el.offsetTop - offsetBefore);
        if (Math.abs(container.scrollTop - target) > 1) {
          container.scrollTop = target;
        }
      });
    }
  };

  const observer = new MutationObserver(() => restore());
  observer.observe(container, { childList: true, subtree: true });

  fn();

  // Synchronous check (if DOM changed synchronously)
  if (!restored && el.offsetTop !== offsetBefore) restore();

  // After Preact's Promise microtask render
  queueMicrotask(() => queueMicrotask(restore));

  // Final safety net — ensure overflow is always restored
  requestAnimationFrame(restore);
}

/** Wire a transcript element to the shared scroll signals: attach the scroll +
 *  resize observers (`makeScrollObservers`) and register it as the active scroll
 *  target so the chevrons and deep links know which transcript to move.
 *
 *  It was `useAutoScroll`, and it carried a layout effect that snapped the
 *  container to the bottom on every content arrival. That was the app's main
 *  bottom-pin, so it is gone, and with it the `deps` array that told it when
 *  content had arrived: nothing here reacts to content any more. The name
 *  follows, since a hook called `useAutoScroll` that does not auto-scroll would
 *  mislead every future reader of the two call sites.
 *
 *  Listener setup tracks the actual DOM element via a ref, not just the `ready`
 *  boolean. If the element changes (e.g. component unmount/remount during SSE
 *  reconnection), listeners are detached from the old element and reattached to
 *  the new one on the next render. This prevents "dead listener" bugs where
 *  scroll events go to a detached DOM node. */
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

    const { onScroll, onResize } = makeScrollObservers(el);

    el.addEventListener('scroll', onScroll, { passive: true });
    const ro = new ResizeObserver(onResize);
    ro.observe(el);
    // The container is position:absolute inset:0 (chat.css), so its own box
    // never resizes when children grow — observing it alone misses in-thread
    // size changes (panel header expand/collapse). Observe each child too,
    // and re-observe on childList changes so new exchanges join in.
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
  // Show the welcome until it's dismissed. `welcomeSuggestionsDismissed()` reads
  // the DB-backed preference and fails closed while preferences load (returns
  // true), so a returning user who already dismissed never sees a flash; a fresh
  // workspace gets it back once preferences settle and read unset. Reactive on
  // both `exchanges` and the preferences signal.
  const showWelcome = showWelcomeSurface({
    isEmpty,
    welcomeDismissed: welcomeSuggestionsDismissed(),
  });

  // Sequenced welcome entrance: hold the surface hidden (CSS `opacity: 0` base)
  // until the prompt textarea has finished sliding up to its resting position,
  // then add `.welcome-revealing` to play the fade+slide enter animation.
  // `promptAnimating` is the authoritative end-of-move signal — ThreadPane flips
  // it true for the FLIP move and false on the transform `transitionend`. The
  // rAF defer wins the race against a move that's about to START: when entering
  // compose-empty this child's layout effect runs BEFORE ThreadPane's sets
  // `promptAnimating`, so re-checking the live signal one frame later lets a
  // just-started move re-gate the reveal. When there's no move (initial mount,
  // mobile, reduced-motion) the welcome reveals on the next frame — no delay.
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
              onScrollUp={() => areaRef.current?.scrollTo({ top: 0, behavior: 'smooth' })}
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
