import type { VNode } from 'preact';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { focusedThreadId, threadMap, activeStreamingBuffer, threadsLoaded, promptAnimating, effectiveThreadStatus, revealOnFocus } from '../../store/store';
import { unfocusThread } from '../../store/actions/threads';
import { loadThreadEvents, forceRetryThreadEvents } from '../../store/actions/thread-loading';
import { ChatExchange } from './ChatExchange';
import { ChevronUpIcon, ChevronDownIcon } from '../shared/icons';
import { useAutoScroll } from './CreateThreadView';
import { ThreadStatusIcon, resolveVisualStatus } from '../shared/ThreadStatusIcon';
import { ThreadTitleEditor } from './ThreadTitleEditor';
import { PinButton } from '../shared/PinButton';
import { MobileThreadTitleBar } from '../layout/MobileAppHeader';
import { exchangeStatus as getExchangeStatus, exchangeImageCount, exchangeResponseModel, exchangeReasoningEffort, computeExchanges } from '../../store/thread-events';
import { isActive as isStatusActive } from '../../store/exchange-status';
import { awayFromBottom, notAtTop, scrollToBottom } from './scrollState';
import { isIOS } from '../../utils/platform';

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

/** Mark a thread as revealed (call after animation starts). Exported for testing. */
export function commitReveal(threadId: string) {
    lastRevealedThread = threadId;
}

/** Reset reveal tracking (called on unmount so re-entering the same thread animates). Exported for testing. */
export function resetRevealTracking() {
    lastRevealedThread = null;
}

/** Timeout (ms) before showing "Tap to reload" in loading state. */
const RELOAD_TIMEOUT = 8000;

/** Discriminated union — each variant is one render path. Impossible states
 *  (e.g. "loaded with events but animating") can't be constructed. */
export type EmptyReason =
    | { kind: 'loading'; threadId: string }
    | { kind: 'failed'; threadId: string }
    | { kind: 'corrupt'; threadId: string }
    | { kind: 'empty' };

/** Derive the empty reason from thread state. During animation, returns
 *  'loading' — the spinner is the correct visual for a transition, and this
 *  prevents the error state from flashing when events arrive via SSE before
 *  the animation gate lifts. */
export function emptyReason(
    animating: boolean,
    eventsLoaded: boolean,
    eventsLoadFailed: boolean,
    eventCount: number,
    threadId: string,
): EmptyReason {
    if (animating) return { kind: 'loading', threadId };
    if (eventsLoadFailed) return { kind: 'failed', threadId };
    if (eventsLoaded && eventCount > 0) return { kind: 'corrupt', threadId };
    if (eventsLoaded) return { kind: 'empty' };
    return { kind: 'loading', threadId };
}

function ThreadEmptyState({ reason }: { reason: EmptyReason }) {
    const [showReload, setShowReload] = useState(false);
    const threadId = reason.kind !== 'empty' ? reason.threadId : '';

    useEffect(() => {
        setShowReload(false);
        if (reason.kind !== 'loading') return;
        const timer = setTimeout(() => setShowReload(true), RELOAD_TIMEOUT);
        return () => clearTimeout(timer);
    }, [threadId, reason.kind]);

    switch (reason.kind) {
        case 'failed':
        case 'corrupt': {
            const message = reason.kind === 'failed' ? 'Failed to load messages' : 'Messages could not be displayed';
            return (
                <div class="thread-empty-state thread-empty-error">
                    <p>{message}</p>
                    <button class="action-btn" onClick={() => forceRetryThreadEvents(reason.threadId)}>Retry</button>
                    <button class="thread-empty-reload" onClick={() => location.reload()}>Reload page</button>
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
                    <div class="loading-spinner" />
                    {showReload && (
                        <button class="thread-empty-reload" onClick={() => location.reload()}>
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
    const exchanges = useMemo(
        () => hasContent && !animating && eventThread
            ? computeExchanges(eventThread) : [],
        [threadId, eventCount, pendingCount, hasContent, animating],
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
        // Only animate on dismiss→next (Done button), not regular thread selection.
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
            loadThreadEvents(threadId);
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
                loadThreadEvents(threadId);
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

    // Force iOS Safari repaint — toggles the compositor layer's transform
    // to invalidate iOS's cached texture and make content visible.
    // Called on data changes AND on resume from background.
    const forceIOSRepaint = () => {
        const el = areaRef.current;
        if (!el?.isConnected || !isIOS()) return;
        let raf1: number | undefined;
        let raf2: number | undefined;
        raf1 = requestAnimationFrame(() => {
            if (!el.isConnected) return;
            el.style.transform = 'translateZ(0.1px)';
            raf2 = requestAnimationFrame(() => {
                if (!el.isConnected) return;
                el.style.transform = '';
            });
        });
        return () => {
            if (raf1) cancelAnimationFrame(raf1);
            if (raf2) cancelAnimationFrame(raf2);
        };
    };

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
    useEffect(() => {
        if (threadId) {
            if (eventsLoaded) scrollToBottom();
            return forceIOSRepaint();
        }
    }, [threadId, eventsLoaded, hasExchanges]);

    // iOS PWA resume: force repaint when returning from background.
    // Signal values don't change on resume (same thread, same events), so
    // no re-render produces DOM changes. iOS Safari's compositor may have
    // recycled the layer texture while backgrounded — content is in the DOM
    // but invisible. Scrolling fixes it because scroll events force a
    // compositor repaint. This listener forces the repaint proactively.
    useEffect(() => {
        if (!isIOS()) return;
        function onVisibilityChange() {
            if (document.visibilityState !== 'visible') return;
            forceIOSRepaint();
        }
        document.addEventListener('visibilitychange', onVisibilityChange);
        return () => document.removeEventListener('visibilitychange', onVisibilityChange);
    }, []);

    // Self-healing watchdog: if a thread has events but exchanges are empty,
    // rebuild the events Map and re-fetch from the API. iOS Safari can
    // corrupt long-lived Map internals under memory pressure, causing
    // has()/get() to return wrong results. Capped at 2 attempts.
    const watchdogRef = useRef<ThreadRetryRef>(null);
    useEffect(() => {
        if (!threadId || !eventsLoaded || exchanges.length > 0) return;
        if (eventCount === 0) return; // Legitimately empty
        if (hasExhaustedRetries(watchdogRef, threadId, 2)) return;
        const timer = setTimeout(() => {
            incrementRetry(watchdogRef, threadId);
            const thread = threadMap.value.get(threadId);
            if (thread) {
                // Rebuild Map internals (corrupted has()/get()), then full re-fetch.
                thread.events = new Map(thread.events);
                thread.eventsLoaded = false;
                thread.lastDbSeq = 0;
                loadThreadEvents(threadId);
            }
        }, 300);
        return () => clearTimeout(timer);
    }, [threadId, eventsLoaded, exchanges.length, eventCount]);

    useAutoScroll(areaRef, [eventCount, streamingBuffer, pendingCount], true);

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
        // Show spinner while waiting for thread to appear in map — never blank.
        // This covers the window between page load (localStorage hydrates
        // focusedThreadId) and loadAllThreads completing (thread enters map).
        // On iOS Safari PWA cold start this can be several seconds.
        return (
            <div class="thread-view">
                <div class="thread-content-wrap">
                    <div class="thread-content visible">
                        <div class="loading-spinner" />
                    </div>
                </div>
            </div>
        );
    }

    const threadTitle = eventThread.meta.title || '';
    const visualStatus = resolveVisualStatus(
      effectiveThreadStatus(eventThread),
      eventThread.meta.activeChildrenCount > 0,
      eventThread.meta.ccHasChanges,
    );
    return (
        <div class="thread-view">
            <div class="thread-view-header">
                <PinButton threadId={threadId} pinned={eventThread.meta.pinned} />
                <ThreadTitleEditor threadId={threadId} title={threadTitle} />
                <ThreadStatusIcon status={visualStatus} />
            </div>
            <div class="thread-content-wrap">
                <div class="thread-content visible" ref={areaRef}>
                    <MobileThreadTitleBar />

                    {exchanges.length === 0 ? (
                        <ThreadEmptyState reason={emptyReason(animating, eventsLoaded, eventsLoadFailed, eventCount, threadId!)} />
                    ) : (
                        exchanges.reduce<{ nodes: VNode[]; imgOffset: number; lastModel?: string; lastEffort?: string }>((acc, ex, i) => {
                            const isLast = i === exchanges.length - 1;
                            const priorActive = i > 0 && isStatusActive(getExchangeStatus(exchanges[i - 1], '', true));
                            acc.nodes.push(
                                <ChatExchange
                                    key={'ex-' + ex.userSeq}
                                    exchange={ex}
                                    streamingBuffer={isLast ? streamingBuffer : ''}
                                    isLast={isLast}
                                    threadId={threadId!}
                                    hasPriorActive={priorActive}
                                    imageOffset={acc.imgOffset}
                                    priorModel={acc.lastModel}
                                    priorEffort={acc.lastEffort}
                                />
                            );
                            acc.imgOffset += exchangeImageCount(ex);
                            acc.lastModel = exchangeResponseModel(ex) ?? acc.lastModel;
                            acc.lastEffort = exchangeReasoningEffort(ex) ?? acc.lastEffort;
                            return acc;
                        }, { nodes: [], imgOffset: 0 }).nodes
                    )}
                </div>
                <button
                    class={`scroll-to-top${isNotAtTop ? ' visible' : ''}`}
                    onClick={() => {
                        const el = areaRef.current;
                        if (el) el.scrollTo({ top: 0, behavior: 'smooth' });
                    }}
                >
                    <ChevronUpIcon />
                </button>
                <button
                    class={`scroll-to-bottom${isUp ? ' visible' : ''}`}
                    onClick={() => {
                        const el = areaRef.current;
                        if (el) el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
                    }}
                >
                    <ChevronDownIcon />
                </button>
            </div>
        </div>
    );
}
