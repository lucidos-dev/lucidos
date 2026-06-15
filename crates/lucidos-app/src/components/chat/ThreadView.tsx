import { useEffect, useLayoutEffect, useMemo, useRef } from 'preact/hooks';
import { focusedThreadId, threadMap, activeStreamingBuffer, threadsLoaded, promptAnimating, revealOnFocus } from '../../store/store';
import { getThreadEventsBump } from '../../store/threadActivity';
import { unfocusThread } from '../../store/actions/threads';
import { loadThreadEvents, forceRetryThreadEvents } from '../../store/actions/thread-loading';
import { rebuildCorruptedThreadEvents } from '../../store/actions/thread-sync';
import { useAutoScroll, renderExchanges, ScrollControls } from './CreateThreadView';
import { ThreadStatusIcon, threadVisualStatus } from '../shared/ThreadStatusIcon';
import { ThreadTitleEditor } from './ThreadTitleEditor';
import { CopyThreadRefButton } from '../shared/CopyThreadRefButton';
import { ExportThreadButton } from '../shared/ExportThreadButton';
import { MobileThreadTitleBar } from '../layout/MobileAppHeader';
import { computeExchanges, hasContentEvents } from '../../store/thread-events';
import { awayFromBottom, notAtTop, scrollToBottom, scrolledUp, hasPendingEventScroll } from './scrollState';
import { useScrollMemory, hasSavedScroll, threadScrollKey } from '../../hooks/useScrollMemory';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { DelayedSpinner } from '../shared/DelayedSpinner';
import { forceIOSRepaint, createRepaintThrottle } from '../../utils/iosRepaint';
import { onPageResume } from '../../utils/pageResume';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { refreshClient } from '../../hooks/sw-update';

// Module-level tracking survives component unmount/remount (e.g. Thread A → CreateThread → Thread B).
// Using a ref would reset on remount, causing the fade-in to be skipped.
let lastRevealedThread: string | null = null;

/** Escalating retry delays for the empty-thread safety retry. */
const EMPTY_THREAD_RETRY_DELAYS = [500, 2000, 5000];

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

/** Minimum gap (ms) between forced iOS repaints while a thread streams. ~5/sec
 *  keeps the compositor layer from getting stuck blank without thrashing it on
 *  every token. */
const STREAM_REPAINT_THROTTLE_MS = 200;

/** Discriminated union — each variant is one render path. Impossible states
 *  (e.g. "loaded with events but animating") can't be constructed. */
export type EmptyReason =
    | { kind: 'loading'; threadId: string }
    | { kind: 'failed'; threadId: string }
    | { kind: 'corrupt'; threadId: string }
    | { kind: 'empty' };

/** Derive the empty reason from thread state. During animation, returns
 *  'loading' — rendered as the delayed spinner — which prevents the error
 *  state from flashing when events arrive via SSE before the animation gate
 *  lifts.
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
): EmptyReason {
    if (animating) return { kind: 'loading', threadId };
    if (eventsLoadFailed) return { kind: 'failed', threadId };
    if (eventsLoaded && hasContent) return { kind: 'corrupt', threadId };
    if (eventsLoaded) return { kind: 'empty' };
    return { kind: 'loading', threadId };
}

function ThreadEmptyState({ reason }: { reason: EmptyReason }) {
    // Both call sites key this component by threadId, so the spinner delay
    // (inside DelayedSpinner) and this reload timeout restart per thread.
    const showReload = useDelayedFlag(reason.kind === 'loading', RELOAD_TIMEOUT);

    switch (reason.kind) {
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
        case 'empty':
            return (
                <div class="thread-empty-state">
                    <p>No messages in this thread</p>
                </div>
            );
        case 'loading':
            return (
                <div class="thread-empty-state">
                    <DelayedSpinner />
                    {showReload && (
                        <button class="thread-empty-reload" onClick={() => refreshClient()}>
                            Taking too long? Tap to reload
                        </button>
                    )}
                </div>
            );
    }
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
    // every event arrival. See `~/.claude/plans/generic-sparking-garden.md`.
    const eventsBump = threadId ? getThreadEventsBump(threadId) : 0;
    const exchanges = useMemo(
        () => hasContent && !animating && eventThread
            ? computeExchanges(eventThread) : [],
        [threadId, eventCount, pendingCount, hasContent, animating, eventsBump],
    );
    const streamingBuffer = animating ? '' : activeStreamingBuffer.value;

    // Eligible for slide-in? revealOnFocus checked in the layout effect only
    // to avoid subscribing the render to the signal (prevents extra re-renders).
    const shouldReveal = shouldRevealThread(threadId, animating, hasContent);

    const areaRef = useRef<HTMLDivElement>(null);
    const isUp = awayFromBottom.value;
    const isNotAtTop = notAtTop.value;

    // Force-restart CSS animation imperatively — works even when the
    // .revealing class is already on the element from a prior thread switch.
    // Runs before paint (useLayoutEffect) so the user never sees a flash.
    // Animates .thread-view (title + content together) so the whole thread
    // slides in from the bottom while header and prompt stay put.
    // Applies to ALL .thread-view elements (desktop + mobile render both in the
    // DOM simultaneously) so the visible copy always gets the animation.
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

    // Force iOS Safari repaint of the scroll container — invalidates iOS's
    // cached compositor texture so DOM-present-but-black content shows.
    // Called on data changes AND on resume from background.
    const forceRepaint = () => forceIOSRepaint(areaRef.current);

    // Scroll to bottom when events finish loading for the focused thread.
    // focusThread() calls scrollToBottom() but its ResizeObserver suppression
    // expires after ~2 rAFs. If events load asynchronously (longer than that),
    // the ResizeObserver fires when content renders, sets scrolledUp=true,
    // and useAutoScroll skips the scroll. This re-triggers scrollToBottom()
    // once content is actually ready.
    //
    // Also force iOS repaint on every threadId change (not just eventsLoaded).
    // iOS Safari's compositor caches layer textures inside scroll-snap parents.
    // After many thread switches, it stops repainting already-loaded threads.
    // Triggering on threadId alone covers threads where eventsLoaded was already
    // true (no transition to trigger the effect).
    //
    // hasExchanges dep: events can arrive via SSE before loadThreadEvents
    // completes (eventsLoaded stays false). When exchanges first appear from
    // SSE-delivered events, the compositor layer may still hold a blank
    // texture. This dep ensures a repaint fires on the 0→N transition.
    const hasExchanges = exchanges.length > 0;
    const savedScrollKey = threadId ? threadScrollKey(threadId) : null;

    // Mark the user as scrolled-up synchronously when a saved scroll exists.
    // Several auto-scroll callers (visualViewport.resize in MobileSwipeContainer,
    // useHideOnScroll's focusin/focusout) gate on `wasAtBottom = !scrolledUp.value`
    // and call scrollToBottom() if true. On iOS PWA those fire repeatedly during
    // initial load and would override useScrollMemory's restore. Setting
    // scrolledUp early makes wasAtBottom=false so they skip.
    useLayoutEffect(() => {
        if (savedScrollKey && hasSavedScroll(savedScrollKey)) {
            scrolledUp.value = true;
        }
    }, [savedScrollKey]);

    useEffect(() => {
        if (threadId) {
            // Skip auto-scroll when a saved scroll position exists — its 500ms
            // loop would otherwise overwrite the restore set by useScrollMemory.
            // The saved key only holds a value when the user was scrolled up
            // (shouldSave: () => scrolledUp.value), so at-bottom defers here.
            // Skip too while a notification deep-link owns the scroll. Its claim
            // is held until scrollToEventAndPulse's deadline (not released the
            // instant it lands), so it also covers the re-fire this effect does
            // when `hasExchanges` flips 0→true a beat after the deep-link
            // resolves. Deliberately NOT gated on scrolledUp: this effect's own
            // purpose is to recover the at-bottom snap on a slow load where a
            // ResizeObserver fire escalated scrolledUp=true (see the comment
            // above) — a scrolledUp gate would defeat that recovery.
            if (eventsLoaded && !hasSavedScroll(savedScrollKey) && !hasPendingEventScroll()) scrollToBottom();
            return forceRepaint();
        }
    }, [threadId, eventsLoaded, hasExchanges]);

    // iOS PWA resume: force repaint when returning from background.
    // Signal values don't change on resume (same thread, same events), so no
    // re-render produces DOM changes. iOS Safari's compositor may have recycled
    // the layer texture while backgrounded — content is in the DOM but invisible
    // (renders black). Subscribing to the shared resume signal fires the repaint
    // on visibilitychange / pageshow / focus, not just visibilitychange: iOS
    // frequently restores a PWA via pageshow (bfcache) with no `visible`
    // visibilitychange, which left the old handler silent and the content black
    // until a tap — and that tap could land on an invisible question / permission
    // and answer it (see utils/pageResume, which also swallows that wake-tap).
    // forceRepaint is iOS-gated and null-safe.
    useEffect(() => onPageResume(forceRepaint), []);

    // Sustained-streaming repaint (iOS): entering a *running* thread, the rapid
    // streaming DOM mutations can make WKWebView blank the .thread-content
    // compositor layer AFTER the one-shot entry/load repaint above has already
    // fired — the content stays in the DOM (still scrollable, chevron shows) but
    // renders black until a manual scroll. eventsBump ticks on every append to
    // THIS thread (tokens, tool events, CC text), so repaint on a throttle as
    // content streams in. Fire-and-forget on purpose: this stream-driven repaint
    // has no lifecycle to clean up. The gate keeps it to one repaint per ~200ms,
    // and forceIOSRepaint supersedes an overlapping toggle (reusing the captured
    // baseline) so nothing accumulates; forceRepaint is iOS-gated and null-safe.
    const streamRepaintGate = useMemo(() => createRepaintThrottle(STREAM_REPAINT_THROTTLE_MS), []);
    useEffect(() => {
        if (!hasExchanges) return;
        if (!streamRepaintGate(performance.now())) return;
        forceRepaint();
    }, [eventsBump, hasExchanges]);

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
            rebuildCorruptedThreadEvents(threadId);
        }, 300);
        return () => clearTimeout(timer);
    }, [threadId, eventsLoaded, exchanges.length, eventCount]);

    useAutoScroll(areaRef, [eventCount, streamingBuffer, pendingCount], true);

    // Restoring sets scrolledUp so useAutoScroll defers to the saved offset
    // rather than snapping to bottom on the next render.
    useScrollMemory(
        areaRef,
        savedScrollKey,
        {
            paused: !eventsLoaded,
            shouldSave: () => scrolledUp.value,
            onRestored: () => { scrolledUp.value = true; },
            // A notification deep-link (toast / push / inbox) owns the scroll
            // when focusing an UNfocused thread: skip the saved-scroll restore
            // so it doesn't fire after scrollToEventAndPulse and snap away from
            // the source event. The claim is held until the deep-link's deadline,
            // so this is true for the whole resolve window. (Already-focused
            // threads don't re-run this hook, so they never needed the guard —
            // which is why the bug only bit unfocused-thread deep-links.)
            shouldRestore: () => !hasPendingEventScroll(),
        },
    );

    // Re-scroll after render: addPendingMessage's scrollToBottom() can race with
    // ResizeObserver re-setting scrolledUp before Preact commits the DOM update.
    const prevPendingRef = useRef(pendingCount);
    useEffect(() => {
        if (pendingCount > prevPendingRef.current) {
            scrollToBottom();
        }
        prevPendingRef.current = pendingCount;
    }, [pendingCount]);

    if (!threadId) return null;

    if (!eventThread) {
        // Don't unfocus if threads haven't loaded yet — the thread may appear
        // once loadAllThreads completes. Only unfocus after threads are loaded.
        if (threadsLoaded.value) {
            unfocusThread();
        }
        // Waiting for the thread to appear in the map — same delayed-spinner
        // empty state as the events-loading path, including the 8s "Taking too
        // long? Tap to reload" escape hatch. This covers the window between
        // page load (localStorage hydrates focusedThreadId) and loadAllThreads
        // completing (thread enters map). On iOS Safari PWA cold start this
        // can be several seconds.
        return (
            <div class="thread-view">
                <div class="thread-content-wrap">
                    <div class="thread-content visible">
                        <ThreadEmptyState key={threadId} reason={{ kind: 'loading', threadId }} />
                    </div>
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
                    <CopyThreadRefButton threadId={threadId} title={threadTitle} />
                    <ExportThreadButton threadId={threadId} title={threadTitle} />
                </span>
            </div>
            <div class="thread-content-wrap">
                <div class="thread-content visible" ref={areaRef}>
                    <MobileThreadTitleBar />

                    {exchanges.length === 0 ? (
                        <ThreadEmptyState key={threadId} reason={emptyReason(animating, eventsLoaded, eventsLoadFailed, hasContentEvents(eventThread.events), threadId!)} />
                    ) : (
                        renderExchanges(exchanges, threadId!, streamingBuffer)
                    )}
                </div>
                <ScrollControls
                    showUp={isNotAtTop}
                    showDown={isUp}
                    onScrollUp={() => areaRef.current?.scrollTo({ top: 0, behavior: 'smooth' })}
                    onScrollDown={scrollToBottom}
                />
            </div>
        </div>
    );
}
