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
import { homeThreadId } from '../../store/actions/homeThread';
import { anchorSpacer, setAnchorSpacer, splitAnchorCorrection } from './anchorCorrection';
import { onClassifiedScrollAsEngine } from './classifiedScroll';
import { awayFromBottom, showScrollToBottom, notAtTop, scrollToBottom, scrollToTop, setActiveScrollElement, getActiveScrollElement, makeScrollObservers, honourAnchoredMutation, isOtherNavigationScroll, markAnchorScroll, readerGestureSince } from './scrollState';
import { isElementVisible } from '../../utils/elementVisibility';
import { ChatExchange } from './ChatExchange';
import type { ProposedChangeSeed } from './ChatExchange';
import { NO_SIDE_QUESTIONS, placeSideQuestions, sideQuestionOwners } from './SideQuestionCard';
import type { SideQuestion } from '../../store/sideQuestions';
import { ChevronUpIcon, ChevronDownIcon } from '../shared/icons';
import { WelcomeMessage } from './WelcomeMessage';
import type { Exchange, StoredEvent } from '../../store/thread-events';
import { exchangeStatus as getExchangeStatus, exchangeResponseModel, exchangeReasoningEffort, sessionCodingAgents, exchangeKey, continuableAbortIndex, queuedFollowupRun, readMarkers, isChangeLifecycleEvent, restartPauseFoldsInto, opensAwaitingAnswer, agentWorkedSince, threadFlagsChangeDrawing } from '../../store/thread-events';
import { isActive as isStatusActive } from '../../store/exchange-status';
import { forceWebKitRepaint, settledScrollTop } from '../../utils/webkitRepaint';
import { opensSoftwareKeyboard } from '../../utils/dom';
import { nowMs } from '../../utils/scrollActivity';
import { scaledDurationMs } from '../../utils/motion';
import { DISCLOSURE_MAX_MS } from '../../utils/disclosureMotion';

export function buildProposedChangeInfo(exchanges: Exchange[]): Map<string, ProposedChangeSeed> {
  const map = new Map<string, ProposedChangeSeed>();
  const summaries = new Map<string, Map<string, string>>();
  for (const ex of exchanges) {
    for (const { event } of ex.steps) {
      if (event.type === 'ChangeProposed' && event.change_id) {
        map.set(event.change_id, { description: event.description, fileCount: event.files?.length });
      } else if (event.type === 'ChangeSummarized' && event.change_id && event.summary) {
        const byDescription = summaries.get(event.change_id) ?? new Map<string, string>();
        byDescription.set(event.description ?? '', event.summary);
        summaries.set(event.change_id, byDescription);
      }
    }
  }
  for (const [changeId, seed] of map) {
    seed.summary = summaries.get(changeId)?.get(seed.description ?? '');
  }
  return map;
}

const NO_PROPOSED_CHANGE_INFO = new Map<string, ProposedChangeSeed>();

type DeliveredEventInfo = { eventType: string; eventId?: string; payloadJson?: string };

function buildDeliveredEventInfo(exchanges: Exchange[]): Map<string, DeliveredEventInfo> {
  const map = new Map<string, DeliveredEventInfo>();
  for (const ex of exchanges) {
    for (const { event } of ex.steps) {
      if (event.type !== 'EventWaitDelivered' || !event._eventId) continue;
      map.set(event._eventId, {
        eventType: event.event_type,
        eventId: event.event_id,
        payloadJson: formatDeliveredPayload(event.payload),
      });
    }
  }
  return map;
}

export function formatDeliveredPayload(payload: unknown): string | undefined {
  if (payload === null || payload === undefined) return undefined;
  if (typeof payload === 'object' && Object.keys(payload as object).length === 0) return undefined;
  try {
    return JSON.stringify(payload, null, 2);
  } catch {
    return undefined;
  }
}

const NO_DELIVERED_EVENT_INFO = new Map<string, DeliveredEventInfo>();

function deliveryEventId(ex: Exchange): string | undefined {
  const ev = ex.userEvent;
  return ev.type === 'PromptInjected' ? ev.delivered_event_id : undefined;
}

export function renderExchanges(
  exchanges: Exchange[],
  threadId: string,
  streamingBuffer: string,
  renderFromIndex = 0,
  floorRowsHidden = 0,
  sideQuestions: readonly SideQuestion[] = NO_SIDE_QUESTIONS,
): VNode[] {
  const continuableIdx = continuableAbortIndex(exchanges);
  const thread = threadMap.value.get(threadId);
  const threadMeta = thread?.meta;
  const threadIsCC = threadMeta?.channel === 'claude_code';
  const threadBackend = threadMeta?.codingAgent ?? 'claude-code';
  const sessionBackends = threadIsCC ? sessionCodingAgents(exchanges, threadBackend) : [];
  const threadIdle = isRenderedThreadIdle(thread);
  const threadAwaitingAnswer = threadMeta?.status === 'waiting_for_user_answer';
  const threadCanceling = cancelingThreadIds.value.has(threadId);
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
  const sideQuestionPlaces = sideQuestionOwners(
    exchanges,
    sideQuestions,
    (i) => queuedRun.queuedIndices.has(i) || restartPauseFoldsInto(exchanges[i], exchanges[i + 1]),
  );
  const markers = readMarkers(exchanges, threadIsCC);
  const nodes: VNode[] = [];
  let lastModel: string | undefined;
  let lastEffort: string | undefined;

  const hasChangePanel = exchanges.some(ex => isChangeLifecycleEvent(ex.userEvent));
  const proposedChangeInfo = hasChangePanel ? buildProposedChangeInfo(exchanges) : NO_PROPOSED_CHANGE_INFO;
  const hasEventDelivery = exchanges.some(ex => deliveryEventId(ex) !== undefined);
  const deliveredEventInfo = hasEventDelivery ? buildDeliveredEventInfo(exchanges) : NO_DELIVERED_EVENT_INFO;

  const priorActiveAt = (i: number): boolean =>
    i > 0 && isStatusActive(getExchangeStatus(exchanges[i - 1], '',  false,  false, threadIsCC, threadIdle, threadAwaitingAnswer));

  const pinnedOrder: number[] = [];
  if (threadAwaitingAnswer || !threadIdle) {
    exchanges.forEach((ex, i) => {
      if (queuedRun.queuedIndices.has(i) || !opensAwaitingAnswer(ex)) return;
      const isLast = i === activeIdx;
      const status = getExchangeStatus(ex, isLast ? streamingBuffer : '', isLast, priorActiveAt(i), threadIsCC, threadIdle, threadAwaitingAnswer);
      if (status === 'awaiting-answer' && !agentWorkedSince(exchanges, i)) pinnedOrder.push(i);
    });
  }
  const pinnedIndices = new Set(pinnedOrder);
  const openCardIdx = pinnedOrder.length > 0 ? pinnedOrder[pinnedOrder.length - 1] : -1;

  const renderOne = (ex: Exchange, i: number, pausedBy?: StoredEvent): VNode => {
    const isLast = i === activeIdx;
    const isQueued = queuedIndices.has(i);
    const priorActive = priorActiveAt(i);
    const behindOpenQuestion = openCardIdx >= 0 && i > openCardIdx && !isQueued;
    const flagsMatter = threadFlagsChangeDrawing(ex, {
      streamingBuffer: isLast ? streamingBuffer : '',
      isLast,
      isQueued,
      hasPriorActive: priorActive,
      threadIsCC,
      behindOpenQuestion,
    });
    const seedChangeId = isChangeLifecycleEvent(ex.userEvent)
      ? (ex.userEvent as { change_id?: string }).change_id
      : undefined;
    const proposedSeed = seedChangeId ? proposedChangeInfo.get(seedChangeId) : undefined;
    const matchedEvent = deliveredEventInfo.get(deliveryEventId(ex) ?? '');
    return (
      <ChatExchange
        key={exchangeKey(ex)}
        exchange={ex}
        revision={ex.revision ?? 0}
        streamingBuffer={isLast ? streamingBuffer : ''}
        isLast={isLast}
        isQueued={isQueued}
        readMarker={markers?.get(i)}
        threadId={threadId}
        hasPriorActive={priorActive}
        priorModel={lastModel}
        priorEffort={lastEffort}
        isContinuableAbort={i === continuableIdx}
        threadIsCC={threadIsCC}
        sessionCodingAgent={isQueued ? threadBackend : sessionBackends[i] ?? threadBackend}
        threadIdle={flagsMatter && threadIdle}
        threadAwaitingAnswer={flagsMatter && threadAwaitingAnswer}
        behindOpenQuestion={behindOpenQuestion}
        threadCanceling={threadCanceling}
        rowsHidden={i === renderFromIndex && !pinnedIndices.has(i) ? floorRowsHidden : 0}
        proposedChangeDesc={proposedSeed?.description}
        proposedChangeFileCount={proposedSeed?.fileCount}
        proposedChangeSummary={proposedSeed?.summary}
        matchedEventType={matchedEvent?.eventType}
        matchedEventId={matchedEvent?.eventId}
        matchedPayloadJson={matchedEvent?.payloadJson}
        pausedBy={pausedBy}
        sideQuestions={sideQuestionPlaces.owned.get(i) ?? NO_SIDE_QUESTIONS}
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
      nodes.push(renderOne(ex, i));
      advance(ex);
      return;
    }

    const queuedNodes: VNode[] = [];
    for (const j of queuedOrder) {
      const ex = exchanges[j];
      queuedNodes.push(renderOne(ex, j));
      advance(ex);
    }
    nodes.push(
      <details
        class="queued-message-group"
        key={`queued-${queuedOrder.join('-')}`}
      >
        <summary class="queued-message-group-summary">
          <span class="queued-message-group-label">{`Queued (${queuedCount})`}</span>
        </summary>
        <div class="queued-message-group-body">
          {queuedNodes}
        </div>
      </details>
    );
  };

  let queuedAnchor = activeIdx;
  for (let i = exchanges.length - 1; i >= 0; i--) {
    if (!queuedRun.queuedIndices.has(i)) {
      queuedAnchor = i;
      break;
    }
  }

  const pinnedNodes: VNode[] = [];
  for (let i = 0; i < exchanges.length;) {
    if (queuedRun.queuedIndices.has(i)) {
      i++;
      continue;
    }

    const ex = exchanges[i];
    const folds = restartPauseFoldsInto(ex, exchanges[i + 1]);
    const pausedBy = i > 0 && restartPauseFoldsInto(exchanges[i - 1], ex)
      ? exchanges[i - 1].userEvent
      : undefined;
    if (pinnedIndices.has(i)) pinnedNodes.push(renderOne(ex, i, pausedBy));
    else if (i >= renderFromIndex && !folds) nodes.push(renderOne(ex, i, pausedBy));
    advance(ex);

    if (i === queuedAnchor) {
      nodes.push(...pinnedNodes);
      renderQueued();
    }

    i++;
  }

  return placeSideQuestions(nodes, exchanges, sideQuestionPlaces.unowned, renderFromIndex > 0);
}


function contentOffsetTop(container: HTMLElement, el: HTMLElement): number {
  return el.getBoundingClientRect().top - container.getBoundingClientRect().top + container.scrollTop;
}

const ANCHOR_SETTLE_FRAMES = 12;

const ROLL_SETTLE_SLACK_MS = 50;

export function withScrollAnchor(anchor: Element | null | undefined, fn: () => void) {
  const container = anchor?.closest('.thread-content') as HTMLElement | null;
  if (!container || !anchor) { fn(); return; }

  const focused = document.activeElement;
  if (focused instanceof HTMLElement && container.contains(focused) && opensSoftwareKeyboard(focused)) {
    focused.blur();
  }

  const held = anchor as HTMLElement;
  const offsetBefore = contentOffsetTop(container, held);
  const scrollBefore = settledScrollTop(container);
  const overflowBefore = container.style.overflow;
  const pressedAt = nowMs();
  let restored = false;

  container.style.overflow = 'hidden';
  const offsetFrozen = contentOffsetTop(container, held);

  const restore = () => {
    if (restored) return;
    restored = true;
    observer.disconnect();
    container.style.overflow = overflowBefore;

    const anchored = held.isConnected;
    const targetNow = (): { scrollTop: number; spacer: number } => {
      const offset = contentOffsetTop(container, held);
      return splitAnchorCorrection(scrollBefore + (offset - anchorSpacer(container) - offsetBefore));
    };
    const wanted = anchored ? targetNow() : { scrollTop: container.scrollTop, spacer: anchorSpacer(container) };
    setAnchorSpacer(container, wanted.spacer);
    markAnchorScroll(container, wanted.scrollTop);

    forceWebKitRepaint(container);

    honourAnchoredMutation(container);

    if (anchored) {
      let framesLeft = ANCHOR_SETTLE_FRAMES;
      const rollsLandAt = nowMs() + scaledDurationMs(DISCLOSURE_MAX_MS) + ROLL_SETTLE_SLACK_MS;
      let lastFrameAt = -Infinity;
      const reassert = (frameAt: number) => {
        if (!held.isConnected || isOtherNavigationScroll(container)) return;
        if (readerGestureSince(container, pressedAt)) return;
        const target = targetNow();
        setAnchorSpacer(container, target.spacer);
        if (settledScrollTop(container) !== target.scrollTop) {
          markAnchorScroll(container, target.scrollTop);
          honourAnchoredMutation(container);
        }
        const rolling = frameAt > lastFrameAt && frameAt < rollsLandAt;
        lastFrameAt = frameAt;
        if (--framesLeft <= 0 && !rolling) return;
        requestAnimationFrame(reassert);
      };
      requestAnimationFrame(reassert);
    }
  };

  const observer = new MutationObserver(() => restore());
  observer.observe(container, { childList: true, subtree: true });

  fn();

  if (!restored && contentOffsetTop(container, held) !== offsetFrozen) restore();

  queueMicrotask(() => queueMicrotask(restore));

  requestAnimationFrame(restore);
}

export function useScrollObservers(ref: preact.RefObject<HTMLDivElement>, ready: boolean) {
  const listenerRef = useRef<{ el: HTMLDivElement; cleanup: () => void } | null>(null);

  useEffect(() => {
    const el = ready ? ref.current : null;
    const prev = listenerRef.current;

    if (el === (prev?.el ?? null)) return;

    prev?.cleanup();
    listenerRef.current = null;

    if (!el) return;

    const { onScroll, onResize, detachGestures } = makeScrollObservers(el);

    const stopScroll = onClassifiedScrollAsEngine(el, onScroll);
    const ro = new ResizeObserver(onResize);
    ro.observe(el);
    function observeChildren() {
      for (const child of Array.from(el!.children)) {
        ro.observe(child);
      }
    }
    const mo = new MutationObserver(observeChildren);
    mo.observe(el, { childList: true });
    observeChildren();

    if (isElementVisible(el)) {
      setActiveScrollElement(el);
    }

    listenerRef.current = {
      el,
      cleanup: () => {
        stopScroll();
        detachGestures();
        ro.disconnect();
        mo.disconnect();
        if (getActiveScrollElement() === el) {
          setActiveScrollElement(null);
        }
      },
    };
  });

  useEffect(() => () => {
    listenerRef.current?.cleanup();
    listenerRef.current = null;
    notAtTop.value = false;
    awayFromBottom.value = false;
  }, []);
}

export function welcomePlacement(opts: {
  threadsLoaded: boolean;
  homeId: string | null;
  welcomeDismissed: boolean;
}): 'home' | 'compose' | 'none' {
  if (!opts.threadsLoaded || opts.welcomeDismissed) return 'none';
  return opts.homeId ? 'home' : 'compose';
}

export function CreateThreadView() {
  const exchanges = activeExchanges.value;
  const streamingBuffer = activeStreamingBuffer.value;
  const threadId = focusedThreadId.value || '';
  const loaded = threadsLoaded.value;
  const areaRef = useRef<HTMLDivElement>(null);
  const isUp = showScrollToBottom.value;
  const isNotAtTop = notAtTop.value;

  const animating = promptAnimating.value;

  const isEmpty = exchanges.length === 0;
  const here = threadId && threadId === homeThreadId.value ? 'home' : 'compose';
  const showWelcome = isEmpty && welcomePlacement({
    threadsLoaded: loaded,
    homeId: homeThreadId.value,
    welcomeDismissed: welcomeSuggestionsDismissed(),
  }) === here;

  const [welcomeRevealed, setWelcomeRevealed] = useState(false);
  useLayoutEffect(() => {
    if (!showWelcome) { setWelcomeRevealed(false); return; }
    if (welcomeRevealed || animating) return;
    const raf = requestAnimationFrame(() => {
      if (promptAnimating.value) return;
      setWelcomeRevealed(true);
    });
    return () => cancelAnimationFrame(raf);
  }, [animating, showWelcome, welcomeRevealed]);

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
              onScrollUp={scrollToTop}
              onScrollDown={scrollToBottom}
            />
          )}
        </>
      )}
    </div>
  );
}

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
