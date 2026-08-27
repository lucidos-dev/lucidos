import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { focusedThreadId, threadMap, activeStreamingBuffer, threadsLoaded, bootstrappingThreadId, promptAnimating, revealOnFocus, connectionStatus, scaledDurationMs } from '../../store/store';
import { getThreadEventsBump } from '../../store/threadActivity';
import { unfocusThread } from '../../store/actions/threads';
import { loadThreadEvents, forceRetryThreadEvents } from '../../store/actions/thread-loading';
import { checkConnection } from '../../store/actions/connection';
import { gatewayPickerHref } from '../../utils/basePath';
import { replaceDocument } from '../../utils/documentNavigation';
import { rebuildCorruptedThreadEvents } from '../../store/actions/thread-sync';
import { useScrollObservers, renderExchanges, ScrollControls } from './CreateThreadView';
import { ThreadStatusIcon, threadVisualStatus } from '../shared/ThreadStatusIcon';
import { ThreadTitleEditor } from './ThreadTitleEditor';
import { PinThreadButton } from '../shared/PinThreadButton';
import { ThreadOverflowMenu } from '../shared/ThreadOverflowMenu';
import { MobileThreadTitleBar } from '../layout/MobileAppHeader';
import { computeExchanges, hasContentEvents } from '../../store/thread-events';
import { awayFromBottom, notAtTop, scrollToBottomAnimated, scrollToTop, hasPendingEventScroll, isElementVisible, isNavigationScroll, deepLinkRenderAll } from './scrollState';
import { INITIAL_WINDOW, MAX_FILL_EXPANSIONS, canSeedRenderWindow, computeRenderFromIndex, hasMoreAbove, expandRenderCount, exchangeRenderCost, seedRenderCount, windowNeedsFill, WINDOW_EXPAND_MARGIN_PX, scrollToTopNeedsRenderAll } from './threadWindow';
import { useScrollMemory, threadScrollKey } from '../../hooks/useScrollMemory';
import { useThreadScrollIndicator } from '../../hooks/useThreadScrollIndicator';
import { useDelayedFlag, useLingeringFlag } from '../../hooks/useDelayedLoading';
import { ThreadSkeleton } from './ThreadSkeleton';
import { showThreadSkeletonNow, threadIsLoadingNow, type ThreadLoadingState } from './threadSkeletonGate';
import { forceWebKitRepaint, forceWebKitRepaintBurst, createRepaintThrottle } from '../../utils/webkitRepaint';
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

// Per-thread render-window size (count of trailing exchanges to render), kept at
// module scope so it survives a switch-away-and-back — see the windowing block in
// ThreadView. `Infinity` = render all (set by a deep-link). Session-scoped; one
// small int per visited thread.
const renderCountByThread = new Map<string, number>();

/** Fill rounds spent per thread, beside the window they grew. Module-scoped for
 *  the same reason that Map is: the window survives a switch-away-and-back, so a
 *  per-mount counter would hand every revisit a fresh `MAX_FILL_EXPANSIONS` and
 *  compound the cost the cap exists to bound. */
const fillRoundsByThread = new Map<string, number>();

/** Scroll metrics captured just before a window grow, so the anchor effect can
 *  restore `scrollTop` by the height added. */
type PendingExpand = { prevScrollHeight: number; prevScrollTop: number } | null;

/** This thread's window right now: the stored count, or the seed until one is
 *  stored. Three callers read it and must agree: the render, the scroll-up
 *  expansion and the fill. */
function currentRenderCount(threadId: string, costs: readonly number[]): number {
    return renderCountByThread.get(threadId) ?? seedRenderCount(costs);
}

/** Grow this thread's window by one budgeted step, holding the reader on the
 *  content they were looking at. Two callers, differing only in what asks: the
 *  scroll-up expansion and the fill.
 *
 *  Prepending older exchanges grows the list upward, so it captures the metrics
 *  the anchor effect needs. That effect is keyed on `renderFromIndex`, which is
 *  why the anchor is armed only when the count ACTUALLY grows. A no-op grow
 *  would leave the ref set, and the re-entrancy guard both callers share would
 *  then wedge every later expansion. Reports whether it grew. */
function growRenderWindow(
    el: HTMLElement,
    threadId: string,
    costs: readonly number[],
    current: number,
    pending: { current: PendingExpand },
    bump: () => void,
): boolean {
    const next = expandRenderCount(costs, current);
    if (next === current) return false;
    pending.current = { prevScrollHeight: el.scrollHeight, prevScrollTop: el.scrollTop };
    renderCountByThread.set(threadId, next);
    bump();
    return true;
}

/** Escalating retry delays for the empty-thread safety retry. */
const EMPTY_THREAD_RETRY_DELAYS = [500, 2000, 5000];

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
): EmptyReason {
    if (animating) return { kind: 'animating' };
    if (eventsLoadFailed) return { kind: 'failed', threadId };
    if (eventsLoaded && hasContent) return { kind: 'corrupt', threadId };
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
                    <button class="action-btn" onClick={() => forceRetryThreadEvents(reason.threadId)}>Retry</button>
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
                    <button class="action-btn" onClick={() => { void checkConnection(); forceRetryThreadEvents(reason.threadId); }}>Retry</button>
                    {pickerHref && (
                        <button class="thread-empty-reload" onClick={() => replaceDocument(pickerHref)}>
                            Back to workspaces
                        </button>
                    )}
                </div>
            );
        }
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
    // open-start was stamped when this thread's real event load began
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

    // --- Thread-render windowing (perf) ---
    // A large focused thread used to render — and markdown-parse — every exchange
    // synchronously on open and on every re-render (measured ~270–500ms of pure
    // JS). Render only a TAIL of the list and grow it on scroll-up. The per-thread
    // render count lives in a module Map (`renderCountByThread`) so it SURVIVES a
    // switch-away-and-back — otherwise returning to a thread you'd scrolled up in
    // would re-window to the tail and useScrollMemory couldn't restore your spot.
    // A new thread starts at the SEEDED window: the newest exchanges fitting the
    // step budget, capped at INITIAL_WINDOW. Its first render is already windowed,
    // never a one-off full render. A notification deep-link forces the full list
    // instead, so a windowed-out target can render. The persist effect below then
    // keeps it full, so clearing the claim doesn't snap the user back.
    // See threadWindow.ts + scrollState.deepLinkRenderAll.
    const [, bumpWin] = useState(0);
    // Re-render after a window write. `bumpWin` is stable, so an effect holding
    // an older copy of this still reaches the current component.
    const bumpWindow = () => bumpWin(n => n + 1);
    const exchangeCosts = useMemo(() => exchanges.map(exchangeRenderCost), [exchanges]);
    let renderCount = threadId ? currentRenderCount(threadId, exchangeCosts) : INITIAL_WINDOW;
    if (deepLinkRenderAll.value) renderCount = Infinity;
    const renderFromIndex = computeRenderFromIndex(exchanges.length, renderCount);

    // Persist the deep-link "render all" so the thread stays fully rendered after
    // the claim clears (no snap back to the tail while the user reads an old event).
    useEffect(() => {
        if (threadId && deepLinkRenderAll.value && renderCountByThread.get(threadId) !== Infinity) {
            renderCountByThread.set(threadId, Infinity);
            bumpWin(n => n + 1);
        }
    }, [threadId, deepLinkRenderAll.value]);

    // Fix the window once the event load settles, so later renders read a stored
    // value instead of re-deriving one. The seed is a function of the exchange
    // costs, and a live turn's cost grows with every streamed step. A derived
    // window would therefore push older turns off the top while the reader
    // watches. Write-once: the entry only ever grows afterwards, from the
    // scroll-up expansion, the chevron's render-all, or the effect above.
    //
    // `canSeedRenderWindow` is what keeps that write-once off a fragment. A
    // LAYOUT effect, so this stores the seed before the browser paints the
    // render that computed it. A plain effect runs after paint, leaving a gap
    // where a streamed step could re-derive a different window first.
    const canSeedWindow = canSeedRenderWindow({
        hasExchanges: exchanges.length > 0,
        eventsLoaded,
        eventsLoadFailed,
    });
    useLayoutEffect(() => {
        if (threadId && canSeedWindow && !renderCountByThread.has(threadId)) {
            renderCountByThread.set(threadId, seedRenderCount(exchangeCosts));
        }
    }, [threadId, canSeedWindow]);

    // Latest exchange costs for the scroll handler and the up-chevron, so neither
    // re-attaches on every streaming append. Their length is the exchange count.
    const exchangeCostsRef = useRef<number[]>([]);
    exchangeCostsRef.current = exchangeCosts;
    // Armed by `growRenderWindow`, consumed by the anchor effect below.
    const pendingExpandRef = useRef<PendingExpand>(null);
    // Set by the up-chevron when it renders the full thread before scrolling to
    // the genuine top — consumed by the layout effect that performs the jump once
    // the expanded list commits. See onScrollUp below.
    const pendingScrollTopRef = useRef(false);

    // Eligible for slide-in? revealOnFocus checked in the layout effect only
    // to avoid subscribing the render to the signal (prevents extra re-renders).
    const shouldReveal = shouldRevealThread(threadId, animating, hasContent);

    const areaRef = useRef<HTMLDivElement>(null);
    const isUp = awayFromBottom.value;
    const isNotAtTop = notAtTop.value;

    // The mobile transcript draws its own scroll indicator, because the native
    // overlay one spans a box that starts behind the fixed header (see
    // components/chat/scrollIndicator.ts). No-op on desktop.
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
        renderFromIndex,
        totalExchanges: exchanges.length,
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

    // Watchdog: if a focused thread still has no content after 2 seconds,
    // force-retry event loading. This catches ANY stuck state — not just
    // the specific race conditions patched individually before.
    // forceRetryThreadEvents has its own retry cap (once per thread) and
    // skips if a load is already in-flight, preventing infinite loops.
    useEffect(() => {
        if (!threadId || !threadInMap || eventsLoaded || eventsLoadFailed) return;
        const timer = setTimeout(() => {
            forceRetryThreadEvents(threadId);
        }, 2000);
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
            // A notification deep-link (toast / push / inbox) owns the scroll
            // when focusing an UNfocused thread: skip both the restore and the
            // top reset so neither fires after scrollToEventAndPulse and snaps
            // away from the source event. The claim is held until the
            // deep-link's deadline, so this is true for the whole resolve
            // window. (Already-focused threads don't re-run this hook, so they
            // never needed the guard, which is why the bug only bit
            // unfocused-thread deep-links.)
            shouldRestore: () => !hasPendingEventScroll(),
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
        const onScroll = () => {
            if (pendingExpandRef.current) return;
            // Only the READER asking for older turns may grow the window. Our
            // own positioning fires scroll events too, and opening a thread now
            // lands at the TOP of the rendered slice, which is inside the
            // margin by construction: without this the open would grow the
            // window by a chunk on every visit, markdown-parsing it
            // synchronously on the open path and compounding across re-opens
            // (renderCountByThread is module-scoped), which is exactly the cost
            // windowing exists to avoid. The up-chevron is covered too, and
            // wants to be: it renders the full thread itself before gliding, so
            // an expansion mid-glide would re-anchor the viewport and stall it.
            if (isNavigationScroll(el)) return;
            if (el.scrollTop > WINDOW_EXPAND_MARGIN_PX) return;
            const costs = exchangeCostsRef.current;
            const current = currentRenderCount(threadId, costs);
            if (!hasMoreAbove(costs.length, current)) return;
            growRenderWindow(el, threadId, costs, current, pendingExpandRef, bumpWindow);
        };
        el.addEventListener('scroll', onScroll, { passive: true });
        return () => el.removeEventListener('scroll', onScroll);
    }, [threadId, eventsLoaded]);

    // After a window grow prepends older exchanges (content grows upward), restore
    // scrollTop by the height added so the viewport stays put — no jump.
    useLayoutEffect(() => {
        const el = areaRef.current;
        const pend = pendingExpandRef.current;
        if (!el || !pend) return;
        pendingExpandRef.current = null;
        el.scrollTop = pend.prevScrollTop + (el.scrollHeight - pend.prevScrollHeight);
    }, [renderFromIndex]);

    // After the up-chevron renders the full thread (set renderCount = Infinity),
    // jump to the genuine top once the expanded list commits. scrollToTop() forces
    // _resizeMode='ignore' first, so the huge render-all ResizeObserver grow can't
    // hit onResize's scroll-mode branch and pin us back to the bottom. Runs before
    // paint so the user never sees the intermediate position.
    useLayoutEffect(() => {
        if (!pendingScrollTopRef.current) return;
        pendingScrollTopRef.current = false;
        scrollToTop();
    }, [renderFromIndex]);

    // The window must leave the reader able to REACH what it left out, and one
    // shorter than the pane does not. The scroll-up expansion above is the only
    // way older exchanges enter it, and only a scroll event fires that. So the
    // transcript freezes on the seed, with the up chevron as the sole way out.
    // That is the reported "an old thread opens on one card and will not
    // scroll". `windowNeedsFill` carries why the seed cannot prevent it.
    //
    // The grow goes through the same anchored primitive the scroll-up takes, so
    // a fill lands the reader on the content they were already looking at. On
    // the open that content is the tail, so the thread opens at its end with a
    // full pane. A later fill, when a pane resize leaves the transcript short
    // again, holds them still instead of dropping them into an older turn.
    const fillWindow = () => {
        const el = areaRef.current;
        if (!el || !threadId || !canSeedWindow) return;
        // The re-entrancy guard the scroll-up shares: one grow is in flight
        // until the anchor effect lands it, and its metrics describe the DOM as
        // it stands now.
        if (pendingExpandRef.current) return;
        // Cheapest question first. It is pure arithmetic, and it rejects every
        // thread already rendered whole, which is most of them. The reads below
        // force a layout, so they must not run on a thread that cannot fill.
        const costs = exchangeCostsRef.current;
        const current = currentRenderCount(threadId, costs);
        if (!hasMoreAbove(costs.length, current)) return;
        const rounds = fillRoundsByThread.get(threadId) ?? 0;
        if (rounds >= MAX_FILL_EXPANSIONS) return;
        // A transcript laid out at 0x0 answers every geometric question wrongly.
        // A collapsed desktop split gives one, so this is an ordinary state
        // rather than a corner. The observer below is what brings the
        // measurement back once such a pane is revealed.
        if (!isElementVisible(el)) return;
        if (!windowNeedsFill(el, costs.length, current)) return;
        // Counted only when it actually grew, so a no-op cannot spend a round.
        if (growRenderWindow(el, threadId, costs, current, pendingExpandRef, bumpWindow)) {
            fillRoundsByThread.set(threadId, rounds + 1);
        }
    };
    // Held in a ref so the observer effect below can stay keyed on the thread
    // alone while still calling THIS render's closure.
    const fillWindowRef = useRef(fillWindow);
    fillWindowRef.current = fillWindow;

    // Measure after every commit that can change the answer. A grow changes
    // `renderFromIndex`, so the rounds run off these deps.
    useLayoutEffect(() => { fillWindowRef.current(); },
        [threadId, canSeedWindow, renderFromIndex, exchanges.length]);

    // And measure again when the pane's own BOX changes, which is the one
    // trigger the deps above cannot see. Keyed on the thread alone, so a
    // streaming append does not churn the observer.
    useLayoutEffect(() => {
        const el = areaRef.current;
        if (!el || !threadId) return;
        const observer = new ResizeObserver(() => fillWindowRef.current());
        observer.observe(el);
        return () => observer.disconnect();
    }, [threadId]);

    if (!threadId) return null;

    if (!eventThread) {
        // Don't unfocus if threads haven't loaded yet — the thread may appear
        // once loadAllThreads completes. Only unfocus after threads are loaded.
        // revealPane:false — this is stale-pointer cleanup during render, not a
        // navigation. ThreadView is mounted in the background on mobile (all
        // three panes mount), so revealing the thread pane here would swipe a
        // user on the content pane away mid-render.
        //
        // A thread being bootstrapped is exempt: focusThreadOrBootstrapResult
        // focuses it optimistically so the tap is acknowledged while its
        // metadata is in flight, and it is absent from the map for exactly that
        // reason. Cleaning it up here would undo the focus on the next render
        // and put the dead interval straight back. The bootstrap restores the
        // prior focus itself if the thread turns out not to exist.
        if (threadsLoaded.value && bootstrappingThreadId.value !== threadId) {
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
                    <div class="thread-content visible" key="content">
                        <ThreadEmptyState key={threadId} reason={waitingReason} />
                    </div>
                    <ThreadSkeletonOverlay key="skeleton" show={showThreadSkeleton} />
                </div>
            </div>
        );
    }

    const threadTitle = threadDisplayTitle(eventThread);
    const visualStatus = threadVisualStatus(eventThread);
    return (
        <div class="thread-view">
            <div class="thread-view-header">
                <ThreadStatusIcon status={visualStatus} />
                <ThreadTitleEditor threadId={threadId} title={threadTitle} />
                <span class="thread-view-header-actions">
                    {eventThread.meta.state !== 'composing' && (
                        <PinThreadButton threadId={threadId} saved={eventThread.meta.saved} />
                    )}
                    <ThreadOverflowMenu threadId={threadId} title={threadTitle} />
                </span>
            </div>
            {/* `has-scroll-indicator` is what licenses mobile.css to suppress the
                native overlay indicator on this scroller: the suppression is
                scoped to a wrap that actually carries a replacement, so a
                transcript can never end up with no scroll feedback at all. */}
            {/* Keyed to match the no-thread tree above, so the cold-open switch
                between them reuses these nodes rather than rebuilding them. See
                the comment on that return for what the rebuild costs. */}
            <div class="thread-content-wrap has-scroll-indicator" key="wrap">
                {/* tabindex makes the transcript a keyboard-focusable scroll
                    region: once focused (via Tab or the ⌘↑/⌘↓ turn shortcuts) the
                    native Arrow/PageUp/PageDown/Home/End/Space keys scroll it. */}
                <div class="thread-content visible" key="content" ref={areaRef} tabIndex={0} role="region" aria-label="Thread transcript">
                    <MobileThreadTitleBar />

                    {exchanges.length === 0 ? (
                        <ThreadEmptyState key={threadId} reason={emptyReason(animating, eventsLoaded, eventsLoadFailed, hasContentEvents(eventThread.events), threadId!, connectionStatus.value === 'disconnected')} />
                    ) : (
                        renderExchanges(exchanges, threadId!, streamingBuffer, renderFromIndex)
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
                       when it was the only way to reach them. Offer it whenever
                       there is anything above, scrolled or windowed out. */
                    showUp={isNotAtTop || hasMoreAbove(exchanges.length, renderCount)}
                    showDown={isUp}
                    onScrollUp={() => {
                        // Windowed thread with older exchanges still above the
                        // rendered tail: render the FULL thread first, then jump to
                        // the true top once it commits (see the layout effect above).
                        // Without render-all the top of the rendered tail isn't the
                        // top of the thread, and the scroll-up window-expand re-anchors
                        // the viewport partway (the "needed N clicks" bug).
                        // `renderCount` is this render's effective window, so a
                        // deep-link render-all already answers false here.
                        if (threadId && scrollToTopNeedsRenderAll(exchanges.length, renderCount)) {
                            pendingScrollTopRef.current = true;
                            renderCountByThread.set(threadId, Infinity);
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
