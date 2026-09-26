import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { focusedThreadId, threadMap, stepsExpanded, detailsExpanded, collapsedExchanges, activeStreamingBuffer, threadsLoaded, awaitedThreadId, promptAnimating, revealOnFocus, connectionStatus, scaledDurationMs, effectiveThreadStatus, isMidTurn } from '../../store/store';
import { getThreadEventsBump } from '../../store/threadActivity';
import { unfocusThread } from '../../store/actions/threads';
import { loadThreadEvents, loadOlderThreadEvents, ensureWholeThreadLoaded, forceRetryThreadEvents, retryThreadEvents, threadHistoryReadInFlight, threadLoadInFlightMs } from '../../store/actions/thread-loading';
import { checkConnection } from '../../store/actions/connection';
import { gatewayPickerHref } from '../../utils/basePath';
import { replaceDocument } from '../../utils/documentNavigation';
import { rebuildCorruptedThreadEvents } from '../../store/actions/thread-sync';
import { useScrollObservers, renderExchanges, ScrollControls } from './CreateThreadView';
import { StoppedChildNotice } from './StoppedChildNotice';
import { ThreadStatusIcon, threadVisualStatus } from '../shared/ThreadStatusIcon';
import { ThreadTitleEditor } from './ThreadTitleEditor';
import { PinThreadButton } from '../shared/PinThreadButton';
import { ThreadOverflowMenu } from '../shared/ThreadOverflowMenu';
import { MobileThreadTitleBar } from '../layout/MobileAppHeader';
import { computeExchanges, exchangeKey, exchangeResponseEvents, hasContentEvents, turnBodyFolded, type Exchange } from '../../store/thread-events';
import { rowsDrawnByClamp } from '../../store/event-rendering';
import { statusLabel } from '../../store/exchange-status';
import { awayFromBottom, notAtTop, scrollToBottomAnimated, scrollToTop, hasPendingEventScroll, isElementVisible, isNavigationScroll, isScrollbarHeld, isWhereWeLastScrolledIt, markAnchorScroll, onScrollbarReleased, scrollbarReleased, deepLinkRenderAll } from './scrollState';
import { EMPTY_FILL_LEDGER, WHOLE_THREAD, chargeFillRound, drawnRowsInWindow, fillRoundAllowed, settleFillLedger, anythingAbove, atScrollTop, canSeedRenderWindow, deepLinkMustPersist, edgeHasMoreAbove, edgeMustReachRow, exchangeRenderCost, readingChaseAction, READING_CHASE_PAGE_SIZE, expandWindowEdge, fillAction, reseedOnReopen, seedWindowEdge, transcriptScrolls, UPWARD_SCROLL_KEYS, WINDOW_EXPAND_MARGIN_PX, scrollToTopNeedsRenderAll, type FillLedger, type RowsAt, type WindowEdge } from './threadWindow';
import { anchorTargetTop, readScrollAnchor, type ScrollAnchor } from './scrollAnchor';
import { useScrollMemory, threadScrollKey, readSavedScroll, type SavedScroll } from '../../hooks/useScrollMemory';
import { useThreadScrollIndicator } from '../../hooks/useThreadScrollIndicator';
import { useDelayedFlag, useLingeringFlag } from '../../hooks/useDelayedLoading';
import { ThreadSkeleton } from './ThreadSkeleton';
import { showThreadSkeletonNow, threadIsLoadingNow, type ThreadLoadingState } from './threadSkeletonGate';
import { forceWebKitRepaint, forceWebKitRepaintBurst, createRepaintThrottle, repaintNudgeShift, settledScrollTop } from '../../utils/webkitRepaint';
import { isWebKit } from '../../utils/platform';
import { onPageResume } from '../../utils/pageResume';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { refreshClient } from '../../hooks/sw-update';
import { recordPerfSample } from '../../utils/perfQueue';
import { takeThreadOpenStart, takeThreadRerenderStart } from '../../utils/threadOpenMarks';
import { readRenderPhaseTotals } from '../../utils/renderPhaseTimers';
import { reportThreadRenderProbe } from '../../utils/threadRenderProbe';
import { postClientLog } from '../../utils/liveness';
import { publishScrollbarGutter } from '../../utils/scrollbarGutter';

/** Only sample a grouping fold this slow (ms) — keeps cheap incremental folds
 *  (streaming tokens) out of the perf log; we only want the expensive full
 *  rebuilds that the user actually feels on open / answer. */
const PERF_GROUP_THRESHOLD_MS = 20;

// Module-level tracking survives component unmount/remount (e.g. Thread A → CreateThread → Thread B).
// Using a ref would reset on remount, causing the fade-in to be skipped.
let lastRevealedThread: string | null = null;

// Per-thread render-window TOP EDGE: which exchange is the oldest rendered, and
// how many of ITS leading rows are clamped off. Module-scoped so it survives a
// switch-away-and-back (see the windowing block in ThreadView). `WHOLE_THREAD`
// is what a deep-link sets. Session-scoped; two small ints per visited thread.
//
// A window the reader grew survives that switch, even one grown to the first
// turn. A render-all does not: it is a claim one visit's navigation made, and a
// fresh open drops it back to the seed. See `reseedOnReopen`.
//
// The edge, not the trailing count, in both dimensions. A turn or a row already
// on screen must not leave the DOM when a newer one is appended.
// `renderCountFromFloor` carries the rest.
//
// Stored with the turn it names BY IDENTITY, beside the index. A fold of older
// history grows the list at the FRONT. A bare index then names an older turn,
// from the fold's commit until the re-point effect runs. That one render drew
// every turn the fold brought in, a blocking render (ADR 0081). The reading
// position's restore also landed on that stale page, and the re-point then
// shrank it under the reader. `storedEdge` resolves the key instead.
const renderFloorByThread = new Map<string, StoredWindow>();

/** A stored window, and who stored it (`StoredWindowKind`, which is what a
 *  reopen asks). A render-all is pinned to the thread's first turn whatever
 *  arrives in front of it. A grown edge names its turns by key
 *  (`exchangeKey`): its own, and the one after it, for when a fold absorbs a
 *  fragment into its real turn. */
type StoredWindow =
    | { kind: 'render-all' }
    | { kind: 'grown'; edge: WindowEdge; key: string | null; nextKey: string | null };

/** Store `edge` against the exchanges it was computed from. */
function storeEdge(threadId: string, edge: WindowEdge, exchanges: readonly Exchange[]): void {
    const at = exchanges[edge.exchange];
    const next = exchanges[edge.exchange + 1];
    renderFloorByThread.set(threadId, {
        kind: 'grown',
        edge,
        key: at ? exchangeKey(at) : null,
        nextKey: next ? exchangeKey(next) : null,
    });
}

/** Store a render-all: the thread's first turn, pinned there. */
function storeRenderAll(threadId: string): void {
    renderFloorByThread.set(threadId, { kind: 'render-all' });
}

/** The stored edge, resolved against THESE exchanges. The index stands while
 *  it still names its turn, which is every commit but a fold's. */
function storedEdge(threadId: string, exchanges: readonly Exchange[]): WindowEdge | undefined {
    const stored = renderFloorByThread.get(threadId);
    if (!stored) return undefined;
    if (stored.kind === 'render-all') return WHOLE_THREAD;
    const { edge, key, nextKey } = stored;
    if (key === null) return edge;
    const at = exchanges[edge.exchange];
    if (at && exchangeKey(at) === key) return edge;
    return anchorAfterBackfill(exchanges, { anchorKey: key, nextKey, rowsHidden: edge.rowsHidden }) ?? edge;
}

/** The fill's rounds per thread (`FillLedger`), beside the window they grew.
 *  Module-scoped for the same reason that Map is: the window survives a
 *  switch-away-and-back, so a per-mount ledger would hand every revisit fresh
 *  caps and compound the cost they exist to bound.
 *
 *  A scroll-driven backfill is the reader asking and is never counted here:
 *  this bounds only what the fill asks for on its own. */
const fillLedgerByThread = new Map<string, FillLedger>();


/** The thread this session last opened, so a re-open can be told from a later
 *  commit of the same visit. Module-scoped for the same reason the maps above
 *  are: it has to survive a remount, which a layout swap at the mobile
 *  breakpoint performs mid-read. See `reseedOnReopen`.
 *
 *  `owner` is the mounted instance that holds the visit. A layout swap renders
 *  the incoming instance BEFORE the outgoing one unmounts. Without the owner,
 *  that teardown ended the visit the incoming instance had just continued, and
 *  its next render re-seeded the window under the reader. */
let lastSeededVisit: { threadId: string; owner: object } | null = null;

/** Scroll metrics captured just before a window grow, so the anchor effect can
 *  restore `scrollTop` by the height added. */
type PendingExpand = { prevScrollHeight: number; prevScrollTop: number } | null;

/** This thread's window right now: the stored edge, or the seed until one is
 *  stored. Four callers read it and must agree: the render, the scroll-up
 *  expansion, the fill and the anchor walk. */
function currentEdge(
    threadId: string,
    exchanges: readonly Exchange[],
    costs: readonly number[],
    rowsAt: RowsAt,
): WindowEdge {
    return storedEdge(threadId, exchanges) ?? seedWindowEdge(costs, rowsAt);
}

/** Grow this thread's window by one budgeted step, holding the reader on the
 *  content they were looking at. Three callers, differing only in what asks:
 *  the scroll-up expansion, the fill and the anchor walk.
 *
 *  Prepending older rows grows the list upward, so it captures the metrics the
 *  anchor effect needs, and arms only when the window ACTUALLY grows. Reports
 *  whether it grew.
 *
 *  The capture is not always this one's to keep. The backfill re-point
 *  overwrites it with the reading the REQUEST took, that being the last one
 *  from the frame the reader was in. Safe because the anchor effect rides
 *  `winTick`: every arming bumps it, so a capture is consumed whatever the
 *  resulting edge string is. */
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

/** A read of older history in flight, and what the reader was looking at when
 *  it started.
 *
 *  `anchorKey` is the `exchangeKey` of the exchange the window rested on. The
 *  edge is an INDEX. A page of older history grows the array at the front, so
 *  that index would afterwards name a different turn. Re-finding the anchor is
 *  what holds the reader still.
 *
 *  Identity rather than a count of what arrived. The fold can merge a page's
 *  last turn into the first one held. So the array does not grow by the number
 *  of exchanges the page carried. */
type HistoryHold = {
    /** The turn to re-point the WINDOW onto, and null when there is none to
     *  re-point against: a page that folded to nothing renderable, or a
     *  whole-history hold (see `wholeHistoryHold`). Either way the landing
     *  holds the reader by pixels and leaves the window alone. */
    anchorKey: string | null;
    /** The turn BELOW the anchor, as a second chance at re-pointing.
     *
     *  A page boundary rarely lands on a turn start, so the oldest loaded turn
     *  is usually a fragment whose opening message is still unfetched. When the
     *  page behind it arrives, the fold merges that fragment into the real turn
     *  and the anchor's key stops existing.
     *
     *  This one is a whole turn well inside the loaded set, so no merge at the
     *  boundary can absorb it. The content the anchor held now sits in the
     *  exchange just before it. */
    nextKey: string | null;
    rowsHidden: number;
    prevScrollHeight: number;
    prevScrollTop: number;
    /** The TURN the reader is parked on, when one can be named, and the offset
     *  its top sat at. What `holdTargetTop` prefers, because it survives growth
     *  the two numbers above cannot describe.
     *
     *  Those two say only how much taller the transcript got, so the correction
     *  has to assume every pixel of it landed above the reader. A live thread
     *  drawing a reply BELOW them breaks that, and it fires no scroll event, so
     *  the refresh never sees it. The correction then pushes the reader down by
     *  whatever the agent wrote. A whole-history fetch runs for seconds, which
     *  is long enough for that to be a screenful.
     *
     *  Null where no turn can be named, and for a scroll-driven backfill, whose
     *  read is one short round trip. */
    anchor: ScrollAnchor | null;
};

/** Reads of older history in flight, per thread, for the same reason the window
 *  Map is module-scoped: a switch away and back must not start a second one. */
const historyHoldByThread = new Map<string, HistoryHold>();

/** Where the container must sit to leave the reader on the content they were
 *  reading, once older history has folded in above them.
 *
 *  The hold's TURN first, measured afresh in the layout the fold left. Failing
 *  that the height delta, which is all a hold naming no turn has. */
export function holdTargetTop(el: HTMLElement, hold: HistoryHold): number {
    const anchored = hold.anchor ? anchorTargetTop(el, hold.anchor) : null;
    if (anchored !== null) return anchored;
    return hold.prevScrollTop + (el.scrollHeight - hold.prevScrollHeight);
}

/** The hold a WHOLE-HISTORY fetch takes, and why it names no turn to re-point.
 *
 *  A deep link renders the thread whole, so `ensureWholeThreadLoaded` pulls the
 *  history behind the newest page. That history folds in at the FRONT, seconds
 *  after the link has already landed the reader on their event. WebKit
 *  implements no scroll anchoring, so the transcript grows above them and
 *  carries them up into history nobody asked for.
 *
 *  It is the same hold a scroll-driven backfill takes, so the scroll listener
 *  keeps it current while the link's own glide moves the reader.
 *
 *  NO ANCHOR KEY, where `requestBackfill` names one. That key re-points the
 *  WINDOW, and a render-all draws every turn and must go on drawing them. The
 *  `anchor` beside it is a different thing, and this hold DOES carry one: it
 *  positions the reader rather than the window. */
export function wholeHistoryHold(el: HTMLElement): HistoryHold {
    return {
        anchorKey: null,
        nextKey: null,
        rowsHidden: 0,
        prevScrollTop: settledScrollTop(el),
        prevScrollHeight: el.scrollHeight,
        anchor: readSettledAnchor(el),
    };
}

/** The turn the reader is parked on, for a hold applied in a later frame. A
 *  nudge in flight has scrolled the content a pixel off where the reader is, so
 *  the offset is read that pixel off too. See `settledScrollTop`. */
function readSettledAnchor(el: HTMLElement): ScrollAnchor | null {
    const anchor = readScrollAnchor(el);
    return anchor && { ...anchor, relTop: anchor.relTop + repaintNudgeShift(el) };
}

/** Where the window should sit once a page of older history has folded in.
 *
 *  The anchor turn first, which is the reader's own place. Failing that, the
 *  turn below it, because the boundary turn is usually a fragment the fold
 *  absorbs when its opening message finally arrives. The absorbed content then
 *  lives in the exchange just before that one, so that is where the window
 *  goes.
 *
 *  `rowsHidden` travels only with the anchor itself. In the fallback it counts
 *  rows of a turn that is gone, and showing a whole turn hides nothing the
 *  reader had.
 *
 *  Null when neither survives, which leaves the caller nothing to re-point
 *  against. Exported for its own unit test, since both arms turn on how the
 *  fold merges. */
export function anchorAfterBackfill(
    exchanges: readonly Exchange[],
    pend: { anchorKey: string | null; nextKey: string | null; rowsHidden: number },
): WindowEdge | null {
    // A null key matches nothing, `exchangeKey` always returning a string.
    const found = exchanges.findIndex(e => exchangeKey(e) === pend.anchorKey);
    if (found >= 0) return { exchange: found, rowsHidden: pend.rowsHidden };
    if (!pend.nextKey) return null;
    const below = exchanges.findIndex(e => exchangeKey(e) === pend.nextKey);
    if (below < 0) return null;
    return { exchange: Math.max(0, below - 1), rowsHidden: 0 };
}

/** What a saved *reading position* asks the window to draw: a whole turn, or
 *  one step row (ADR 0152). */
export type ReadingTarget = { kind: 'turn'; id: string } | { kind: 'row'; id: string };

/** The target a record names, or null for a record that names none. */
export function readingTargetOf(record: SavedScroll | null): ReadingTarget | null {
    if (record?.kind === 'anchor') return { kind: 'turn', id: record.eventId };
    if (record?.kind === 'row') return { kind: 'row', id: record.rowEventId };
    return null;
}

/** Where `target` sits in the loaded exchanges: its exchange, and for a row
 *  its index among that exchange's rendered rows. Index -1 when no loaded
 *  exchange holds it.
 *
 *  A row is found by event id among a turn's events. The id is the tool call's
 *  (or the result's), which a *continuation fragment* holds too, as its own
 *  first event. The row index counts the rows `rowsDrawnAt` lists, which are
 *  the rows `rowsHidden` clamps. */
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
        const row = exchangeResponseEvents(ex, false, true).findIndex(
            (r) => r.type === 'step' && (r.call_event_id === target.id || r.result_event_id === target.id),
        );
        return { index: i, row: Math.max(0, row) };
    }
    return { index: -1, row: 0 };
}

/** Ask for the page of history behind the window, holding the reader's place.
 *
 *  A no-op on two counts: one is already in flight, or the store has nothing
 *  older. The store refuses a second fetch itself, so this guard is about the
 *  ANCHOR. A second capture would overwrite the first reader's position.
 *
 *  An empty transcript still asks. It has no anchor and needs none, there being
 *  no reading position to hold. Refusing there left a page that folded to
 *  nothing with no way to fetch the one behind it.
 *
 *  Reports whether it took the capture, which is the fill's signal that a
 *  request is really going out. The fill has already ruled out the store's own
 *  refusal by then, asking `threadHistoryReadInFlight` first. */
function requestBackfill(
    el: HTMLElement,
    threadId: string,
    exchanges: readonly Exchange[],
    edge: WindowEdge,
    onSettled: (added: boolean) => void,
    pageSize?: number,
): boolean {
    // The STORE's own answer, rather than "is a hold recorded". The two agree
    // while a hold belongs to a running read. Asking the store is what stops a
    // hold nothing consumed from refusing pages for ever.
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
    settleOn(loadOlderThreadEvents(threadId, pageSize, () => scrollbarReleased(el)), threadId, onSettled);
    return true;
}

/** What a read of older history does when it settles, either way. Both readers
 *  end here, `requestBackfill` above and `requestWholeHistory` below.
 *
 *  A fold tells the landing through `onSettled`, never through `exchanges`
 *  changing. A live turn on an active thread changes those too. Consuming the
 *  hold there spends it on an update the reader never asked for, and leaves
 *  nothing for the read that follows.
 *
 *  Nothing arrived means nothing moved and the hold is spent, so dropping it is
 *  what lets the next scroll ask again.
 *
 *  A read whose thread the reader has LEFT says nothing at all. Its callbacks
 *  belong to the component, which is showing another thread by now. A
 *  whole-history read runs for seconds, so it would spend that thread's hold.
 *  The switch has already dropped this thread's own. Same guard the up chevron
 *  keeps on its own late read. */
function settleHistoryRead(threadId: string, added: boolean, onSettled: (added: boolean) => void): void {
    if (!added) historyHoldByThread.delete(threadId);
    if (focusedThreadId.value !== threadId) return;
    onSettled(added);
}

/** Settle `read` when it answers, and settle it as "nothing arrived" if it
 *  throws. Neither read rejects today, each catching its own transport failure,
 *  and settling on one anyway is what keeps a hold from outliving its read. A
 *  hold nothing consumes blocks every later page for the rest of the visit. */
function settleOn(read: Promise<boolean>, threadId: string, onSettled: (added: boolean) => void): void {
    void read.then(
        added => settleHistoryRead(threadId, added, onSettled),
        () => settleHistoryRead(threadId, false, onSettled),
    );
}

/** Threads whose whole history this visit has already asked for, so the retry
 *  below cannot ask twice. Module-scoped like the maps above, and cleared by
 *  the same teardown: a later visit is free to ask again. */
const wholeHistoryAskedByThread = new Set<string>();

/** Pull the history behind the newest page for a deep link's render-all,
 *  holding the reader's place across the fold. `wholeHistoryHold` carries what
 *  the hold is for and why it re-points no window.
 *
 *  Its caller stands it down while a read is already running, so this never
 *  queues behind one. A queued read would fold with no hold at all: the hold it
 *  would take is the running read's, still in the map.
 *
 *  ONCE per visit, and that is what bounds the retry. A fetch that FAILS toasts
 *  inside the store and leaves `hasOlderEvents` true. Asking again on its own
 *  settle is then a loop, one toast a round. The chevron and the reader's own
 *  scroll are the ways back from a failure. */
function requestWholeHistory(el: HTMLElement, threadId: string, onSettled: (added: boolean) => void): void {
    if (wholeHistoryAskedByThread.has(threadId)) return;
    wholeHistoryAskedByThread.add(threadId);
    historyHoldByThread.set(threadId, wholeHistoryHold(el));
    settleOn(ensureWholeThreadLoaded(threadId, () => scrollbarReleased(el)), threadId, onSettled);
}

/** Escalating retry delays for the empty-thread safety retry. */
const EMPTY_THREAD_RETRY_DELAYS = [500, 2000, 5000];

/** How long a focused thread may show nothing with NO fetch in flight before
 *  the watchdog restarts its load. Short, because nothing is running: the
 *  retry costs one request and can only shorten the wait. */
const LOST_LOAD_RETRY_MS = 2000;

/** How long an IN-FLIGHT load may run before the watchdog calls it stalled and
 *  restarts it. Long, because restarting a live download re-fetches the whole
 *  snapshot: the thread that motivated this ships 374 kB compressed, and a
 *  phone can legitimately spend seconds on it.
 *
 *  Sized for the failure it exists to catch, not for slowness. An iOS
 *  suspension can leave a fetch promise unresolved for ever, and no honest
 *  snapshot takes this long. The reader has the "Taking too long? Tap to
 *  reload" fuse at 8s either way, so nobody is stranded waiting on this. */
const STALLED_LOAD_RETRY_MS = 30_000;

/** Delay (ms) after a thread is focused before the settle probe samples render
 *  state and re-fires the repaint burst. Past the open burst's tail (1000ms) so a
 *  late blank is covered, and short enough to sample BEFORE the user typically
 *  recovers it with a manual scroll. See the settle-probe effect + threadRenderProbe.ts. */
const SETTLE_PROBE_DELAY_MS = 1500;

/** Ref shape shared by retryCountRef and watchdogRef. */
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

/** Determine whether the thread content is eligible for slide-in animation.
 *  The caller must also check revealOnFocus before triggering the animation. */
export function shouldRevealThread(threadId: string | null | undefined, animating: boolean, hasContent: boolean): boolean {
    if (!threadId || animating || !hasContent) return false;
    if (threadId === lastRevealedThread) return false;
    return true;
}

/** Mark a thread as revealed (call after animation starts). */
export function commitReveal(threadId: string) {
    lastRevealedThread = threadId;
}

/** Reset reveal tracking (called on unmount so re-entering the same thread animates). */
export function resetRevealTracking() {
    lastRevealedThread = null;
}

/** Timeout (ms) before showing "Tap to reload" in loading state. */
const RELOAD_TIMEOUT = 8000;

/** Minimum gap (ms) between forced WebKit repaints while a thread streams. ~5/sec
 *  keeps the compositor layer from getting stuck blank without thrashing it on
 *  every token. */
const STREAM_REPAINT_THROTTLE_MS = 200;

/** Discriminated union — each variant is one render path. Impossible states
 *  (e.g. "loaded with events but animating") can't be constructed. */
export type EmptyReason =
    | { kind: 'loading'; threadId: string }
    | { kind: 'animating' }
    | { kind: 'failed'; threadId: string }
    | { kind: 'corrupt'; threadId: string }
    | { kind: 'disconnected'; threadId: string }
    | { kind: 'working' }
    | { kind: 'empty' };

/** Derive the empty reason from thread state. During the compose→thread send
 *  animation, returns 'animating' — rendered as nothing — which both prevents
 *  the error/empty states from flashing when events arrive via SSE before the
 *  animation gate lifts AND keeps the loading skeleton (a "fetching an existing
 *  conversation" affordance) from showing on a brand-new thread's first prompt
 *  send, where the content is the just-sent message, not a DB fetch.
 *
 *  `hasContent` is true iff the thread has at least one event that should
 *  contribute to a rendered exchange (see hasContentEvents). A composing draft
 *  carrying only ThreadStarted is empty, not corrupt — the corrupt branch is
 *  reserved for actual content events failing to form exchanges. Composing
 *  drafts never reach this code path — ThreadPane routes them to
 *  CreateThreadView. */
export function emptyReason(
    animating: boolean,
    eventsLoaded: boolean,
    eventsLoadFailed: boolean,
    hasContent: boolean,
    threadId: string,
    disconnected: boolean,
    /** Is a turn running on this thread right now? Default `false`, so the many
     *  existing callers that only ever describe a settled thread are unchanged. */
    turnInFlight: boolean = false,
): EmptyReason {
    if (animating) return { kind: 'animating' };
    if (eventsLoadFailed) return { kind: 'failed', threadId };
    if (eventsLoaded && hasContent) return { kind: 'corrupt', threadId };
    // Loaded, nothing to draw, and work is running. That is not an empty
    // thread, and "No messages in this thread" over a live turn is the blank
    // this whole change exists to stop. Two ways in: a voice thread whose
    // first content event has yet to be written, and a thread opened fresh
    // mid-turn. See docs/plans/2026-09-05-a-turn-is-never-blank.md.
    if (eventsLoaded && turnInFlight) return { kind: 'working' };
    if (eventsLoaded) return { kind: 'empty' };
    // The thread never loaded AND the engine is unreachable (the dot is red). Show
    // an honest "can't reach this workspace" state instead of a spinner that
    // degrades to "Tap to reload" — a full reload from the SW cache just re-renders
    // the same dead shell. `disconnected` ONLY overrides `loading`: a load that
    // actually failed/completed (failed/corrupt/empty above) is already honest.
    if (disconnected) return { kind: 'disconnected', threadId };
    return { kind: 'loading', threadId };
}

function ThreadEmptyState({ reason }: { reason: EmptyReason }) {
    // Both call sites key this component by threadId, so this timer restarts per
    // thread. The loading SKELETON is no longer rendered here — it's a fading
    // overlay in `.thread-content-wrap` (ThreadSkeletonOverlay) so it can
    // crossfade out as the exchanges appear, instead of a hard swap inside the
    // scroll container. This component only owns the terminal text states + the
    // 8s "stuck load" reload affordance (a delay-only fuse).
    const showReload = useDelayedFlag(reason.kind === 'loading', RELOAD_TIMEOUT);
    return threadEmptyStateBody(reason, showReload);
}

/** What a transcript with no exchanges draws, as a pure function of the reason
 *  and the one clock above it.
 *
 *  Split out of the component so the invariant over it can be enumerated
 *  without a DOM: `the-transcript-is-never-blank.test.tsx` walks every arm and
 *  asserts each one says something. Only two are allowed to say nothing, and
 *  both are named there.
 *
 *  Exported ONLY for that test, which is why it takes its clock rather than
 *  reading one. Render it through `ThreadEmptyState`. */
export function threadEmptyStateBody(reason: EmptyReason, showReload: boolean) {
    switch (reason.kind) {
        case 'animating':
            // Content is gated by the compose→thread send FLIP, not a DB fetch —
            // render nothing so a brand-new thread's first send doesn't flash a
            // placeholder. The just-sent message appears when the gate lifts.
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
            // Engine unreachable (red dot) and this thread never loaded. Retry
            // re-probes the connection + reloads the thread's events (NOT a page
            // reload — offline, that just re-serves the cached shell). "Back to
            // workspaces" is the always-reachable recovery surface; shown only
            // when a gateway picker exists (null on a legacy direct engine).
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
            // Nothing has been written down yet, but a turn is running. It
            // wears the same running-text shimmer a turn's own header wears,
            // so the two read as one affordance rather than two loaders.
            //
            // The word comes from `statusLabel`, never a literal. A transcript
            // with no turns yet must not say something different from the turn
            // that appears under it a moment later. No steps have arrived by
            // construction here, which is "Requesting".
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
            // The skeleton is the overlay; here only the 8s reload affordance
            // appears if the load is genuinely stuck.
            return showReload ? (
                <div class="thread-empty-state">
                    <button class="thread-empty-reload" onClick={() => refreshClient()}>
                        Taking too long? Tap to reload
                    </button>
                </div>
            ) : null;
    }
}

/** The thread-open loading skeleton, as a fading overlay over the scroll area
 *  (a sibling of `.thread-content` in the position:relative `.thread-content-wrap`).
 *  Living outside the scroll container lets it linger and crossfade OUT as the
 *  exchanges render underneath, rather than hard-swapping. `show` is the
 *  delay-gated loading flag; the overlay then lingers for the length of its own
 *  fade while fading. Decorative + pointer-events:none so it never blocks the
 *  content.
 *
 *  The fade is `opacity var(--duration-normal)`, whose 1x value is the constant
 *  below; the linger scales it by the Animation speed slider and adds fixed
 *  slack, so the overlay cannot unmount mid-fade at a slow setting. */
const SKELETON_FADE_OUT_MS = 200;
const SKELETON_FADE_SLACK_MS = 50;

/** The overlay's class for one render, pure so both states are testable
 *  without a DOM. Opaque while shown, transparent on the way out.
 *
 *  It carries NO entrance. The delay gate has already decided this wait
 *  deserves a loader, so the loader has to be legible the moment it lands. */
export function skeletonOverlayClass(show: boolean): string {
    return show ? 'thread-skeleton-overlay' : 'thread-skeleton-overlay is-fading';
}

function ThreadSkeletonOverlay({ show }: { show: boolean }) {
    const mounted = useLingeringFlag(show, scaledDurationMs(SKELETON_FADE_OUT_MS) + SKELETON_FADE_SLACK_MS);
    if (!mounted) return null;
    return (
        <div class={skeletonOverlayClass(show)} aria-hidden="true">
            <ThreadSkeleton />
        </div>
    );
}

export function ThreadView() {
    const threadId = focusedThreadId.value;

    // Event-driven thread data from threadMap
    const eventThread = threadId ? threadMap.value.get(threadId) : undefined;
    const eventsLoaded = eventThread?.eventsLoaded ?? false;
    const eventsLoadFailed = eventThread?.eventsLoadFailed ?? false;
    // Does the SERVER hold turns above what this client has? The window's own
    // edge answers a narrower question. Reading it for this one is what stuck a
    // reported thread with no scroll and no chevron.
    const hasOlderEvents = eventThread?.hasOlderEvents === true;

    const animating = promptAnimating.value;
    // Fallback: if loadThreadEvents failed (e.g. iOS Safari PWA resume),
    // still render any events delivered via SSE. Also count pending user
    // messages — CodingAgentThreadSpawned transfers them before DB events load.
    const eventCount = eventThread?.events.size ?? 0;
    const pendingCount = eventThread?.pendingUserMessages.length ?? 0;
    const hasPending = pendingCount > 0;
    const hasContent = eventsLoaded || eventCount > 0 || hasPending;

    // Compute exchanges directly from thread data instead of reading
    // activeExchanges computed signal — iOS Safari PWA computed signal
    // dependency tracking can become stale after prolonged use.
    // useMemo avoids recomputing on unrelated re-renders (scroll, streaming).
    // threadId MUST be in deps — without it, switching between threads with the
    // same eventCount returns stale exchanges from the previous thread.
    // The per-thread events bump is also in deps: SSE-time streaming arrivals
    // no longer fire `threadMap`, so `eventCount` (read from `threadMap`) can't
    // be the only stream-aware dep. Reading the bump here both subscribes
    // ThreadView to this thread's stream activity AND invalidates the memo on
    // every event arrival.
    const eventsBump = threadId ? getThreadEventsBump(threadId) : 0;
    // Time the grouping fold (the suspected O(n) cost behind both "clicking into
    // a thread" and "answering a question" lag — both re-run computeExchanges).
    // The fold snapshots its own duration + size into groupSampleRef so the
    // post-commit effect below records a SELF-CONSISTENT sample (recording inside
    // a memo would be an impure side-effect; reading eventCount/exchanges at
    // effect time could pair a stale duration with a different render). Cleared
    // by the effect after it fires. See utils/perfQueue.ts.
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
    // Track the exchange count this render actually produced. The settle probe
    // below compares it against a FRESH recompute from the store to tell a
    // missed re-render (rendered 0, store has N) apart from a genuine empty —
    // see utils/threadRenderProbe.ts.
    const lastRenderedExchangesLenRef = useRef(0);
    lastRenderedExchangesLenRef.current = exchanges.length;
    const streamingBuffer = animating ? '' : activeStreamingBuffer.value;

    // Perf instrumentation: when a grouping fold was slow, sample it (with the
    // size captured alongside it) after commit. Keyed on `exchanges` so it fires
    // once per actual recompute, not on unrelated re-renders. Threshold keeps
    // cheap incremental folds (streaming tokens) out of the log — only the
    // expensive full rebuilds that the user feels get recorded. Fire-and-forget.
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

    // Perf instrumentation: record the open→paint span ONCE per thread open. The
    // open-start was stamped when this thread was focused
    // (loadThreadEvents → utils/threadOpenMarks). Fire on the FIRST content
    // render — when exchanges first appear — and take() the mark so streaming
    // appends / scroll-window growth don't re-fire it (a later render finds no
    // mark). The rAF runs just before the browser paints this committed render,
    // so renderMs ≈ open→paint. Pure render/paint is derived in analysis by
    // subtracting the paired thread-load fetchMs/applyMs (and the group mark's
    // groupMs) for this threadId. Telemetry carve-out (.claude/rules/frontend.md):
    // strictly fire-and-forget — no toast; recordPerfSample swallows internally
    // and nothing here can throw into the render path. See utils/perfQueue.ts.
    useLayoutEffect(() => {
        if (!threadId || exchanges.length === 0) return;
        const base = takeThreadOpenStart(threadId);
        if (base === undefined) return; // already fired, or open not stamped
        const exchangeCount = exchanges.length;
        const events = eventCount;
        requestAnimationFrame(() => {
            // Split the open span: markdown/linkify are the deltas in the phase
            // timers since open-start; the DOM/reconciliation remainder is derived
            // in analysis as renderMs − fetchMs − applyMs − markdownMs − linkifyMs.
            const totals = readRenderPhaseTotals();
            recordPerfSample('thread-render', {
                threadId,
                // Were this thread's events already in memory? A warm open
                // fetches nothing, so its whole span is render. Without the
                // field the two kinds average together and neither is legible.
                warm: base.warm,
                eventCount: events,
                exchangeCount,
                renderMs: Math.round(performance.now() - base.start),
                markdownMs: Math.round(totals.markdownMs - base.md),
                linkifyMs: Math.round(totals.linkifyMs - base.link),
            });
        });
    }, [threadId, exchanges.length]);

    // Perf instrumentation: record a user-initiated RE-RENDER (follow-up send or
    // answering a question) ONCE, with the same markdown/linkify split. The mark
    // is stamped at the action (chat.ts addPendingMessage / chat-claude-code.ts
    // answerThreadQuestion) right before the state change that triggers the
    // re-render, so the FIRST render after is the laggy one we want. No deps array
    // (runs after every commit) so the fire is independent of WHAT changed — the
    // answer flips `answeringThreadIds` (not an exchange-count/eventsBump signal),
    // so a deps-gated effect would miss it. The guard is a cheap take(): a no-op
    // (one Map.get) on the vast majority of renders that have no pending mark, and
    // the take clears it so streaming re-renders don't re-fire. Same telemetry
    // carve-out (.claude/rules/frontend.md) as the open mark.
    useLayoutEffect(() => {
        if (!threadId || exchanges.length === 0) return;
        const mark = takeThreadRerenderStart(threadId);
        if (mark === undefined) return; // no pending user-initiated re-render
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

    // The transcript itself. Declared here rather than beside the render.
    // Every effect below that positions the reader reaches for it, the first
    // being the deep link's own history fetch.
    const areaRef = useRef<HTMLDivElement>(null);
    // This mount's identity, as the holder of a visit (`lastSeededVisit`).
    const visitOwner = useRef({}).current;

    // --- Thread-render windowing (perf) ---
    // A large focused thread used to render — and markdown-parse — every exchange
    // synchronously on open and on every re-render (measured ~270–500ms of pure
    // JS). Render only a TAIL of the list and grow it on scroll-up. The per-thread
    // window edge lives in a module Map (`renderFloorByThread`) so it SURVIVES a
    // switch-away-and-back — otherwise returning to a thread you'd scrolled up in
    // would re-window to the tail and useScrollMemory couldn't restore your spot.
    // A new thread starts at the SEEDED window: the newest exchanges fitting the
    // step budget, capped at INITIAL_WINDOW. Its first render is already windowed,
    // never a one-off full render. A notification deep-link forces the full list
    // instead, so a windowed-out target can render. The persist effect below then
    // keeps it full, so clearing the claim doesn't snap the user back.
    // See threadWindow.ts + scrollState.deepLinkRenderAll.
    const [winTick, bumpWin] = useState(0);
    // Bumped only when older history has FOLDED IN, which is what the re-point
    // effect keys on. An `exchanges` change is not the same event: a live turn
    // on an active thread produces one too.
    const [historyFolded, setHistoryFolded] = useState(0);
    // And bumped whenever a read of older history SETTLES, however it settled.
    // The deep-link retry needs the empty and failed cases too, which the fold
    // count deliberately does not carry.
    const [historyReadSettled, setHistoryReadSettled] = useState(0);
    const onHistoryRead = (added: boolean) => {
        if (added) setHistoryFolded(n => n + 1);
        setHistoryReadSettled(n => n + 1);
    };
    // Re-render after a window write. `bumpWin` is stable, so an effect holding
    // an older copy of this still reaches the current component.
    const bumpWindow = () => bumpWin(n => n + 1);
    const exchangeCosts = useMemo(() => exchanges.map(exchangeRenderCost), [exchanges]);
    // The rows the exchange at `index` renders, each flagged by whether the
    // reader's view draws it. The second half of the window's arithmetic, and
    // the only part that has to fold a turn. The window budgets by the drawn
    // rows (`ROW_BUDGET`); the list's length is the unit `rowsHidden` counts.
    //
    // `exchangeRenderCost` above is O(1) and counts raw events, deliberately:
    // it runs for every exchange on every commit. This runs for the floor
    // exchange when the window is seeded or grown, and for the drawn window
    // when the fill measures a round. Not memoized for that reason.
    //
    // Folded as a SETTLED turn (not last, thread idle). Those two flags move
    // the count by at most the one derived live row, and this feeds a budget
    // rather than a contract. Reading them here would make the floor turn's
    // clamp twitch with the live turn's status, which is a worse trade.
    //
    // The view is read in RENDER, so a toggle re-renders this component and the
    // fill below re-measures against what now draws.
    const showSteps = stepsExpanded.value;
    const showDetails = detailsExpanded.value;
    const folds = collapsedExchanges.value;
    const rowsDrawnAt: RowsAt = (index) => {
        const exchange = exchanges[index];
        if (!exchange || !threadId) return [];
        return rowsDrawnByClamp(exchangeResponseEvents(exchange, false, true), {
            showSteps,
            showDetails,
            folded: turnBodyFolded(folds, threadId, exchange),
        });
    };
    // Fix the window once the event load settles, so later renders read a stored
    // value instead of re-deriving one. The seed is a function of the exchange
    // costs, and a live turn's cost grows with every streamed step. A derived
    // window would therefore push older turns off the top while the reader
    // watches. Write-once: the edge only ever moves UP the list afterwards, from
    // the scroll-up expansion, the chevron's render-all, or the effect below.
    //
    // `canSeedRenderWindow` is what keeps that write-once off a fragment.
    const canSeedWindow = canSeedRenderWindow({
        hasExchanges: exchanges.length > 0,
        eventsLoaded,
        eventsLoadFailed,
    });
    // Once per VISIT, the re-seed a reopen owes (`reseedOnReopen`).
    //
    // In RENDER, before the edge below is read, and not in an effect. An effect
    // runs after the render that drew the OLD edge. Preact flushes a
    // component's pending effects before re-rendering it, so the reading
    // position's restore attached and landed on the old, taller window. The
    // re-render then shrank it under the reader: to the top, or clamped to the
    // bottom from deep in the thread.
    //
    // The visit mark is MODULE-scoped, not a ref. `ThreadView` is remounted
    // whole when the layout swaps at the mobile breakpoint, which a rotation or
    // a window drag does mid-read. Written past the settle guard, so a render
    // before the events land does not spend the visit. The teardown below is
    // what ends one.
    if (threadId && canSeedWindow) {
        const firstThisVisit = lastSeededVisit?.threadId !== threadId;
        lastSeededVisit = { threadId, owner: visitOwner };
        const stored = renderFloorByThread.get(threadId);
        if (!stored || (firstThisVisit && reseedOnReopen(stored.kind, deepLinkRenderAll.peek()))) {
            storeEdge(threadId, seedWindowEdge(exchangeCosts, rowsDrawnAt), exchanges);
        }
    }
    const edge = deepLinkRenderAll.value
        ? WHOLE_THREAD
        : threadId ? currentEdge(threadId, exchanges, exchangeCosts, rowsDrawnAt) : WHOLE_THREAD;
    const renderFromIndex = edge.exchange;

    // Persist the deep-link "render all" so the thread stays fully rendered after
    // the claim clears (no snap back to the tail while the user reads an old event).
    // `deepLinkMustPersist` owns the not-yet-stored arm, which is the one a
    // warm thread never exercises and an inline test got wrong.
    useEffect(() => {
        if (!threadId || !deepLinkRenderAll.value) return;
        if (!deepLinkMustPersist(storedEdge(threadId, exchanges))) return;
        storeRenderAll(threadId);
        bumpWin(n => n + 1);
    }, [threadId, deepLinkRenderAll.value]);

    // The window says draw everything, so the history has to BE everything. A
    // long thread opens on one page, and the linked event is usually older.
    //
    // SEPARATE from the effect above, and keyed on `hasOlderEvents`. A cold
    // open runs that one before the first page has landed, when the thread
    // still reports nothing older. A single call there would ask for nothing
    // and never be repeated. This asks again once the load settles the answer,
    // and stands down on a thread already whole.
    //
    // It HOLDS the reader across the fold. See `requestWholeHistory`: that
    // history arrives at the FRONT, seconds after the deep link placed the
    // reader, and nothing else answers for content growing above them.
    //
    // `historyReadSettled` is what RETRIES it. The ask stands down while
    // another read is running, and that read's settle is the moment asking can
    // hold the reader. `hasOlderEvents` does not always move with it.
    useEffect(() => {
        if (!threadId || !hasOlderEvents) return;
        // Only the `!eventThread` return draws no transcript, and
        // `hasOlderEvents` above has already excluded it. The check narrows the
        // type rather than covering a state.
        const el = areaRef.current;
        if (!el) return;
        // A read ALREADY RUNNING will fold, whoever started it and whether or
        // not a link still claims this open. So hold the reader for it and ask
        // for nothing more.
        //
        // The claim is not asked here, and that is what covers a reader who
        // left and came back inside a long fetch. The teardown drops the hold
        // with the visit, and a re-entry finds the read it was taken for still
        // running.
        if (threadHistoryReadInFlight(threadId)) {
            if (!historyHoldByThread.get(threadId)) {
                historyHoldByThread.set(threadId, wholeHistoryHold(el));
            }
            return;
        }
        if (!deepLinkRenderAll.value) return;
        requestWholeHistory(el, threadId, onHistoryRead);
    }, [threadId, deepLinkRenderAll.value, hasOlderEvents, historyReadSettled]);


    // Latest exchange costs for the scroll handler and the up-chevron, so neither
    // re-attaches on every streaming append. Their length is the exchange count.
    const exchangeCostsRef = useRef<number[]>([]);
    exchangeCostsRef.current = exchangeCosts;
    // The row-count fold, held the same way and for the same reason: a grower
    // running off a ref must fold THIS render's exchanges, not a captured copy.
    const rowsDrawnAtRef = useRef(rowsDrawnAt);
    rowsDrawnAtRef.current = rowsDrawnAt;
    // The exchanges themselves, for the same reason again. The backfill names
    // its anchor turn by identity, which a cost array cannot supply.
    const exchangesRef = useRef<readonly Exchange[]>(exchanges);
    exchangesRef.current = exchanges;
    // The window edge as one value an effect can depend on, BOTH dimensions. A
    // round that only uncovers rows leaves `edge.exchange` unmoved, so an effect
    // keyed on that alone would miss the commit it exists to answer.
    const edgeKey = `${edge.exchange}:${edge.rowsHidden}`;
    // Armed by `growRenderWindow`, consumed by the anchor effect below.
    const pendingExpandRef = useRef<PendingExpand>(null);
    // The turn or row this thread's saved *reading position* names, while the
    // window has yet to reach it. Null once it is rendered, or when there was
    // never one to reach. See `reachAnchor` below.
    const anchorTargetRef = useRef<ReadingTarget | null>(null);
    // How many reads of older history this visit has spent chasing that
    // position (`readingChaseAction`).
    const readingChasesRef = useRef(0);
    // Set by the up-chevron when it renders the full thread before scrolling to
    // the genuine top — consumed by the layout effect that performs the jump once
    // the expanded list commits. See onScrollUp below.
    const pendingScrollTopRef = useRef(false);

    // Eligible for slide-in? revealOnFocus checked in the layout effect only
    // to avoid subscribing the render to the signal (prevents extra re-renders).
    const shouldReveal = shouldRevealThread(threadId, animating, hasContent);

    const isUp = awayFromBottom.value;
    const isNotAtTop = notAtTop.value;

    // Mobile draws its own scroll indicator, because the fixed header covers
    // the native one (components/chat/scrollIndicator.ts).
    //
    // The two elements are held in STATE via callback refs, not in refs: this
    // component renders a loading branch before the transcript branch, so the
    // indicator mounts after the first effect pass, and a ref filling in would
    // never re-run the hook's effect (stable object, unchanged dependency).
    const [indicatorTrack, setIndicatorTrack] = useState<HTMLDivElement | null>(null);
    const [indicatorThumb, setIndicatorThumb] = useState<HTMLDivElement | null>(null);
    useThreadScrollIndicator({
        scrollerRef: areaRef,
        track: indicatorTrack,
        thumb: indicatorThumb,
    });

    // Re-publish --scrollbar-gutter-width now that a real transcript exists to
    // measure. The boot publish (main.tsx) ran before any of this was mounted and
    // could only estimate from a detached probe, which on iOS over-reports by the
    // ::-webkit-scrollbar width the transcript does not actually reserve, leaving
    // the composer's right edge inside the question cards it docks under
    // (utils/scrollbarGutter.ts). Mount-only: what a scroll container reserves is
    // content-independent (`overflow-y: scroll` + `scrollbar-gutter: stable`), so
    // one forced layout read per entry into thread mode is enough, and it runs
    // before paint so the corrected inset is never a visible jump.
    useLayoutEffect(() => { publishScrollbarGutter(); }, []);

    // Force-restart CSS animation imperatively — works even when the
    // .revealing class is already on the element from a prior thread switch.
    // Runs before paint (useLayoutEffect) so the user never sees a flash.
    // Animates .thread-view (title + content together) so the whole thread
    // slides in from the bottom while header and prompt stay put.
    // Applies to every mounted .thread-view element. `App` mounts only the
    // active layout's pane tree, so that is the visible transcript.
    useLayoutEffect(() => {
        if (!shouldReveal || !threadId) return;
        commitReveal(threadId);
        // Only animate on dismiss→next (Archive button), not regular thread selection.
        // peek() reads without subscribing so the effect doesn't re-run on flag changes.
        if (!revealOnFocus.peek()) return;
        revealOnFocus.value = false;
        document.querySelectorAll('.thread-view').forEach(el => {
            el.classList.remove('revealing');
            void (el as HTMLElement).offsetHeight;
            el.classList.add('revealing');
        });
    }, [threadId, animating, hasContent]);

    // Reset on unmount so re-entering the same thread triggers animation
    useEffect(() => resetRevealTracking, []);

    // Load thread events from DB if not yet loaded — backfills any events
    // missed before SSE connected (e.g. recovery threads after engine restart).
    // threadInMap dep ensures re-fire when thread appears in map (e.g.
    // focusThread runs before loadAllThreads completes — first call returns
    // early because thread isn't in map, but this dep change retriggers).
    const threadInMap = !!eventThread;
    useEffect(() => {
        if (threadId && threadInMap && !eventsLoaded) {
            void loadThreadEvents(threadId);
        }
    }, [threadId, threadInMap, eventsLoaded]);

    // The awaited thread arrived, so the exemption below has nothing left to
    // protect. Released here because this component is its only reader: a clear
    // at each map-insert site would drift the next time one is added.
    useEffect(() => {
        if (threadId && threadInMap && awaitedThreadId.value === threadId) {
            awaitedThreadId.value = null;
        }
    }, [threadId, threadInMap]);

    // Safety retry: if eventsLoaded=true but thread is empty, loadThreadEvents
    // may have fetched before the backend committed events. Retry with escalating
    // delays (500ms, 2s, 5s) to give the backend time to commit.
    // Deps exclude eventCount/hasPending to avoid re-runs on every SSE event —
    // the timer's inner check reads fresh state instead.
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

    // Watchdog: a focused thread with no content is either genuinely stuck or
    // merely slow, and the two are owed different patience.
    //
    // NOTHING IN FLIGHT is the stuck one. The fetch never started, or it died
    // leaving no claim standing. That is the race this was written for, and a
    // restart costs one request.
    //
    // AN ATTEMPT IN FLIGHT is usually just a big snapshot on a phone. This
    // fired at a flat two seconds, and `forceRetryThreadEvents` clears the
    // in-flight guard. So a healthy slow load was downloaded a SECOND time over
    // the same pipe, lengthening the wait it exists to shorten. Only a stall
    // past `STALLED_LOAD_RETRY_MS` earns a retry now. That is the iOS
    // suspension case `forceRetryThreadEvents` names, where a fetch promise is
    // left unresolved for ever.
    //
    // Two timers rather than a poll: ask once at the short deadline, and if a
    // fetch is running, sleep exactly as long as it has left. The cleanup
    // cancels both, and `forceRetryThreadEvents` caps itself at one retry per
    // thread, so neither arm can loop.
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

    // Force a WebKit repaint of the scroll container: it invalidates the stale
    // cached compositor texture so DOM-present-but-blank content shows.
    // Called on data changes AND on resume from background.
    const forceRepaint = () => forceWebKitRepaint(areaRef.current);
    // Drop-resilient variant for the thread-OPEN path: a single toggle there has
    // no retry (unlike the streaming throttle and the resume re-fires), so a
    // cold open whose two rAF frames WebKit drops/coalesces stays blank until a
    // manual scroll. The burst spreads several toggles over a few hundred ms.
    const forceRepaintBurst = () => forceWebKitRepaintBurst(areaRef.current);

    // Force a repaint on every threadId change (not just eventsLoaded).
    // WebKit's compositor caches layer textures inside scroll-snap parents.
    // After many thread switches, it stops repainting already-loaded threads.
    // Triggering on threadId alone covers threads where eventsLoaded was already
    // true (no transition to trigger the effect).
    //
    // hasExchanges dep: events can arrive via SSE before loadThreadEvents
    // completes (eventsLoaded stays false). When exchanges first appear from
    // SSE-delivered events, the compositor layer may still hold a blank
    // texture. This dep ensures a repaint fires on the 0→N transition.
    const hasExchanges = exchanges.length > 0;

    // Drives the fading skeleton overlay (ThreadSkeletonOverlay): we're in the
    // 'loading' empty state — no exchanges yet, and not animating / failed /
    // loaded-empty. This mirrors emptyReason(...).kind === 'loading' but is also
    // valid before the thread is in the map (cold start: eventsLoaded and
    // eventsLoadFailed are false). Suppressed when disconnected so the shimmer
    // doesn't sit over the honest "Can't reach this workspace" state
    // (emptyReason maps that same case to 'disconnected', not 'loading').
    //
    // Two clocks arm it, and the second is why the first is not enough. The
    // delay gate keeps a fast / prefetched open from flashing it. A big snapshot
    // landing INSIDE that delay raises the flag itself: the render it triggers
    // blocks the main thread, and the gate's timer cannot fire during it (see
    // threadSkeletonGate.ts).
    const loadingState: ThreadLoadingState = {
        hasExchanges,
        animating,
        eventsLoadFailed,
        eventsLoaded,
        disconnected: connectionStatus.value === 'disconnected',
    };
    const delayElapsed = useDelayedFlag(threadIsLoadingNow(loadingState));
    const showThreadSkeleton = showThreadSkeletonNow(loadingState, delayElapsed);
    // Remember whether the skeleton was shown for this thread, so the content
    // fade-in below can step aside and let the overlay crossfade do the reveal.
    // Shown IS covered here, because the overlay is opaque whenever it is up.
    const skeletonShownRef = useRef<string | null>(null);
    if (showThreadSkeleton && threadId) skeletonShownRef.current = threadId;

    // Content fade-in: play a one-shot opacity fade on the content area the first
    // time a thread's exchanges are present. Opacity-only on the scroll container so
    // scroll math (scroll-to-bottom, saved-scroll, deep-link, window expansion) is
    // untouched; runs in a layout effect (before paint) so content never flashes
    // at full opacity first. Tracked per thread so streaming appends don't re-fade
    // and a re-open replays; reduced-motion disables it via CSS. SKIPPED when the
    // skeleton overlay was shown — then the exchanges render at full opacity
    // beneath the overlay, which crossfades out to reveal them (no double fade /
    // blank frame). The fade is therefore for the fast/no-skeleton open only.
    const enteredThreadRef = useRef<string | null>(null);
    useLayoutEffect(() => {
        if (!threadId || !hasExchanges || enteredThreadRef.current === threadId) return;
        const el = areaRef.current;
        if (!el) return; // content div not mounted yet — retry on the next render
        enteredThreadRef.current = threadId;
        if (skeletonShownRef.current === threadId) return; // overlay crossfade owns the reveal
        el.classList.remove('content-entering');
        void el.offsetHeight; // reflow so re-adding replays the one-shot animation
        el.classList.add('content-entering');
    }, [threadId, hasExchanges]);

    const savedScrollKey = threadId ? threadScrollKey(threadId) : null;

    useEffect(() => {
        if (threadId) {
            // Burst (not a single toggle): this is the open path's ONLY repaint
            // for an idle thread — once content is in the DOM no eventsBump tick
            // re-fires the streaming throttle, and the thread may never go to
            // background to trigger the resume path. A cold open whose one toggle
            // WebKit drops/coalesces (or fires before the layer blanks) would
            // then stay blank until a manual scroll. The burst's spaced retries
            // recover it. cleanup cancels any pending retries on dep change.
            return forceRepaintBurst();
        }
    }, [threadId, eventsLoaded, hasExchanges]);

    // Page resume: force repaint when returning from background.
    // Signal values don't change on resume (same thread, same events), so no
    // re-render produces DOM changes. WebKit's compositor may have recycled
    // the layer texture while backgrounded — content is in the DOM but invisible
    // (renders black). Subscribing to the shared resume signal fires the repaint
    // on visibilitychange / pageshow / focus, not just visibilitychange: iOS
    // frequently restores a PWA via pageshow (bfcache) with no `visible`
    // visibilitychange, which left the old handler silent and the content black
    // until a tap — and that tap could land on an invisible question / permission
    // and answer it (see utils/pageResume, which also swallows that wake-tap).
    // forceRepaint is WebKit-gated and null-safe. The resume listeners are too,
    // so the packaged desktop app repaints on a window it comes back to.
    useEffect(() => onPageResume(forceRepaint), []);

    // Sustained-streaming repaint (WebKit): entering a *running* thread, the rapid
    // streaming DOM mutations can make WKWebView blank the .thread-content
    // compositor layer AFTER the one-shot entry/load repaint above has already
    // fired — the content stays in the DOM (still scrollable, chevron shows) but
    // renders black until a manual scroll. eventsBump ticks on every append to
    // THIS thread (tokens, tool events, CC text), so repaint on a throttle as
    // content streams in. The gate keeps the LEADING edge to one repaint per
    // ~200ms (forceWebKitRepaint supersedes an overlapping toggle so nothing
    // accumulates; forceRepaint is WebKit-gated and null-safe), and its TRAILING
    // edge fires once after the stream pauses — load-bearing for the "click Less
    // on the last running result blanks the pane" report: the toggle's own
    // restore() repaints the collapse shrink, but the next streamed mutation
    // re-blanks the layer a beat later; without the trailing edge that request is
    // throttled away, and if CC then pauses (a tool call running) the pane stays
    // black. The trailing repaint clears it. cancel() on unmount so a pending
    // trailing fire can't run against a torn-down element.
    const streamRepaintGate = useMemo(() => createRepaintThrottle(STREAM_REPAINT_THROTTLE_MS), []);
    useEffect(() => {
        if (!hasExchanges) return;
        streamRepaintGate.request(performance.now(), forceRepaint);
    }, [eventsBump, hasExchanges]);
    useEffect(() => () => streamRepaintGate.cancel(), [streamRepaintGate]);

    // Self-healing watchdog: if a thread has CONTENT events but exchanges are
    // empty, rebuild the events Map and re-fetch from the API. iOS Safari can
    // corrupt long-lived Map internals under memory pressure, causing
    // has()/get() to return wrong results. Capped at 2 attempts. The
    // hasContentEvents gate covers both empty-Map and metadata-only threads
    // (e.g. a composing draft carrying only ThreadStarted) — both are
    // legitimately empty and don't need self-healing.
    const watchdogRef = useRef<ThreadRetryRef>(null);
    useEffect(() => {
        if (!threadId || !eventsLoaded || !eventThread || exchanges.length > 0) return;
        if (!hasContentEvents(eventThread.events)) return;
        if (hasExhaustedRetries(watchdogRef, threadId, 2)) return;
        const timer = setTimeout(() => {
            incrementRetry(watchdogRef, threadId);
            // Breadcrumb the recovery so the next repro shows whether the
            // empty-render/corrupted-Map path fired (vs. the paint path, which
            // leaves no recovery trace). ios-pwa-blackout investigation.
            postClientLog('render', 'rebuild_corrupted_thread', {
                thread_id: threadId,
                event_count: eventThread.events.size,
                channel: eventThread.meta.channel,
            });
            rebuildCorruptedThreadEvents(threadId);
        }, 300);
        return () => clearTimeout(timer);
    }, [threadId, eventsLoaded, exchanges.length, eventCount]);

    // Settle probe + late repaint (WebKit): a fixed delay after a thread is focused,
    // sample the render state and re-fire the open repaint burst ONCE. Armed on
    // threadId alone (NOT eventsLoaded), and reads fresh store/DOM at fire time, so
    // it still runs when a post-load store write failed to re-render this view —
    // the "summary present, body empty, recovers on scroll" report. The probe
    // breadcrumb (skips genuinely-empty threads) records whether the
    // body is missing in the DOM (render gap), stale vs. the store (missed
    // re-render), or present-and-laid-out (compositor paint loss). The extra burst
    // is a transform-only, supersede-safe toggle — harmless on a healthy layer,
    // and it recovers a layer that WKWebView blanked after the open burst's window
    // under prolonged use. One-shot per focus; the cleanup clears it on switch.
    useEffect(() => {
        if (!threadId || !isWebKit()) return;
        const timer = setTimeout(() => {
            const thread = threadMap.value.get(threadId);
            if (!thread) return;
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

    // Scroll memory is the ONLY thing that positions the transcript on open, so
    // it answers every form of "where does this thread start":
    //  - a saved offset restores, returning the reader where they left off;
    //  - a saved LIVE EDGE resumes the standing follow they left armed, so a
    //    thread they were watching is still being watched on re-entry;
    //  - no saved position (a first visit) opens at the TOP of what is
    //    rendered, the way a document opens, via `resetOnEmpty`.
    // A position is never retired for being old. It used to be, once the thread
    // had gained a turn, so that the open would fall through to the
    // auto-scroll-to-bottom and land on the new part. With nothing scrolling to
    // the bottom, retiring only converts "return the reader" into "send them to
    // the top", which is the move this whole change exists to stop.
    // The second half is not optional bookkeeping: `.thread-content` is ONE
    // element reused across threads, so without the reset a fresh thread would
    // inherit the offset of the one before it.
    useScrollMemory(
        areaRef,
        savedScrollKey,
        {
            paused: !eventsLoaded,
            resetOnEmpty: true,
            // The transcript is the one container that can RIDE a position as
            // well as sit at one, so it is the one that records a standing
            // follow. A reader who armed the follow here and then visited
            // another thread comes back to the live edge with the follow armed,
            // rather than to the pixel offset the transcript had when they left
            // (with everything the agent produced meanwhile below them). The
            // content pane and the thread drawer share this hook and do not take
            // it: the follow is one global, so an ungated recording would stamp
            // the transcript's request onto whatever they were showing.
            followsLiveEdge: true,
            // And the one container whose HEIGHT is not reproducible, the render
            // window below deciding it afresh on every reload. So the position
            // it records names the TURN the reader was on rather than a pixel
            // offset into a slice that has since changed size.
            anchorsToContent: true,
            // A notification deep-link (toast / push / inbox) owns the scroll
            // when focusing an UNfocused thread: skip both the restore and the
            // top reset so neither fires after scrollToEventAndPulse and snaps
            // away from the source event. The claim is held until the
            // deep-link's deadline, so this is true for the whole resolve
            // window. (Already-focused threads don't re-run this hook, so they
            // never needed the guard, which is why the bug only bit
            // unfocused-thread deep-links.)
            shouldRestore: () => !hasPendingEventScroll(),
            // The walk below renders markdown a round at a time ON BEHALF of
            // the restore. Once the restore is over, however it ended, that
            // work buys nothing: it would keep rendering older turns for a
            // landing nobody is going to make. The reader taking over is the
            // ordinary case, and no timer here could see it.
            onRestoreSettled: () => { anchorTargetRef.current = null; },
            // A chase for the reading position is a request in flight, with
            // no height changing under it. The restore waits it out.
            restoreIsBusy: () => (threadId ? threadHistoryReadInFlight(threadId) : false),
        },
    );

    // Window expansion: when the user scrolls near the top and older exchanges
    // remain above the rendered tail, reveal another chunk. Captures scroll
    // metrics so the layout effect below keeps the viewport anchored. Re-attached
    // per focused thread; reads the live exchange count via a ref so streaming
    // appends don't churn the listener.
    useEffect(() => {
        const el = areaRef.current;
        if (!el || !threadId) return;
        const reachForOlder = () => {
            if (pendingExpandRef.current) return;
            const costs = exchangeCostsRef.current;
            const rows = rowsDrawnAtRef.current;
            const current = currentEdge(threadId, exchangesRef.current, costs, rows);
            if (!edgeHasMoreAbove(current)) {
                // The window has reached the oldest turn this client HOLDS,
                // which on a long thread is not the oldest turn there is. Fetch
                // the page behind it; the effect below re-points the edge, and
                // the reader's next scroll grows into what arrived. A held
                // scrollbar lets it fetch, and holds its landing.
                requestBackfill(el, threadId, exchangesRef.current, current, onHistoryRead);
                return;
            }
            // A held native scrollbar puts its own drag position back, which
            // undoes the anchor write: the reader would jump by the height
            // drawn. The release handler below reaches instead.
            if (isScrollbarHeld(el)) {
                owesGrow = true;
                return;
            }
            growRenderWindow(el, threadId, exchangesRef.current, costs, current, rows, pendingExpandRef, bumpWindow);
        };
        // The grow a scrollbar hold put off. A drag pinned at the top fires no
        // scroll event, so without this the thread locks there. Only a hold
        // that owes a grow reaches, so a click on padding draws nothing. A
        // frame later, so the drag has ended before the anchor write lands.
        let owesGrow = false;
        let releaseFrame = 0;
        const unsubRelease = onScrollbarReleased((released) => {
            if (released !== el || !owesGrow) return;
            owesGrow = false;
            cancelAnimationFrame(releaseFrame);
            releaseFrame = requestAnimationFrame(() => {
                if (!el.isConnected || el.scrollTop > WINDOW_EXPAND_MARGIN_PX) return;
                reachForOlder();
            });
        });
        const onScroll = () => {
            // A page in flight is held against the reader's LAST known frame,
            // so keep that reading current. The capture is taken when the
            // request is made, and both the reader and the app move the
            // transcript before the page lands: a restore places them on their
            // turn, and a slow link leaves seconds to scroll. Refreshed HERE
            // rather than at the landing, where the fold has already moved the
            // frame the capture describes.
            //
            // Ahead of the navigation guard below, deliberately. The app's own
            // writes are exactly the ones the landing must not undo. The layout
            // read costs a forced reflow, and it is paid only while a page is
            // actually in flight.
            const inFlight = historyHoldByThread.get(threadId);
            if (inFlight) {
                inFlight.prevScrollTop = settledScrollTop(el);
                inFlight.prevScrollHeight = el.scrollHeight;
                // The turn travels with the two numbers, for a hold that named
                // one. It describes where the reader is, so it goes stale the
                // moment they move, exactly as the offset does.
                //
                // A read answering null KEEPS the turn it had. Null is what a
                // momentarily unmeasurable container answers, and a stale turn
                // still beats the delta this exists to avoid.
                if (inFlight.anchor) inFlight.anchor = readSettledAnchor(el) ?? inFlight.anchor;
            }
            // Only the READER asking for older turns may grow the window.
            // Our own positioning fires scroll events too. Opening a thread
            // lands at the TOP of the rendered slice, inside the margin by
            // construction.
            //
            // So without this the open would grow the window by a chunk every
            // visit, parsing that markdown synchronously on the open path.
            // The cost compounds across re-opens, since renderFloorByThread is
            // module-scoped, and it is exactly what windowing exists to avoid.
            // The up-chevron is covered too, and wants to be: it renders the
            // whole thread before gliding, so an expansion mid-glide would
            // re-anchor the viewport and stall it.
            //
            // The position term catches our events that land late. A heavy
            // render can push a restore's event past the clock, and the
            // repaint nudge moves a pixel off it. Neither is the reader.
            if (isNavigationScroll(el) || isWhereWeLastScrolledIt(el)) return;
            if (el.scrollTop > WINDOW_EXPAND_MARGIN_PX) return;
            reachForOlder();
        };
        // A reader pinned at the very top fires NO further scroll events, so
        // the handler above cannot hear them ask again. On a thread several
        // pages long that froze the walk: one page landed, and the rest was
        // unreachable without scrolling back down first.
        //
        // A gesture IS the reader, so no navigation guard applies here. The
        // page lands with them parked at the loaded floor. That is the state
        // the anchored landing was built for, and `requestBackfill`'s own
        // guard keeps a flurry of wheel events to one fetch.
        //
        // Direction decides it, and only an UPWARD gesture asks. The event
        // fires before the browser moves `scrollTop`, so a reader leaving the
        // top downward still reads as pinned there. Acting on that would
        // render turns they are scrolling away from, then jump them by the
        // height it added.
        let lastTouchY: number | null = null;
        const reachIfUp = (reachesUp: boolean) => {
            if (!reachesUp || !atScrollTop(el)) return;
            // A transcript that does NOT scroll is `fillWindow`'s case, and
            // its round caps exist for the thread shape that never grows a
            // scrollbar. Growing that from a gesture would have no cap at
            // all, and one trackpad flick is fifty wheel events.
            if (!transcriptScrolls(el)) return;
            reachForOlder();
        };
        const onWheel = (e: WheelEvent) => reachIfUp(e.deltaY < 0);
        const onTouchStart = (e: TouchEvent) => { lastTouchY = e.touches[0]?.clientY ?? null; };
        // A finger travelling DOWN the screen pulls older content into view.
        const onTouchMove = (e: TouchEvent) => {
            const y = e.touches[0]?.clientY ?? null;
            reachIfUp(lastTouchY !== null && y !== null && y > lastTouchY);
            lastTouchY = y;
        };
        const onTouchEnd = () => { lastTouchY = null; };
        // The transcript is `tabindex=0`, so a keyboard reader hits the same
        // wall: at the top these keys scroll nothing and fire no event. A
        // modifier makes it a shortcut rather than a scroll.
        //
        // `keydown` bubbles, and a scroll key on a DESCENDANT is the ordinary
        // case rather than the odd one: a choice card parks focus on its own
        // button, and the browser scrolls the nearest scrollable ancestor for
        // any key that control does not take. Asking whether the key was
        // CONSUMED separates the two, and this handler runs after the
        // control's own. A target check cannot: it would refuse the reader
        // exactly where focus usually sits.
        const onKeyDown = (e: KeyboardEvent) => {
            if (e.defaultPrevented) return;
            if (e.metaKey || e.ctrlKey || e.altKey) return;
            // Space pages DOWN and Shift+Space pages UP, so it is the one key
            // whose direction a modifier decides rather than the key itself.
            const space = e.key === ' ' || e.key === 'Spacebar';
            reachIfUp(space ? e.shiftKey : UPWARD_SCROLL_KEYS.has(e.key));
        };
        el.addEventListener('scroll', onScroll, { passive: true });
        el.addEventListener('wheel', onWheel, { passive: true });
        el.addEventListener('touchstart', onTouchStart, { passive: true });
        el.addEventListener('touchmove', onTouchMove, { passive: true });
        el.addEventListener('touchend', onTouchEnd, { passive: true });
        el.addEventListener('keydown', onKeyDown, { passive: true });
        return () => {
            unsubRelease();
            cancelAnimationFrame(releaseFrame);
            el.removeEventListener('scroll', onScroll);
            el.removeEventListener('wheel', onWheel);
            el.removeEventListener('keydown', onKeyDown);
            el.removeEventListener('touchstart', onTouchStart);
            el.removeEventListener('touchmove', onTouchMove);
            el.removeEventListener('touchend', onTouchEnd);
        };
    }, [threadId, eventsLoaded]);

    // After a window grow prepends older exchanges (content grows upward), restore
    // scrollTop by the height added so the viewport stays put, with no jump.
    //
    // Through `markAnchorScroll`, which is what that function exists for: a
    // correction landing near the top must not read as a request for older
    // turns. This was the one app write that skipped it. Bare, it lets a grow
    // fire the next grow off its own correction, whenever the added height
    // leaves the reader inside the expand margin.
    //
    // Keyed on the window TICK beside the edge, and the tick is the load-bearing
    // half. Every arming bumps it, so consumption cannot depend on the edge
    // STRING changing. It does not always: a backfill grows the array at the
    // front, so the re-point below can arm while index 0 stays index 0. The
    // capture then outlives its commit. Every grower early-returns on it, so
    // all three wedge for the rest of the mount, leaving the chevron as the
    // only thing that works. Same trap the scroll-to-top effect below carries.
    useLayoutEffect(() => {
        const el = areaRef.current;
        const pend = pendingExpandRef.current;
        if (!el || !pend) return;
        pendingExpandRef.current = null;
        markAnchorScroll(el, pend.prevScrollTop + (el.scrollHeight - pend.prevScrollHeight));
    }, [edgeKey, winTick]);

    // Leaving the thread abandons any capture taken for it. The reader's
    // position went with the unmount. Restoring against it on a later visit
    // would move a viewport they have since re-placed themselves in.
    //
    // The pending jump to the top goes with it, for the same reason and one
    // more: this component is mounted unkeyed, so the ref outlives the switch.
    // An arming the old thread never consumed would fire on the next thread's
    // first window write, and yank a reader who pressed nothing.
    useEffect(() => () => {
        if (threadId) historyHoldByThread.delete(threadId);
        if (threadId) wholeHistoryAskedByThread.delete(threadId);
        pendingScrollTopRef.current = false;
        // The VISIT ends here, however it ends: a switch, a New chat that
        // unmounts the pane, or the layout swapping at the breakpoint. Leaving
        // the mark set would make the next open a later commit of this visit,
        // and a render-all would outlive it after all. See `reseedOnReopen`.
        if (lastSeededVisit?.threadId === threadId && lastSeededVisit.owner === visitOwner) {
            lastSeededVisit = null;
        }
    }, [threadId]);

    // History folded in, so the array grew at the FRONT and the edge index now
    // names an older turn than the reader was on. Put it back on the turn it
    // named, by identity, then grow ONCE into what arrived.
    //
    // The grow is not a flourish. The reader asked by scrolling to the top, so
    // they sit at `scrollTop` zero with nothing left to scroll. Re-pointing
    // alone draws the same turns and moves nothing. No scroll event follows, so
    // the next round never arrives and the transcript stalls a page short of
    // its start.
    //
    // A layout effect, so the frame where the edge still names the wrong turn
    // is corrected before it is painted. The scroll restore is left to
    // `growRenderWindow` and the anchor effect above, the mechanism every other
    // expansion already rides.
    useLayoutEffect(() => {
        if (!threadId) return;
        const pend = historyHoldByThread.get(threadId);
        const el = areaRef.current;
        if (!pend || !el) return;
        historyHoldByThread.delete(threadId);
        // A transcript laid out at 0x0 measures nothing, the same guard the
        // growers keep. A pane collapsed between the request and the landing
        // gives one, and a height read there is zero rather than absent: the
        // hold would take the reader to the top of a pane they are not looking
        // through. Hold nobody instead, and leave the window to the observer.
        const hold = isElementVisible(el) ? pend : null;
        const anchored = anchorAfterBackfill(exchanges, pend);
        if (!anchored) {
            // No turn to re-point the WINDOW onto: either none survived the
            // fold, or the hold named none. So hold the reader where they are
            // and leave the window alone.
            if (hold) markAnchorScroll(el, holdTargetTop(el, hold));
            return;
        }
        storeEdge(threadId, anchored, exchanges);
        // A read that lands with no landing gate while the reader holds the
        // scrollbar. The grow waits for the release: a held drag undoes its
        // anchor write.
        const grew = !isScrollbarHeld(el) && growRenderWindow(
            el, threadId, exchanges, exchangeCostsRef.current, anchored,
            rowsDrawnAtRef.current, pendingExpandRef, bumpWindow,
        );
        // The hold comes from the REQUEST, never from here, and overwriting
        // what the grow just captured is the point. This effect runs after the
        // commit that folded the page in, so a reading taken now describes a
        // viewport the landing already moved. Held against it, a reader
        // restored onto their turn kept an offset two turns earlier. The
        // request's capture is the last reading of the frame they were in, and
        // the scroll listener keeps it current until the page lands.
        pendingExpandRef.current = hold && {
            prevScrollTop: hold.prevScrollTop,
            prevScrollHeight: hold.prevScrollHeight,
        };
        // Nothing to grow into, so the re-point is the whole change. The bump
        // still owes the effect above its round, the hold being armed either
        // way.
        if (!grew) bumpWindow();
    }, [threadId, historyFolded]);

    // After the up-chevron sets a whole-thread render floor, jump to the genuine
    // top once the expanded list commits. `scrollToTop()` starts a tween, and
    // `honourGrowth` in scrollState stands every growth round down while one is
    // in flight. So the render-all's ResizeObserver cannot pin the reader back
    // to the bottom. Runs before paint so the user never sees the intermediate
    // position.
    //
    // Keyed on the window TICK, never on `edgeKey` alone. The chevron is now
    // offered on unloaded history too, and such a thread routinely has its
    // whole loaded set drawn already. The press then writes the edge it was
    // holding, so a dep on the edge's value never fires and the press reads as
    // a no-op. The second arming, once the history lands, is dropped the same
    // way. The ref guards the extra runs a tick dep brings.
    useLayoutEffect(() => {
        if (!pendingScrollTopRef.current) return;
        pendingScrollTopRef.current = false;
        scrollToTop();
    }, [edgeKey, winTick]);

    // The transcript must leave the reader able to REACH the rest of the
    // thread, and one shorter than the pane does not. The scroll handler above
    // is the only thing that grows the window or fetches a page, and only a
    // scroll event fires it. So a short transcript freezes on whatever the seed
    // took. That is the reported "an old thread opens on one card and will not
    // scroll". `fillAction` carries why the seed cannot prevent it, and which
    // of the two moves is owed.
    //
    // Both go through the same anchored primitives the scroll-up takes, so a
    // fill lands the reader on the content they were already looking at. On the
    // open that content is the tail, so the thread opens at its end with a full
    // pane. A later fill, when a pane resize leaves the transcript short again,
    // holds them still instead of dropping them into an older turn.
    const fillWindow = () => {
        const el = areaRef.current;
        // Settled, not `canSeedWindow`. That one also wants exchanges, and a
        // page whose events all fold to nothing renderable is exactly the case
        // owed a page. Waiting for the load to settle still matters: paging
        // while the first page is in flight asks for history twice.
        if (!el || !threadId || !(eventsLoaded || eventsLoadFailed)) return;
        // The re-entrancy guard the scroll-up shares: one grow is in flight
        // until the anchor effect lands it, and its metrics describe the DOM as
        // it stands now.
        if (pendingExpandRef.current) return;
        // Cheapest question first. It is pure arithmetic, and it rejects every
        // thread rendered whole to its first event, which is most of them. The
        // reads below force a layout, so they must not run on such a thread.
        const costs = exchangeCostsRef.current;
        const rows = rowsDrawnAtRef.current;
        const current = currentEdge(threadId, exchangesRef.current, costs, rows);
        if (!edgeHasMoreAbove(current) && !hasOlderEvents) return;
        // A transcript laid out at 0x0 answers every geometric question wrongly.
        // A collapsed desktop split gives one, so this is an ordinary state
        // rather than a corner. The observer below is what brings the
        // measurement back once such a pane is revealed.
        if (!isElementVisible(el)) return;
        const owed = fillAction(el, current, hasOlderEvents);
        if (owed === 'none') return;
        // Settle the last round against what the window draws now: one that
        // moved something in yet drew nothing is refunded (`settleFillLedger`).
        const reading = {
            drawn: drawnRowsInWindow(current, exchangesRef.current.length, rows),
            floor: eventThread?.historyFloor?.sequence ?? Infinity,
        };
        // Not while a read is running: a page that has not landed has moved
        // nothing yet, and settling it now would keep it charged for good.
        const stored = fillLedgerByThread.get(threadId) ?? EMPTY_FILL_LEDGER;
        const ledger = threadHistoryReadInFlight(threadId) ? stored : settleFillLedger(stored, reading);
        fillLedgerByThread.set(threadId, ledger);
        if (owed === 'page') {
            // Nothing loaded is left to render, so grow the LOADED SET instead.
            // Charged apart from the render rounds below, a page being a request
            // rather than a fold. The re-point effect starts the next round.
            if (!fillRoundAllowed(ledger, 'page')) return;
            // Asked BEFORE the request, never sorted out after it. The store
            // reports a refusal exactly as it reports a failure. Counting
            // afterwards therefore either spends a round on a request that
            // never ran, or spends none on one that ran and failed. The second
            // leaves a failing endpoint retried for ever, one toast a round.
            //
            // A read that lands with rows re-drives this effect. One that
            // FAILS mutates nothing, so the fill waits for a resize or a
            // scroll instead. The chevron is the way out meanwhile, which is
            // why it reads the same watermark this arm does.
            if (threadHistoryReadInFlight(threadId)) return;
            // Charged only when the capture was taken, the rule the grow below
            // keeps.
            if (requestBackfill(el, threadId, exchangesRef.current, current, onHistoryRead)) {
                fillLedgerByThread.set(threadId, chargeFillRound(ledger, 'page', reading));
            }
            return;
        }
        if (!fillRoundAllowed(ledger, 'grow')) return;
        // Charged only when it actually grew, so a no-op cannot spend a round.
        if (growRenderWindow(el, threadId, exchangesRef.current, costs, current, rows, pendingExpandRef, bumpWindow)) {
            fillLedgerByThread.set(threadId, chargeFillRound(ledger, 'grow', reading));
        }
    };
    // Held in a ref so the observer effect below can stay keyed on the thread
    // alone while still calling THIS render's closure.
    const fillWindowRef = useRef(fillWindow);
    fillWindowRef.current = fillWindow;

    // Measure after every commit that can change the answer. A grow changes
    // `edgeKey`, so the rounds run off these deps.
    //
    // `historyFolded` is what carries the PAGE rounds, and it is not a
    // duplicate of the two beside it. A page can land without changing either:
    // its turns merge into the fragment already on screen, so the count holds,
    // and the re-point can land on the same edge.
    //
    // The FOLD count, never `historyReadSettled`. A read that folded nothing
    // mutated nothing, so the fill has no new answer and would only re-ask a
    // failing endpoint, one toast a round.
    //
    // `winTick` is what asks AGAIN once a capture is consumed. This effect runs
    // in the same commit the re-point arms one in, so it stands down on the
    // re-entrancy guard. No other dep has to change afterwards.
    //
    // The three view settings change what the window DRAWS, so they change the
    // answer too. Hiding steps can leave a filled pane short.
    useLayoutEffect(() => { fillWindowRef.current(); },
        [threadId, canSeedWindow, eventsLoaded, eventsLoadFailed, hasOlderEvents,
            edgeKey, exchanges.length, historyFolded, winTick, showSteps, showDetails, folds]);

    // The window must also reach the turn the reader PARKED on, which the seed
    // has no reason to have taken. A *reading position* names a turn, and the
    // restore cannot place the reader until that turn is in the DOM. So this
    // walks the window up to it and the restore lands off the growth. ADR 0152
    // (docs/adr/) is why a turn, and why the walk is chunked and uncapped.
    //
    // A round per FRAME, and the frame is the mitigation rather than a detail.
    // A grow bumps state from a layout effect, and Preact flushes that render
    // on a MICROTASK. Rounds driven off the commit therefore chain into one
    // task with no paint between them, which is the blocking render ADR 0081
    // forbids. `requestAnimationFrame` is the browser's own "you may paint now".
    //
    // The target is read ONCE per thread. Re-reading would chase the reader's
    // own new position up the list as they scroll.
    useLayoutEffect(() => {
        const record = threadId ? readSavedScroll(threadScrollKey(threadId)) : null;
        anchorTargetRef.current = readingTargetOf(record);
        readingChasesRef.current = 0;
    }, [threadId]);
    const reachAnchor = () => {
        const el = areaRef.current;
        const target = anchorTargetRef.current;
        // `eventsLoaded` is the hook's own `paused` read the other way round. A
        // FAILED load satisfies `canSeedWindow` but attaches no restore, and a
        // walk with no restore behind it renders markdown for nobody.
        if (!el || !threadId || !canSeedWindow || !eventsLoaded || !target) return;
        // A DEEP LINK owns the open, and renders the thread whole to do it. So
        // there is nothing here to reach, and a round taken now would arm
        // `pendingExpandRef` against a `renderFromIndex` the claim pins at 0.
        // Nothing would ever consume it, wedging every grower for this mount.
        // The target is KEPT: the link may end without having placed anybody,
        // and the rescue that covers it reads the same record.
        if (deepLinkRenderAll.value) return;
        // The re-entrancy guard the other growers share: one grow is in flight
        // until the anchor effect lands it.
        if (pendingExpandRef.current) return;
        const costs = exchangeCostsRef.current;
        const rows = rowsDrawnAtRef.current;
        const current = currentEdge(threadId, exchanges, costs, rows);
        const { index, row } = locateReadingTarget(exchanges, target);
        // Not in the loaded pages: chase it, within `MAX_READING_CHASES`
        // (`readingChaseAction` holds the decision, ADR 0234 the bound).
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
        // Cheapest question first, the rule the fill above states: the layout
        // read below must not run on a walk that is already over. Clearing the
        // target is what stops a later commit asking again.
        //
        // A row needs only itself drawn. A turn needs to be WHOLE, since the
        // restore measures that turn's own top edge.
        if (!edgeMustReachRow(current, index, row)) {
            anchorTargetRef.current = null;
            return;
        }
        // A transcript laid out at 0x0 measures nothing, and `growRenderWindow`
        // snapshots its geometry for the correction. Same guard, same reason, as
        // the fill above, and the same recovery: the target is KEPT and the
        // observer below re-asks once the pane has a box.
        if (!isElementVisible(el)) return;
        // A grow that took nothing is the fourth way the walk is over.
        if (!growRenderWindow(el, threadId, exchanges, costs, current, rows, pendingExpandRef, bumpWindow)) {
            anchorTargetRef.current = null;
        }
    };
    const reachAnchorRef = useRef(reachAnchor);
    reachAnchorRef.current = reachAnchor;
    // `historyFolded` too. A chase page that only extends the turn already on
    // screen changes neither the exchange count nor the edge. The row it
    // brought in would otherwise never be looked for.
    useLayoutEffect(() => {
        const frame = requestAnimationFrame(() => reachAnchorRef.current());
        return () => cancelAnimationFrame(frame);
    }, [threadId, canSeedWindow, edgeKey, exchanges.length, historyFolded]);

    // And ask BOTH again when the pane's own box changes, which is the one
    // trigger the deps above cannot see. Each bails on an unmeasurable
    // transcript, an ordinary state a collapsed desktop split produces, and
    // neither has another way back. The restore holds its own wait open across
    // the same state (`onDeadline`), so a pane revealed later still lands the
    // reader. Keyed on the thread, so a streaming append does not churn the
    // observer. `threadInMap` too: the cold-open return carries no `areaRef`,
    // so the first run finds nothing and must re-run once the thread lands.
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
        // Don't unfocus if threads haven't loaded yet — the thread may appear
        // once loadAllThreads completes. Only unfocus after threads are loaded.
        // revealPane:false — this is stale-pointer cleanup during render, not a
        // navigation. ThreadView is mounted in the background on mobile (all
        // three panes mount), so revealing the thread pane here would swipe a
        // user on the content pane away mid-render.
        //
        // An AWAITED thread is exempt, because its absence is expected rather
        // than stale. Two ways in: `focusThreadOrBootstrapResult` focuses
        // optimistically while the metadata is in flight, and
        // `focusSpawnedThread` focuses a thread whose row is still on its way
        // over SSE. Cleaning either up here undoes the focus on the next render
        // and leaves the user on the compose view. The bootstrap restores the
        // prior focus itself if the thread turns out not to exist.
        if (threadsLoaded.value && awaitedThreadId.value !== threadId) {
            unfocusThread({ revealPane: false });
        }
        // Waiting for the thread to appear in the map — same delayed-spinner
        // empty state as the events-loading path, including the 8s "Taking too
        // long? Tap to reload" escape hatch. This covers the window between
        // page load (localStorage hydrates focusedThreadId) and loadAllThreads
        // completing (thread enters map). On iOS Safari PWA cold start this
        // can be several seconds.
        // While we wait for the thread to enter the map, honour the disconnected
        // signal too: a cold boot into an unreachable engine never populates the
        // map, so without this the skeleton would spin into "Tap to reload".
        const waitingReason: EmptyReason = connectionStatus.value === 'disconnected'
            ? { kind: 'disconnected', threadId }
            : { kind: 'loading', threadId };
        // KEYED, and the keys are the whole reason this shape works. The
        // return below is a DIFFERENT tree: it leads with a header, so an
        // unkeyed diff matches by index, finds a header where this wrap was,
        // and rebuilds the subtree. The skeleton overlay is what that costs. A
        // remounted element cannot run a CSS transition, so its crossfade
        // becomes a snap, and a cold open is the ONLY way in: the thread is
        // absent from the map until `loadAllThreads` lands, which on an iOS PWA
        // is nearly every open and on desktop is nearly none.
        //
        // Matched by key instead, the wrap and the overlay survive the switch
        // and keep animating. Pinned by `thread-view-frame-identity.test.ts`.
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

    const threadTitle = threadDisplayTitle(eventThread);
    const visualStatus = threadVisualStatus(eventThread);
    // Is there a feed to draw? The empty state is the other branch, and it
    // centres itself rather than flowing from the top.
    const hasFeed = exchanges.length > 0;
    return (
        <div class="thread-view">
            <div class="thread-view-header">
                <ThreadStatusIcon status={visualStatus} />
                <ThreadTitleEditor key={threadId} threadId={threadId} title={threadTitle} />
                <span class="thread-view-header-actions">
                    {eventThread.meta.state !== 'composing' && (
                        <PinThreadButton threadId={threadId} saved={eventThread.meta.saved} />
                    )}
                    <ThreadOverflowMenu threadId={threadId} title={threadTitle} />
                </span>
            </div>
            {/* `has-scroll-indicator` is what licenses the CSS to hide the
                native scrollbar on this scroller: the suppression is scoped to
                a wrap that actually carries a replacement, so a transcript can
                never end up with no scroll feedback at all. */}
            {/* Keyed to match the no-thread tree above, so the cold-open switch
                between them reuses these nodes rather than rebuilding them. See
                the comment on that return for what the rebuild costs. */}
            <div class="thread-content-wrap has-scroll-indicator" key="wrap">
                {/* tabindex makes the transcript a keyboard-focusable scroll
                    region: once focused (via Tab or the ⌘↑/⌘↓ turn shortcuts) the
                    native Arrow/PageUp/PageDown/Home/End/Space keys scroll it. */}
                <div class="thread-content visible" key="content" ref={areaRef} tabIndex={0} role="region" aria-label="Thread transcript" data-native-context-menu>
                    <MobileThreadTitleBar />

                    {hasFeed ? (
                        // THE FEED IS ITS OWN BOX, so the turns can be addressed
                        // as a group. The scroll container cannot stand in for
                        // it: on mobile that also holds the header spacer and
                        // the sticky title. Two things select through this box,
                        // the last-turn padding rule and the reading position.
                        // It carries no alignment and no floor, and
                        // `.thread-feed` in chat/input-messages.css says why.
                        <div class="thread-feed" key="feed">
                            {renderExchanges(exchanges, threadId!, streamingBuffer, renderFromIndex, edge.rowsHidden)}
                            {/* Last, and boxed, as `readScrollAnchor` requires of
                                every non-turn child in the feed. */}
                            <StoppedChildNotice meta={eventThread.meta} />
                        </div>
                    ) : (
                        <ThreadEmptyState key={threadId} reason={emptyReason(animating, eventsLoaded, eventsLoadFailed, hasContentEvents(eventThread.events), threadId!, connectionStatus.value === 'disconnected', isMidTurn(effectiveThreadStatus(eventThread)))} />
                    )}
                </div>
                <ThreadSkeletonOverlay key="skeleton" show={showThreadSkeleton} />
                {/* Presentational only (aria-hidden): the transcript is already a
                    labelled scroll region and a screen reader announces position
                    from that, not from a decorative bar. */}
                <div class="thread-scroll-indicator" ref={setIndicatorTrack} aria-hidden="true">
                    <div class="thread-scroll-thumb" ref={setIndicatorThumb} />
                </div>
                <ScrollControls
                    /* `notAtTop` alone is the WRONG question now that a thread
                       with no saved position opens at the top of the rendered
                       tail: the reader sits at scrollTop 0 with older turns
                       above them that are not in the DOM, so the chevron
                       (whose handler renders the full thread first) hid exactly
                       when it was the only way to reach them. `anythingAbove`
                       carries the other two terms and why each is needed. */
                    showUp={anythingAbove(isNotAtTop, edge, hasOlderEvents)}
                    showDown={isUp}
                    onScrollUp={() => {
                        // Windowed thread with older exchanges still above the
                        // rendered tail: render the FULL thread first, then jump to
                        // the true top once it commits (see the layout effect above).
                        // Without render-all the top of the rendered tail isn't the
                        // top of the thread, and the scroll-up window-expand re-anchors
                        // the viewport partway (the "needed N clicks" bug).
                        // `edge` is this render's effective window, so a
                        // deep-link render-all already answers false here. A
                        // clamped floor turn answers TRUE: the true top is that
                        // turn's first row, not the first row on screen.
                        //
                        // Unloaded history answers TRUE as well, which is the
                        // second argument.
                        if (threadId && scrollToTopNeedsRenderAll(edge, hasOlderEvents)) {
                            pendingScrollTopRef.current = true;
                            storeRenderAll(threadId);
                            // The true top is the thread's first turn, not the
                            // first one this client holds. Re-arm AFTER the
                            // history lands: the jump below runs on the next
                            // commit, which on a paged thread is the top of the
                            // page rather than of the thread.
                            //
                            // Only while the reader is still HERE. One unpaged
                            // fetch on a long thread takes seconds on a phone,
                            // and the transcript element is shared across
                            // threads. Re-arming blind drags whatever they
                            // opened meanwhile to its top, and cancels any
                            // landing that thread had in flight. The teardown
                            // below cannot cover it: it runs at switch time,
                            // which is before this resolves.
                            //
                            // It settles like every other read of older
                            // history, so a hold taken for ITS fold is
                            // consumed. This press is not the only way into
                            // one: a reader returning mid-fetch takes a hold
                            // for whatever read is running.
                            const el = areaRef.current;
                            void ensureWholeThreadLoaded(threadId, el ? () => scrollbarReleased(el) : undefined).then((added) => {
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
