import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { useWidgetCell } from '../widgets/useWidgetCell';
import { focusedThreadId, threadMap, stepsExpanded, detailsExpanded, collapsedExchanges, activeStreamingBuffer, threadsLoaded, awaitedThreadId, promptAnimating, revealOnFocus, connectionStatus, scaledDurationMs, effectiveThreadStatus, isMidTurn } from '../../store/store';
import { getThreadEventsBump } from '../../store/threadActivity';
import { unfocusThread } from '../../store/actions/threads';
import { loadThreadEvents, loadOlderThreadEvents, ensureWholeThreadLoaded, forceRetryThreadEvents, retryThreadEvents, threadHistoryReadInFlight, threadLoadInFlightMs } from '../../store/actions/thread-loading';
import { checkConnection } from '../../store/actions/connection';
import { gatewayPickerHref } from '../../utils/basePath';
import { replaceDocument } from '../../utils/documentNavigation';
import { rebuildCorruptedThreadEvents } from '../../store/actions/thread-event-repair';
import { useScrollObservers, renderExchanges, ScrollControls } from './CreateThreadView';
import { onClassifiedScroll } from './classifiedScroll';
import type { ScrollClass } from './scrollState';
import { StoppedChildNotice } from './StoppedChildNotice';
import { ContinuedFromNotice, ContinuedInNotice } from './SuccessorNotice';
import { currentCacheCards } from '../../store/actions/preferences';
import { CacheWarmLine, cacheWarmLineEligible } from './CacheKeepAlive';
import { sideQuestionsFor } from '../../store/sideQuestions';
import { threadVisualStatus } from '../shared/threadVisualStatus';
import { ThreadTitleMenu, ThreadTitlePin } from './ThreadTitle';
import { MobileThreadTitleBar } from '../layout/MobileAppHeader';
import { computeExchanges, deepLinkTargetIndex, exchangeKey, drawnResponseEvents, hasContentEvents, turnBodyFolded, type Exchange } from '../../store/thread-events';
import { rowsDrawnByClamp } from '../../store/event-rendering';
import { statusLabel } from '../../store/exchange-status';
import { showScrollToBottom, notAtTop, scrollToBottomAnimated, scrollToTop, hasPendingEventScroll, isScrollHeld, holdAcrossRelayout, onScrollHoldReleased, scrollHoldReleased, deepLinkTarget, scrolledFromTop } from './scrollState';
import { isElementVisible } from '../../utils/elementVisibility';
import { EMPTY_FILL_LEDGER, WHOLE_THREAD, chargeFillRound, drawnRowsInWindow, fillRoundAllowed, settleFillLedger, anythingAbove, atScrollTop, clampedScrollTop, scrollAsksForOlder, canSeedRenderWindow, deepLinkEdge, edgeHasMoreAbove, edgeMustReachRow, exchangeRenderCost, readingChaseAction, READING_CHASE_PAGE_SIZE, expandWindowEdge, fillAction, reseedOnReopen, seedWindowEdge, UPWARD_SCROLL_KEYS, WINDOW_EXPAND_MARGIN_PX, scrollToTopNeedsRenderAll, type FillLedger, type RowsAt, type StoredWindowKind, type WindowEdge } from './threadWindow';
import { transcriptScrolls } from './transcriptScrolls';
import { anchorTargetTop, readScrollAnchor, type ScrollAnchor } from './scrollAnchor';
import { useScrollMemory, readSavedScroll, type SavedScroll } from '../../hooks/useScrollMemory';
import { threadScrollKey } from '../../store/savedScroll';
import { useThreadScrollIndicator } from '../../hooks/useThreadScrollIndicator';
import { useDelayedFlag, useLingeringFlag, useMinimumVisibleFlag } from '../../hooks/useDelayedLoading';
import { ThreadSkeleton } from './ThreadSkeleton';
import { wholeHistoryLoaded, showThreadSkeletonNow, threadIsLoadingNow, type ThreadLoadingState } from './threadSkeletonGate';
import { forceWebKitRepaint, forceWebKitRepaintBurst, createRepaintThrottle, repaintNudgeShift, settledScrollTop } from '../../utils/webkitRepaint';
import { isWebKit } from '../../utils/platform';
import { onPageResume } from '../../utils/pageResume';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { refreshClient } from '../../hooks/sw-update';
import { recordPerfSample } from '../../utils/perfQueue';
import { takeThreadOpenStart, takeThreadRerenderStart } from '../../utils/threadOpenMarks';
import { readRenderPhaseTotals } from '../../utils/renderPhaseTimers';
import { reportAbsentThreadRender, reportThreadRenderProbe } from '../../utils/threadRenderProbe';
import { postClientLog } from '../../utils/liveness';
import { publishScrollbarGutter } from '../../utils/scrollbarGutter';
import { FindBar } from '../shared/FindBar';
import { isEngineUnreachable } from '../../utils/connectionNotice';
import { transcriptRenderRequest } from '../../store/actions/transcriptRenderRequest';
import { lazyComponent } from '../../utils/lazyComponent';
import { viewportIsMobile } from '../../utils/viewport';

const ThreadUsersPanel = lazyComponent(() => import('./ThreadUsers').then((m) => m.ThreadUsersPanel));
import { ThreadTerminalButton } from './ThreadTerminalButton';
const ThreadUsersButton = lazyComponent(() => import('./ThreadUsers').then((m) => m.ThreadUsersButton));

const PERF_GROUP_THRESHOLD_MS = 20;

let lastRevealedThread: string | null = null;

const renderFloorByThread = new Map<string, StoredWindow>();

type StoredWindow = { kind: StoredWindowKind; edge: WindowEdge; key: string | null; nextKey: string | null };

function storeEdge(threadId: string, edge: WindowEdge, exchanges: readonly Exchange[], kind: StoredWindowKind = 'grown'): void {
    const at = exchanges[edge.exchange];
    const next = exchanges[edge.exchange + 1];
    renderFloorByThread.set(threadId, {
        kind,
        edge,
        key: at ? exchangeKey(at) : null,
        nextKey: next ? exchangeKey(next) : null,
    });
}

function storeRenderAll(threadId: string): void {
    renderFloorByThread.set(threadId, { kind: 'claimed', edge: WHOLE_THREAD, key: null, nextKey: null });
}

function storedEdge(threadId: string, exchanges: readonly Exchange[]): WindowEdge | undefined {
    const stored = renderFloorByThread.get(threadId);
    if (!stored) return undefined;
    const { edge, key, nextKey } = stored;
    if (key === null) return edge;
    const at = exchanges[edge.exchange];
    if (at && exchangeKey(at) === key) return edge;
    return anchorAfterBackfill(exchanges, { anchorKey: key, nextKey, rowsHidden: edge.rowsHidden }) ?? edge;
}

const fillLedgerByThread = new Map<string, FillLedger>();


let lastSeededVisit: { threadId: string; owner: object } | null = null;

type PendingExpand = { prevScrollHeight: number; prevScrollTop: number } | null;

function currentEdge(
    threadId: string,
    exchanges: readonly Exchange[],
    costs: readonly number[],
    rowsAt: RowsAt,
): WindowEdge {
    return storedEdge(threadId, exchanges) ?? seedWindowEdge(costs, rowsAt);
}

function growRenderWindow(
    el: HTMLElement,
    threadId: string,
    exchanges: readonly Exchange[],
    costs: readonly number[],
    current: WindowEdge,
    rowsAt: RowsAt,
    pending: { current: PendingExpand },
    bump: () => void,
): boolean {
    const next = expandWindowEdge(current, costs, rowsAt);
    if (next.exchange === current.exchange && next.rowsHidden === current.rowsHidden) return false;
    pending.current = { prevScrollHeight: el.scrollHeight, prevScrollTop: settledScrollTop(el) };
    storeEdge(threadId, next, exchanges);
    bump();
    return true;
}

type HistoryHold = {
    anchorKey: string | null;
    nextKey: string | null;
    rowsHidden: number;
    prevScrollHeight: number;
    prevScrollTop: number;
    anchor: ScrollAnchor | null;
};

const historyHoldByThread = new Map<string, HistoryHold>();

export function holdTargetTop(el: HTMLElement, hold: HistoryHold): number {
    const anchored = hold.anchor ? anchorTargetTop(el, hold.anchor) : null;
    if (anchored !== null) return anchored;
    return hold.prevScrollTop + (el.scrollHeight - hold.prevScrollHeight);
}

export function readerOnlyHold(el: HTMLElement): HistoryHold {
    return {
        anchorKey: null,
        nextKey: null,
        rowsHidden: 0,
        prevScrollTop: settledScrollTop(el),
        prevScrollHeight: el.scrollHeight,
        anchor: readSettledAnchor(el),
    };
}

function readSettledAnchor(el: HTMLElement): ScrollAnchor | null {
    const anchor = readScrollAnchor(el);
    return anchor && { ...anchor, relTop: anchor.relTop + repaintNudgeShift(el) };
}

export function anchorAfterBackfill(
    exchanges: readonly Exchange[],
    pend: { anchorKey: string | null; nextKey: string | null; rowsHidden: number },
): WindowEdge | null {
    const found = exchanges.findIndex(e => exchangeKey(e) === pend.anchorKey);
    if (found >= 0) return { exchange: found, rowsHidden: pend.rowsHidden };
    if (!pend.nextKey) return null;
    const below = exchanges.findIndex(e => exchangeKey(e) === pend.nextKey);
    if (below < 0) return null;
    return { exchange: Math.max(0, below - 1), rowsHidden: 0 };
}

export type ReadingTarget = { kind: 'turn'; id: string } | { kind: 'row'; id: string };

export function readingTargetOf(record: SavedScroll | null): ReadingTarget | null {
    if (record?.kind === 'anchor' || record?.kind === 'line') return { kind: 'turn', id: record.eventId };
    if (record?.kind === 'row') return { kind: 'row', id: record.rowEventId };
    return null;
}

export function locateReadingTarget(
    exchanges: readonly Exchange[],
    target: ReadingTarget,
): { index: number; row: number } {
    if (target.kind === 'turn') {
        return { index: exchanges.findIndex((ex) => ex.userEvent._eventId === target.id), row: 0 };
    }
    for (let i = exchanges.length - 1; i >= 0; i--) {
        const ex = exchanges[i];
        const holds = ex.userEvent._eventId === target.id
            || ex.steps.some(({ event }) => event._eventId === target.id);
        if (!holds) continue;
        const names = (s: { call_event_id?: string; result_event_id?: string }) =>
            s.call_event_id === target.id || s.result_event_id === target.id;
        const row = drawnResponseEvents(ex, false, true, currentCacheCards()).findIndex(
            (r) => r.type === 'step' && (names(r) || !!r.children?.some(names)),
        );
        return { index: i, row: Math.max(0, row) };
    }
    return { index: -1, row: 0 };
}

function requestBackfill(
    el: HTMLElement,
    threadId: string,
    exchanges: readonly Exchange[],
    edge: WindowEdge,
    onSettled: (added: boolean) => void,
    pageSize?: number,
): boolean {
    if (threadHistoryReadInFlight(threadId)) return false;
    const anchor = exchanges[edge.exchange];
    const next = exchanges[edge.exchange + 1];
    historyHoldByThread.set(threadId, {
        anchorKey: anchor ? exchangeKey(anchor) : null,
        nextKey: next ? exchangeKey(next) : null,
        rowsHidden: edge.rowsHidden,
        prevScrollHeight: el.scrollHeight,
        prevScrollTop: settledScrollTop(el),
        anchor: null,
    });
    settleOn(loadOlderThreadEvents(threadId, pageSize, () => scrollHoldReleased(el)), threadId, onSettled);
    return true;
}

function settleHistoryRead(threadId: string, added: boolean, onSettled: (added: boolean) => void): void {
    if (!added) historyHoldByThread.delete(threadId);
    if (focusedThreadId.value !== threadId) return;
    onSettled(added);
}

function settleOn(read: Promise<boolean>, threadId: string, onSettled: (added: boolean) => void): void {
    void read.then(
        added => settleHistoryRead(threadId, added, onSettled),
        () => settleHistoryRead(threadId, false, onSettled),
    );
}

const linkPagingGaveUp = new Set<string>();

function requestLinkPage(el: HTMLElement, threadId: string, onSettled: (added: boolean) => void): void {
    if (linkPagingGaveUp.has(threadId)) return;
    historyHoldByThread.set(threadId, readerOnlyHold(el));
    settleOn(loadOlderThreadEvents(threadId, undefined, () => scrollHoldReleased(el)), threadId, (added) => {
        if (!added) linkPagingGaveUp.add(threadId);
        onSettled(added);
    });
}

const EMPTY_THREAD_RETRY_DELAYS = [500, 2000, 5000];

const LOST_LOAD_RETRY_MS = 2000;

const STALLED_LOAD_RETRY_MS = 30_000;

const SETTLE_PROBE_DELAY_MS = 1500;

type ThreadRetryRef = { id: string; count: number } | null;

function hasExhaustedRetries(ref: { current: ThreadRetryRef }, threadId: string, max: number): boolean {
    const r = ref.current;
    return !!r && r.id === threadId && r.count >= max;
}

function incrementRetry(ref: { current: ThreadRetryRef }, threadId: string): void {
    if (!ref.current || ref.current.id !== threadId) {
        ref.current = { id: threadId, count: 1 };
    } else {
        ref.current.count++;
    }
}

export function shouldRevealThread(threadId: string | null | undefined, animating: boolean, hasContent: boolean): boolean {
    if (!threadId || animating || !hasContent) return false;
    if (threadId === lastRevealedThread) return false;
    return true;
}

export function commitReveal(threadId: string) {
    lastRevealedThread = threadId;
}

export function resetRevealTracking() {
    lastRevealedThread = null;
}

const RELOAD_TIMEOUT = 8000;

const STREAM_REPAINT_THROTTLE_MS = 200;

export type EmptyReason =
    | { kind: 'loading'; threadId: string }
    | { kind: 'animating' }
    | { kind: 'failed'; threadId: string }
    | { kind: 'corrupt'; threadId: string }
    | { kind: 'disconnected'; threadId: string }
    | { kind: 'working' }
    | { kind: 'empty' };

export function emptyReason(
    animating: boolean,
    historyLoaded: boolean,
    eventsLoadFailed: boolean,
    hasContent: boolean,
    threadId: string,
    disconnected: boolean,
    turnInFlight: boolean = false,
): EmptyReason {
    if (animating) return { kind: 'animating' };
    if (eventsLoadFailed) return { kind: 'failed', threadId };
    if (historyLoaded && hasContent) return { kind: 'corrupt', threadId };
    if (historyLoaded && turnInFlight) return { kind: 'working' };
    if (historyLoaded) return { kind: 'empty' };
    if (disconnected) return { kind: 'disconnected', threadId };
    return { kind: 'loading', threadId };
}

function ThreadEmptyState({ reason }: { reason: EmptyReason }) {
    const showReload = useDelayedFlag(reason.kind === 'loading', RELOAD_TIMEOUT);
    return threadEmptyStateBody(reason, showReload);
}

export function threadEmptyStateBody(reason: EmptyReason, showReload: boolean) {
    switch (reason.kind) {
        case 'animating':
            return null;
        case 'failed':
        case 'corrupt': {
            const message = reason.kind === 'failed' ? 'Failed to load messages' : 'Messages could not be displayed';
            return (
                <div class="thread-empty-state thread-empty-error">
                    <p>{message}</p>
                    <button class="action-btn" onClick={() => retryThreadEvents(reason.threadId)}>Retry</button>
                    <button class="thread-empty-reload" onClick={() => refreshClient()}>Reload page</button>
                </div>
            );
        }
        case 'disconnected': {
            const pickerHref = gatewayPickerHref();
            return (
                <div class="thread-empty-state thread-empty-error">
                    <p>Can't reach this workspace</p>
                    <button class="action-btn" onClick={() => { void checkConnection(); retryThreadEvents(reason.threadId); }}>Retry</button>
                    {pickerHref && (
                        <button class="thread-empty-reload" onClick={() => replaceDocument(pickerHref)}>
                            Back to workspaces
                        </button>
                    )}
                </div>
            );
        }
        case 'working':
            return (
                <div class="thread-empty-state">
                    <p class="running-shimmer">{statusLabel('pending', false).label}</p>
                </div>
            );
        case 'empty':
            return (
                <div class="thread-empty-state">
                    <p>No messages in this thread</p>
                </div>
            );
        case 'loading':
            return showReload ? (
                <div class="thread-empty-state">
                    <button class="thread-empty-reload" onClick={() => refreshClient()}>
                        Taking too long? Tap to reload
                    </button>
                </div>
            ) : null;
    }
}

const SKELETON_FADE_OUT_MS = 200;
const SKELETON_FADE_SLACK_MS = 50;

export function skeletonOverlayClass(show: boolean): string {
    return show ? 'thread-skeleton-overlay' : 'thread-skeleton-overlay is-fading';
}

function ThreadSkeletonOverlay({ show }: { show: boolean }) {
    const held = useMinimumVisibleFlag(show);
    const mounted = useLingeringFlag(held, scaledDurationMs(SKELETON_FADE_OUT_MS) + SKELETON_FADE_SLACK_MS);
    if (!mounted) return null;
    return (
        <div class={skeletonOverlayClass(held)} aria-hidden="true">
            <ThreadSkeleton />
        </div>
    );
}

export function ThreadView() {
    const threadId = focusedThreadId.value;

    const eventThread = threadId ? threadMap.value.get(threadId) : undefined;
    const eventsLoaded = eventThread?.eventsLoaded ?? false;
    const eventsLoadFailed = eventThread?.eventsLoadFailed ?? false;
    const hasOlderEvents = eventThread?.hasOlderEvents === true;
    const historyLoaded = wholeHistoryLoaded(eventsLoaded, hasOlderEvents);

    const animating = promptAnimating.value;
    const eventCount = eventThread?.events.size ?? 0;
    const pendingCount = eventThread?.pendingUserMessages.length ?? 0;
    const hasPending = pendingCount > 0;
    const hasContent = eventsLoaded || eventCount > 0 || hasPending;

    const eventsBump = threadId ? getThreadEventsBump(threadId) : 0;
    const groupSampleRef = useRef<{ groupMs: number; eventCount: number; exchangeCount: number } | null>(null);
    const exchanges = useMemo(
        () => {
            if (!(hasContent && !animating && eventThread)) return [];
            const t0 = performance.now();
            const result = computeExchanges(eventThread);
            groupSampleRef.current = {
                groupMs: performance.now() - t0,
                eventCount,
                exchangeCount: result.length,
            };
            return result;
        },
        [threadId, eventCount, pendingCount, hasContent, animating, eventsBump],
    );
    const lastRenderedExchangesLenRef = useRef(0);
    lastRenderedExchangesLenRef.current = exchanges.length;
    const streamingBuffer = animating ? '' : activeStreamingBuffer.value;

    useEffect(() => {
        const s = groupSampleRef.current;
        if (s && s.groupMs > PERF_GROUP_THRESHOLD_MS && threadId) {
            recordPerfSample('group', {
                threadId,
                eventCount: s.eventCount,
                exchangeCount: s.exchangeCount,
                groupMs: Math.round(s.groupMs),
            });
        }
        groupSampleRef.current = null;
    }, [exchanges]);

    useLayoutEffect(() => {
        if (!threadId || exchanges.length === 0) return;
        const base = takeThreadOpenStart(threadId);
        if (base === undefined) return;
        const exchangeCount = exchanges.length;
        const events = eventCount;
        requestAnimationFrame(() => {
            const totals = readRenderPhaseTotals();
            recordPerfSample('thread-render', {
                threadId,
                warm: base.warm,
                eventCount: events,
                exchangeCount,
                renderMs: Math.round(performance.now() - base.start),
                markdownMs: Math.round(totals.markdownMs - base.md),
                linkifyMs: Math.round(totals.linkifyMs - base.link),
            });
        });
    }, [threadId, exchanges.length]);

    useLayoutEffect(() => {
        if (!threadId || exchanges.length === 0) return;
        const mark = takeThreadRerenderStart(threadId);
        if (mark === undefined) return;
        const exchangeCount = exchanges.length;
        const events = eventCount;
        requestAnimationFrame(() => {
            const totals = readRenderPhaseTotals();
            recordPerfSample('thread-rerender', {
                threadId,
                cause: mark.cause,
                eventCount: events,
                exchangeCount,
                rerenderMs: Math.round(performance.now() - mark.start),
                markdownMs: Math.round(totals.markdownMs - mark.md),
                linkifyMs: Math.round(totals.linkifyMs - mark.link),
            });
        });
    });

    const areaRef = useRef<HTMLDivElement>(null);
    const [paneWrap, setPaneWrap] = useState<HTMLDivElement | null>(null);
    useWidgetCell(paneWrap);
    const visitOwner = useRef({}).current;

    const [winTick, bumpWin] = useState(0);
    const [historyFolded, setHistoryFolded] = useState(0);
    const [historyReadSettled, setHistoryReadSettled] = useState(0);
    const onHistoryRead = (added: boolean) => {
        if (added) setHistoryFolded(n => n + 1);
        setHistoryReadSettled(n => n + 1);
    };
    const bumpWindow = () => bumpWin(n => n + 1);
    const exchangeCosts = useMemo(() => exchanges.map(exchangeRenderCost), [exchanges]);
    const showSteps = stepsExpanded.value;
    const showDetails = detailsExpanded.value;
    const folds = collapsedExchanges.value;
    const cacheCards = currentCacheCards();
    const rowsDrawnAt: RowsAt = (index) => {
        const exchange = exchanges[index];
        if (!exchange || !threadId) return [];
        return rowsDrawnByClamp(drawnResponseEvents(exchange, false, true, cacheCards), {
            showSteps,
            showDetails,
            folded: turnBodyFolded(folds, threadId, exchange),
        });
    };
    const canSeedWindow = canSeedRenderWindow({
        hasExchanges: exchanges.length > 0,
        eventsLoaded,
        eventsLoadFailed,
    });
    const linkTarget = deepLinkTarget.value;
    if (threadId && canSeedWindow) {
        const firstThisVisit = lastSeededVisit?.threadId !== threadId;
        lastSeededVisit = { threadId, owner: visitOwner };
        const stored = renderFloorByThread.get(threadId);
        if (!stored || (firstThisVisit && reseedOnReopen(stored.kind))) {
            storeEdge(threadId, seedWindowEdge(exchangeCosts, rowsDrawnAt), exchanges);
        }
        if (linkTarget) {
            const now = currentEdge(threadId, exchanges, exchangeCosts, rowsDrawnAt);
            const claimed = deepLinkEdge(now, deepLinkTargetIndex(exchanges, linkTarget));
            if (claimed !== now) storeEdge(threadId, claimed, exchanges, 'claimed');
        }
    }
    const edge = threadId ? currentEdge(threadId, exchanges, exchangeCosts, rowsDrawnAt) : WHOLE_THREAD;
    const renderFromIndex = edge.exchange;

    const renderRequest = transcriptRenderRequest.value;
    useEffect(() => {
        if (!threadId || renderRequest?.threadId !== threadId) return;
        const index = exchanges.findIndex((ex) => exchangeKey(ex) === renderRequest.key);
        if (index < 0) return;
        const now = currentEdge(threadId, exchanges, exchangeCosts, rowsDrawnAt);
        if (index > now.exchange || (index === now.exchange && now.rowsHidden === 0)) return;
        storeEdge(threadId, { exchange: index, rowsHidden: 0 }, exchanges);
        bumpWin(n => n + 1);
    }, [threadId, renderRequest, exchanges]);

    useEffect(() => {
        if (!threadId || !hasOlderEvents) return;
        const el = areaRef.current;
        if (!el) return;
        if (threadHistoryReadInFlight(threadId)) {
            if (!historyHoldByThread.get(threadId)) {
                historyHoldByThread.set(threadId, readerOnlyHold(el));
            }
            return;
        }
        if (!linkTarget || deepLinkTargetIndex(exchangesRef.current, linkTarget) >= 0) return;
        requestLinkPage(el, threadId, onHistoryRead);
    }, [threadId, linkTarget, hasOlderEvents, historyReadSettled]);


    const exchangeCostsRef = useRef<number[]>([]);
    exchangeCostsRef.current = exchangeCosts;
    const rowsDrawnAtRef = useRef(rowsDrawnAt);
    rowsDrawnAtRef.current = rowsDrawnAt;
    const exchangesRef = useRef<readonly Exchange[]>(exchanges);
    exchangesRef.current = exchanges;
    const edgeKey = `${edge.exchange}:${edge.rowsHidden}`;
    const pendingExpandRef = useRef<PendingExpand>(null);
    const anchorTargetRef = useRef<ReadingTarget | null>(null);
    const readingChasesRef = useRef(0);
    const pendingScrollTopRef = useRef(false);

    const shouldReveal = shouldRevealThread(threadId, animating, hasContent);

    const isUp = showScrollToBottom.value;
    const isNotAtTop = notAtTop.value;

    const [indicatorTrack, setIndicatorTrack] = useState<HTMLDivElement | null>(null);
    const [indicatorThumb, setIndicatorThumb] = useState<HTMLDivElement | null>(null);
    useThreadScrollIndicator({
        scrollerRef: areaRef,
        track: indicatorTrack,
        thumb: indicatorThumb,
    });

    useLayoutEffect(() => { publishScrollbarGutter(); }, [eventThread !== undefined]);

    useLayoutEffect(() => {
        if (!shouldReveal || !threadId) return;
        commitReveal(threadId);
        if (!revealOnFocus.peek()) return;
        revealOnFocus.value = false;
        document.querySelectorAll('.thread-view').forEach(el => {
            el.classList.remove('revealing');
            void (el as HTMLElement).offsetHeight;
            el.classList.add('revealing');
        });
    }, [threadId, animating, hasContent]);

    useEffect(() => resetRevealTracking, []);

    const threadInMap = !!eventThread;
    useEffect(() => {
        if (threadId && threadInMap && !eventsLoaded) {
            void loadThreadEvents(threadId);
        }
    }, [threadId, threadInMap, eventsLoaded]);

    useEffect(() => {
        if (threadId && threadInMap && awaitedThreadId.value === threadId) {
            awaitedThreadId.value = null;
        }
    }, [threadId, threadInMap]);

    const retryCountRef = useRef<ThreadRetryRef>(null);
    useEffect(() => {
        if (!threadId || !threadInMap || !eventsLoaded) return;
        if (hasExhaustedRetries(retryCountRef, threadId, EMPTY_THREAD_RETRY_DELAYS.length)) return;
        const thread = threadMap.value.get(threadId);
        if (!thread || thread.events.size > 0 || thread.pendingUserMessages.length > 0) return;
        const attempt = retryCountRef.current?.id === threadId ? retryCountRef.current.count : 0;
        const delay = EMPTY_THREAD_RETRY_DELAYS[attempt] ?? EMPTY_THREAD_RETRY_DELAYS[EMPTY_THREAD_RETRY_DELAYS.length - 1];
        const timer = setTimeout(() => {
            incrementRetry(retryCountRef, threadId);
            const t = threadMap.value.get(threadId);
            if (t && t.events.size === 0 && t.pendingUserMessages.length === 0) {
                t.eventsLoaded = false;
                void loadThreadEvents(threadId);
            }
        }, delay);
        return () => clearTimeout(timer);
    }, [threadId, threadInMap, eventsLoaded]);

    useEffect(() => {
        if (!threadId || !threadInMap || eventsLoaded || eventsLoadFailed) return;
        let timer: ReturnType<typeof setTimeout>;
        const ask = () => {
            const inFlightMs = threadLoadInFlightMs(threadId);
            if (inFlightMs === null || inFlightMs >= STALLED_LOAD_RETRY_MS) {
                forceRetryThreadEvents(threadId);
                return;
            }
            timer = setTimeout(ask, STALLED_LOAD_RETRY_MS - inFlightMs);
        };
        timer = setTimeout(ask, LOST_LOAD_RETRY_MS);
        return () => clearTimeout(timer);
    }, [threadId, threadInMap, eventsLoaded, eventsLoadFailed]);

    const forceRepaint = () => forceWebKitRepaint(areaRef.current);
    const forceRepaintBurst = () => forceWebKitRepaintBurst(areaRef.current);

    const hasExchanges = exchanges.length > 0;

    const loadingState: ThreadLoadingState = {
        hasExchanges,
        animating,
        eventsLoadFailed,
        historyLoaded,
        disconnected: isEngineUnreachable(connectionStatus.value),
    };
    const delayElapsed = useDelayedFlag(threadIsLoadingNow(loadingState));
    const showThreadSkeleton = showThreadSkeletonNow(loadingState, delayElapsed);
    const skeletonShownRef = useRef<string | null>(null);
    if (showThreadSkeleton && threadId) skeletonShownRef.current = threadId;

    const enteredThreadRef = useRef<string | null>(null);
    useLayoutEffect(() => {
        if (!threadId || !hasExchanges || enteredThreadRef.current === threadId) return;
        const el = areaRef.current;
        if (!el) return;
        enteredThreadRef.current = threadId;
        if (skeletonShownRef.current === threadId) return;
        el.classList.remove('content-entering');
        void el.offsetHeight;
        el.classList.add('content-entering');
    }, [threadId, hasExchanges]);

    const savedScrollKey = threadId ? threadScrollKey(threadId) : null;

    useEffect(() => {
        if (threadId) {
            return forceRepaintBurst();
        }
    }, [threadId, eventsLoaded, hasExchanges]);

    useEffect(() => onPageResume(forceRepaint), []);

    const streamRepaintGate = useMemo(() => createRepaintThrottle(STREAM_REPAINT_THROTTLE_MS), []);
    useEffect(() => {
        if (!hasExchanges) return;
        streamRepaintGate.request(performance.now(), forceRepaint);
    }, [eventsBump, hasExchanges]);
    useEffect(() => () => streamRepaintGate.cancel(), [streamRepaintGate]);

    const watchdogRef = useRef<ThreadRetryRef>(null);
    useEffect(() => {
        if (!threadId || !historyLoaded || !eventThread || exchanges.length > 0) return;
        if (!hasContentEvents(eventThread.events)) return;
        if (hasExhaustedRetries(watchdogRef, threadId, 2)) return;
        const timer = setTimeout(() => {
            incrementRetry(watchdogRef, threadId);
            postClientLog('render', 'rebuild_corrupted_thread', {
                thread_id: threadId,
                event_count: eventThread.events.size,
                channel: eventThread.meta.channel,
            });
            rebuildCorruptedThreadEvents(threadId);
        }, 300);
        return () => clearTimeout(timer);
    }, [threadId, historyLoaded, exchanges.length, eventCount]);

    useEffect(() => {
        if (!threadId || !isWebKit()) return;
        const timer = setTimeout(() => {
            const thread = threadMap.value.get(threadId);
            if (!thread) {
                reportAbsentThreadRender(threadId);
                return;
            }
            const fresh = computeExchanges(thread);
            reportThreadRenderProbe({
                threadId,
                channel: thread.meta.channel,
                isCodingAgent: thread.meta.channel === 'claude_code',
                renderedExchangesLen: lastRenderedExchangesLenRef.current,
                freshExchangesLen: fresh.length,
                eventCount: thread.events.size,
                eventsLoaded: thread.eventsLoaded,
                hasContentEvents: hasContentEvents(thread.events),
                animating: promptAnimating.value,
                contentChildCount: areaRef.current?.childElementCount ?? null,
                contentScrollHeight: areaRef.current?.scrollHeight ?? null,
            });
            forceWebKitRepaintBurst(areaRef.current);
        }, SETTLE_PROBE_DELAY_MS);
        return () => clearTimeout(timer);
    }, [threadId]);

    useScrollObservers(areaRef, true);

    useScrollMemory(
        areaRef,
        savedScrollKey,
        {
            paused: !eventsLoaded,
            resetOnEmpty: true,
            followsLiveEdge: true,
            anchorsToContent: true,
            shouldRestore: () => !hasPendingEventScroll(),
            onRestoreSettled: () => { anchorTargetRef.current = null; },
            restoreIsBusy: () => (threadId ? threadHistoryReadInFlight(threadId) : false),
        },
    );

    useEffect(() => {
        const el = areaRef.current;
        if (!el || !threadId) return;
        const reachForOlder = () => {
            if (pendingExpandRef.current) return;
            const costs = exchangeCostsRef.current;
            const rows = rowsDrawnAtRef.current;
            const current = currentEdge(threadId, exchangesRef.current, costs, rows);
            if (!edgeHasMoreAbove(current)) {
                requestBackfill(el, threadId, exchangesRef.current, current, onHistoryRead);
                return;
            }
            if (isScrollHeld(el)) {
                owesGrow = true;
                return;
            }
            growRenderWindow(el, threadId, exchangesRef.current, costs, current, rows, pendingExpandRef, bumpWindow);
        };
        let owesGrow = false;
        let releaseFrame = 0;
        const unsubRelease = onScrollHoldReleased((released) => {
            if (released !== el || !owesGrow) return;
            owesGrow = false;
            cancelAnimationFrame(releaseFrame);
            releaseFrame = requestAnimationFrame(() => {
                if (!el.isConnected || el.scrollTop > WINDOW_EXPAND_MARGIN_PX) return;
                reachForOlder();
            });
        });
        let lastTop = clampedScrollTop(el);
        const onScroll = (scroll: ScrollClass) => {
            const inFlight = historyHoldByThread.get(threadId);
            if (inFlight) {
                inFlight.prevScrollTop = settledScrollTop(el);
                inFlight.prevScrollHeight = el.scrollHeight;
                if (inFlight.anchor) inFlight.anchor = readSettledAnchor(el) ?? inFlight.anchor;
            }
            const previousTop = lastTop;
            lastTop = clampedScrollTop(el);
            if (scroll.navigation || scroll.onLastWrite) return;
            if (!scrollAsksForOlder(previousTop, lastTop)) return;
            reachForOlder();
        };
        let lastTouchY: number | null = null;
        const reachIfUp = (reachesUp: boolean) => {
            if (!reachesUp || !atScrollTop(el)) return;
            if (!transcriptScrolls(el)) return;
            reachForOlder();
        };
        const onWheel = (e: WheelEvent) => reachIfUp(e.deltaY < 0);
        const onTouchStart = (e: TouchEvent) => { lastTouchY = e.touches[0]?.clientY ?? null; };
        const onTouchMove = (e: TouchEvent) => {
            const y = e.touches[0]?.clientY ?? null;
            reachIfUp(lastTouchY !== null && y !== null && y > lastTouchY);
            lastTouchY = y;
        };
        const onTouchEnd = () => { lastTouchY = null; };
        const onKeyDown = (e: KeyboardEvent) => {
            if (e.defaultPrevented) return;
            if (e.metaKey || e.ctrlKey || e.altKey) return;
            const space = e.key === ' ' || e.key === 'Spacebar';
            reachIfUp(space ? e.shiftKey : UPWARD_SCROLL_KEYS.has(e.key));
        };
        const stopScroll = onClassifiedScroll(el, onScroll);
        el.addEventListener('wheel', onWheel, { passive: true });
        el.addEventListener('touchstart', onTouchStart, { passive: true });
        el.addEventListener('touchmove', onTouchMove, { passive: true });
        el.addEventListener('touchend', onTouchEnd, { passive: true });
        el.addEventListener('keydown', onKeyDown, { passive: true });
        return () => {
            unsubRelease();
            cancelAnimationFrame(releaseFrame);
            stopScroll();
            el.removeEventListener('wheel', onWheel);
            el.removeEventListener('keydown', onKeyDown);
            el.removeEventListener('touchstart', onTouchStart);
            el.removeEventListener('touchmove', onTouchMove);
            el.removeEventListener('touchend', onTouchEnd);
        };
    }, [threadId, eventsLoaded]);

    useLayoutEffect(() => {
        const el = areaRef.current;
        const pend = pendingExpandRef.current;
        if (!el || !pend) return;
        pendingExpandRef.current = null;
        holdAcrossRelayout(el, pend.prevScrollTop + (el.scrollHeight - pend.prevScrollHeight));
    }, [edgeKey, winTick]);

    useEffect(() => () => {
        if (threadId) historyHoldByThread.delete(threadId);
        if (threadId) linkPagingGaveUp.delete(threadId);
        pendingScrollTopRef.current = false;
        if (lastSeededVisit?.threadId === threadId && lastSeededVisit.owner === visitOwner) {
            lastSeededVisit = null;
        }
    }, [threadId]);

    useLayoutEffect(() => {
        if (!threadId) return;
        const pend = historyHoldByThread.get(threadId);
        const el = areaRef.current;
        if (!pend || !el) return;
        historyHoldByThread.delete(threadId);
        const hold = isElementVisible(el) ? pend : null;
        const anchored = anchorAfterBackfill(exchanges, pend);
        if (!anchored) {
            if (hold) holdAcrossRelayout(el, holdTargetTop(el, hold));
            return;
        }
        storeEdge(threadId, anchored, exchanges);
        const grew = !isScrollHeld(el) && growRenderWindow(
            el, threadId, exchanges, exchangeCostsRef.current, anchored,
            rowsDrawnAtRef.current, pendingExpandRef, bumpWindow,
        );
        pendingExpandRef.current = hold && {
            prevScrollTop: hold.prevScrollTop,
            prevScrollHeight: hold.prevScrollHeight,
        };
        if (!grew) bumpWindow();
    }, [threadId, historyFolded]);

    useLayoutEffect(() => {
        if (!pendingScrollTopRef.current) return;
        pendingScrollTopRef.current = false;
        scrollToTop();
    }, [edgeKey, winTick]);

    const fillWindow = () => {
        const el = areaRef.current;
        if (!el || !threadId || !(eventsLoaded || eventsLoadFailed)) return;
        if (pendingExpandRef.current) return;
        const costs = exchangeCostsRef.current;
        const rows = rowsDrawnAtRef.current;
        const current = currentEdge(threadId, exchangesRef.current, costs, rows);
        if (!edgeHasMoreAbove(current) && !hasOlderEvents) return;
        if (!isElementVisible(el)) return;
        const owed = fillAction(el, current, hasOlderEvents);
        if (owed === 'none') return;
        const reading = {
            drawn: drawnRowsInWindow(current, exchangesRef.current.length, rows),
            floor: eventThread?.historyFloor?.sequence ?? Infinity,
        };
        const stored = fillLedgerByThread.get(threadId) ?? EMPTY_FILL_LEDGER;
        const ledger = threadHistoryReadInFlight(threadId) ? stored : settleFillLedger(stored, reading);
        fillLedgerByThread.set(threadId, ledger);
        if (owed === 'page') {
            if (!fillRoundAllowed(ledger, 'page')) return;
            if (threadHistoryReadInFlight(threadId)) return;
            if (requestBackfill(el, threadId, exchangesRef.current, current, onHistoryRead)) {
                fillLedgerByThread.set(threadId, chargeFillRound(ledger, 'page', reading));
            }
            return;
        }
        if (!fillRoundAllowed(ledger, 'grow')) return;
        if (growRenderWindow(el, threadId, exchangesRef.current, costs, current, rows, pendingExpandRef, bumpWindow)) {
            fillLedgerByThread.set(threadId, chargeFillRound(ledger, 'grow', reading));
        }
    };
    const fillWindowRef = useRef(fillWindow);
    fillWindowRef.current = fillWindow;

    useLayoutEffect(() => { fillWindowRef.current(); },
        [threadId, canSeedWindow, eventsLoaded, eventsLoadFailed, hasOlderEvents,
            edgeKey, exchanges.length, historyFolded, winTick, showSteps, showDetails, folds]);

    useLayoutEffect(() => {
        const record = threadId ? readSavedScroll(threadScrollKey(threadId)) : null;
        anchorTargetRef.current = readingTargetOf(record);
        readingChasesRef.current = 0;
    }, [threadId]);
    const reachAnchor = () => {
        const el = areaRef.current;
        const target = anchorTargetRef.current;
        if (!el || !threadId || !canSeedWindow || !eventsLoaded || !target) return;
        if (deepLinkTarget.value) return;
        if (pendingExpandRef.current) return;
        const costs = exchangeCostsRef.current;
        const rows = rowsDrawnAtRef.current;
        const current = currentEdge(threadId, exchanges, costs, rows);
        const { index, row } = locateReadingTarget(exchanges, target);
        if (index < 0) {
            const action = readingChaseAction({
                readInFlight: threadHistoryReadInFlight(threadId),
                paneMeasurable: isElementVisible(el),
                hasOlderEvents,
                chasesSpent: readingChasesRef.current,
            });
            if (action === 'chase'
                && requestBackfill(el, threadId, exchanges, current, onHistoryRead, READING_CHASE_PAGE_SIZE)) {
                readingChasesRef.current += 1;
            }
            if (action === 'give-up') anchorTargetRef.current = null;
            return;
        }
        if (!edgeMustReachRow(current, index, row)) {
            anchorTargetRef.current = null;
            return;
        }
        if (!isElementVisible(el)) return;
        if (!growRenderWindow(el, threadId, exchanges, costs, current, rows, pendingExpandRef, bumpWindow)) {
            anchorTargetRef.current = null;
        }
    };
    const reachAnchorRef = useRef(reachAnchor);
    reachAnchorRef.current = reachAnchor;
    useLayoutEffect(() => {
        const frame = requestAnimationFrame(() => reachAnchorRef.current());
        return () => cancelAnimationFrame(frame);
    }, [threadId, canSeedWindow, edgeKey, exchanges.length, historyFolded]);

    useLayoutEffect(() => {
        const el = areaRef.current;
        if (!el || !threadId) return;
        const observer = new ResizeObserver(() => {
            fillWindowRef.current();
            reachAnchorRef.current();
        });
        observer.observe(el);
        return () => observer.disconnect();
    }, [threadId, threadInMap]);

    if (!threadId) return null;

    if (!eventThread) {
        if (threadsLoaded.value && awaitedThreadId.value !== threadId) {
            unfocusThread({ revealPane: false });
        }
        const waitingReason: EmptyReason = isEngineUnreachable(connectionStatus.value)
            ? { kind: 'disconnected', threadId }
            : { kind: 'loading', threadId };
        return (
            <div class="thread-view">
                <div class="thread-content-wrap" key="wrap">
                    <div class="thread-content visible" key="content" data-native-context-menu>
                        <ThreadEmptyState key={threadId} reason={waitingReason} />
                    </div>
                    <ThreadSkeletonOverlay key="skeleton" show={showThreadSkeleton} />
                </div>
            </div>
        );
    }

    const hasFeed = exchanges.length > 0;
    const mobile = viewportIsMobile.value;
    const usersPanel = <ThreadUsersPanel threadId={eventThread.meta.id} />;
    return (
        <div class="thread-view">
            <DesktopThreadTitleBar threadId={eventThread.meta.id} />
            {!mobile && usersPanel}
            <FindBar key="find" surface="thread" scope={`thread:${eventThread.meta.id}`} placeholder="Find in thread" />
            <div ref={setPaneWrap} class="thread-content-wrap has-scroll-indicator" key="wrap">
                <div class="thread-content visible" key="content" ref={areaRef} tabIndex={0} role="region" aria-label="Thread transcript" data-native-context-menu>
                    <MobileThreadTitleBar usersPanel={mobile ? usersPanel : undefined} />

                    {hasFeed ? (
                        <div class="thread-feed" key="feed">
                            {renderFromIndex === 0 && <ContinuedFromNotice meta={eventThread.meta} />}
                            {renderExchanges(
                                exchanges, threadId!, streamingBuffer, renderFromIndex, edge.rowsHidden,
                                sideQuestionsFor(threadId!),
                            )}
                            <StoppedChildNotice meta={eventThread.meta} />
                            <ContinuedInNotice meta={eventThread.meta} />
                            <CacheWarmLine
                                key={threadId}
                                threadId={threadId!}
                                eligible={currentCacheCards() && cacheWarmLineEligible(eventThread)}
                                eventCount={eventThread.events.size}
                            />
                        </div>
                    ) : (
                        <ThreadEmptyState key={threadId} reason={emptyReason(animating, historyLoaded, eventsLoadFailed, hasContentEvents(eventThread.events), threadId!, isEngineUnreachable(connectionStatus.value), isMidTurn(effectiveThreadStatus(eventThread)))} />
                    )}
                </div>
                <ThreadSkeletonOverlay key="skeleton" show={showThreadSkeleton} />
                <div class="thread-scroll-indicator" ref={setIndicatorTrack} aria-hidden="true">
                    <div class="thread-scroll-thumb" ref={setIndicatorThumb} />
                </div>
                <ScrollControls
                    showUp={anythingAbove(isNotAtTop, edge, hasOlderEvents)}
                    showDown={isUp}
                    onScrollUp={() => {
                        if (threadId && scrollToTopNeedsRenderAll(edge, hasOlderEvents)) {
                            pendingScrollTopRef.current = true;
                            storeRenderAll(threadId);
                            const el = areaRef.current;
                            void ensureWholeThreadLoaded(threadId, el ? () => scrollHoldReleased(el) : undefined).then((added) => {
                                settleHistoryRead(threadId, added, onHistoryRead);
                                if (focusedThreadId.value !== threadId) return;
                                pendingScrollTopRef.current = true;
                                bumpWin(n => n + 1);
                            }, () => settleHistoryRead(threadId, false, onHistoryRead));
                            bumpWin(n => n + 1);
                        } else {
                            scrollToTop();
                        }
                    }}
                    onScrollDown={scrollToBottomAnimated}
                />
            </div>
        </div>
    );
}

export function DesktopThreadTitleBar({ threadId }: { threadId: string }) {
    const thread = threadMap.value.get(threadId);
    if (!thread) return null;
    const title = threadDisplayTitle(thread);
    return (
        <div class={`thread-view-header${scrolledFromTop.value ? ' scrolled' : ''}`}>
            <ThreadTitleMenu thread={thread} title={title} status={threadVisualStatus(thread)} />
            <ThreadTitlePin thread={thread} />
            <ThreadTerminalButton threadId={threadId} />
            <ThreadUsersButton threadId={threadId} />
        </div>
    );
}
