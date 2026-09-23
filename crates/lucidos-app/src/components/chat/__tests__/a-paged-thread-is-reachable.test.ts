/** A thread the reader cannot get back through is the bug this pins.
 *
 *  One reported thread opened on a transcript too short to scroll, with every
 *  loaded turn already drawn and 662 events still on the server. The fill saw
 *  nothing left to grow. The chevron asked the same question the same way and
 *  hid itself. No gesture and no button reached the rest of the thread.
 *
 *  So the promise is checked over every state, not over the reported one. Where
 *  older content exists, the reader can scroll to it or press for it. Failing
 *  both, the fill is on its way to fetching it.
 *
 *  Chain and measurements:
 *  docs/plans/2026-09-20-a-paged-transcript-the-reader-can-reach.md. */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import {
    WHOLE_THREAD,
    anythingAbove,
    atScrollTop,
    fillAction,
    UPWARD_SCROLL_KEYS,
    scrollToTopNeedsRenderAll,
    type WindowEdge,
} from '../threadWindow';

const here: string = dirname(fileURLToPath(import.meta.url));
const threadViewSource: string = readFileSync(resolve(here, '../ThreadView.tsx'), 'utf-8');

/** The dependency array of the FIRST effect whose body starts with `marker`.
 *
 *  A scan over the whole file cannot name one effect. Two of them read the
 *  same pair, so a match anywhere would pass on the other's line while the one
 *  under test regressed. */
function depsAfter(marker: string): string | undefined {
    const from = threadViewSource.indexOf(marker);
    if (from < 0) return undefined;
    return threadViewSource.slice(from).match(/\}, \[([^\]]*)\]\);/)?.[1];
}

/** A transcript that does or does not overflow its pane. */
const view = (scrolls: boolean) => ({ scrollHeight: scrolls ? 4000 : 100, clientHeight: 800 });

/** An edge with turns still windowed out, against the edge with none. */
const clamped: WindowEdge = { exchange: 3, rowsHidden: 0 };

describe('a paged thread is always reachable', () => {
    /** The whole state space: overflow, window edge, server watermark. */
    it('never leaves older content behind a transcript with no way up', () => {
        for (const scrolls of [true, false]) {
            for (const edge of [WHOLE_THREAD, clamped]) {
                for (const hasOlderEvents of [true, false]) {
                    const somethingAbove = edge !== WHOLE_THREAD || hasOlderEvents;
                    if (!somethingAbove) continue;

                    const canPress = anythingAbove(false, edge, hasOlderEvents);
                    const willFetch = fillAction(view(scrolls), edge, hasOlderEvents) !== 'none';

                    expect(scrolls || canPress || willFetch).toBe(true);
                }
            }
        }
    });

    /** The exact reported state, named so a regression reads as itself. */
    it('offers the chevron on the reported thread, which could not scroll', () => {
        expect(anythingAbove(false, WHOLE_THREAD, true)).toBe(true);
        expect(fillAction(view(false), WHOLE_THREAD, true)).toBe('page');
    });

    it('offers nothing on a thread rendered whole to its first event', () => {
        expect(anythingAbove(false, WHOLE_THREAD, false)).toBe(false);
        expect(fillAction(view(false), WHOLE_THREAD, false)).toBe('none');
    });

    it('still offers the chevron to a reader who has scrolled down', () => {
        expect(anythingAbove(true, WHOLE_THREAD, false)).toBe(true);
    });

    it('offers it while turns sit above the window, unscrolled', () => {
        expect(anythingAbove(false, clamped, false)).toBe(true);
    });

    /** A clamped floor turn counts as "above" on its rows alone. Its head is
     *  reachable with no older turn behind it. */
    it('counts a turn whose head is clamped off', () => {
        expect(anythingAbove(false, { exchange: 0, rowsHidden: 40 }, false)).toBe(true);
    });
});

/** A render-all lasts ONE visit (ADR 0232), and the whole rule turns on where
 *  a visit ends. `reseedOnReopen` has its own unit tests; what they cannot see
 *  is the component's bookkeeping around it, which is where both holes were.
 *
 *  Source scans, because the mark is module state read by a layout effect. */
describe('the visit a render-all belongs to', () => {
    it('ends whenever the thread pane lets the thread go', () => {
        // A switch, New chat unmounting the pane, and the layout swapping at
        // the breakpoint are one teardown, so one clear covers all three.
        expect(threadViewSource).toContain('if (lastSeededVisit === threadId) lastSeededVisit = null;');
    });

    /** Marking before the events settle spends the visit on a commit that
     *  evaluated nothing. The settled commit then reads it as a later one, so
     *  the guard has to come first. */
    it('is marked only once the load has settled', () => {
        const seed = threadViewSource.slice(
            threadViewSource.indexOf('if (!threadId || !canSeedWindow) return;'),
            threadViewSource.indexOf('}, [threadId, canSeedWindow]);'),
        );
        expect(seed).toContain('lastSeededVisit = threadId;');
        expect(seed.indexOf('canSeedWindow')).toBeLessThan(seed.indexOf('lastSeededVisit = threadId;'));
    });

    /** Module-scoped, not a ref: a ref dies with the mount, and the layout
     *  swap remounts this component while the reader is mid-read. */
    it('outlives the component that reads it', () => {
        expect(threadViewSource).toContain('let lastSeededVisit: string | null = null;');
    });
});

/** Offering the chevron is half the promise. Pressing it has to move somebody.
 *
 *  The press fetches the history and jumps to the true top, and the jump is a
 *  layout effect the handler arms a ref for. On the reported thread the window
 *  already held every loaded turn. So the press writes the edge it was already
 *  on, and the edge's value never changes. */
describe('pressing the chevron on a paged thread', () => {
    it('loads the rest of the thread rather than gliding to the loaded top', () => {
        expect(scrollToTopNeedsRenderAll(WHOLE_THREAD, true)).toBe(true);
        expect(scrollToTopNeedsRenderAll(WHOLE_THREAD, false)).toBe(false);
    });

    it('leaves a thread rendered whole to glide, as before', () => {
        expect(scrollToTopNeedsRenderAll({ exchange: 2, rowsHidden: 0 }, false)).toBe(true);
        expect(scrollToTopNeedsRenderAll(WHOLE_THREAD)).toBe(false);
    });

    /** A source scan, because the effect cannot be reached without rendering
     *  the component. Keyed on the edge alone, the jump never runs for the
     *  press above, and the arming survives to yank the NEXT thread instead.
     *
     *  Sliced to the jump's OWN dependency array rather than matched anywhere
     *  in the file. A second effect now reads the same pair, so a bare
     *  `toContain` would pass on that one's line while this effect regressed. */
    it('keys the jump on the window tick, not on the edge alone', () => {
        expect(depsAfter('if (!pendingScrollTopRef.current) return;')).toBe('edgeKey, winTick');
    });

    /** The same arming, cleared when the reader leaves. */
    it('drops a jump the thread never consumed', () => {
        const from = threadViewSource.indexOf('if (threadId) historyHoldByThread.delete(threadId);');
        const teardown = threadViewSource.slice(from, threadViewSource.indexOf('}, [threadId]);', from));
        expect(teardown).toContain('pendingScrollTopRef.current = false;');
    });
});

/** A page that lands must move nobody, and must leave the window growable.
 *
 *  The re-point runs on the commit that folded the page in, and it can arm the
 *  grow capture while the edge STRING is unchanged: a backfill grows the array
 *  at the front, so index 0 stays index 0 over different turns. Two source
 *  scans, because neither effect can be reached without rendering. */
describe('a page landing on a re-pointed window', () => {
    /** Left on the edge alone the capture outlives its commit. Every grower
     *  early-returns on it, so the fill, the scroll-up expansion and the anchor
     *  walk all stand down for the rest of the mount. */
    it('consumes the grow capture on the tick, not on the edge alone', () => {
        expect(depsAfter('const pend = pendingExpandRef.current;')).toBe('edgeKey, winTick');
    });

    /** And the fill has to be asked again once the capture is gone. It runs
     *  in the same commit the re-point arms in, sees the armed ref and stands
     *  down, so without the tick nothing re-drives it. */
    it('asks the fill again after every window write', () => {
        const from = threadViewSource.indexOf('useLayoutEffect(() => { fillWindowRef.current(); }');
        expect(threadViewSource.slice(from, threadViewSource.indexOf(']);', from))).toContain('winTick');
    });

    /** Held against a reading taken HERE, a reader restored onto their turn
     *  keeps an offset the fold already moved. The request's own capture is the
     *  last reading of the frame they were in, and it is held only while the
     *  pane can be measured. */
    it('holds the reader against the capture the request took', () => {
        const from = threadViewSource.indexOf('const hold = isElementVisible(el) ? pend : null;');
        expect(from, 'the hold must come from the request, and only while visible')
            .toBeGreaterThan(-1);
        const end = threadViewSource.indexOf('}, [threadId, historyFolded]);', from);
        expect(end, 'the re-point effect must close on its own deps')
            .toBeGreaterThan(from);
        const repoint = threadViewSource.slice(from, end);
        expect(repoint).toContain('prevScrollTop: hold.prevScrollTop');
        expect(repoint).toContain('prevScrollHeight: hold.prevScrollHeight');
    });
});

/** The scroll handler is the only caller of the backfill. A container already
 *  at the top fires no scroll event, however hard the reader gestures. So the
 *  second page was unreachable: one landed, and the walk froze.
 *
 *  Measured on a six-page thread, wheeling up: scroll height reached 5,739 at
 *  step 10 and had not moved by step 140. Phase 5 of
 *  docs/plans/2026-09-20-scrolling-up-a-long-thread-never-waits.md. */
describe('a reader pinned at the very top', () => {
    it('is recognised there, fractional offsets included', () => {
        expect(atScrollTop({ scrollTop: 0 })).toBe(true);
        expect(atScrollTop({ scrollTop: 0.5 })).toBe(true);
    });

    /** The slack has to stay under anything that still scrolls, or a gesture
     *  would fetch a page while the scroll handler was also asking. */
    it('is not recognised anywhere a scroll event can still fire', () => {
        expect(atScrollTop({ scrollTop: 2 })).toBe(false);
        expect(atScrollTop({ scrollTop: 400 })).toBe(false);
    });

    /** Source scans: the listeners live inside an effect that needs the
     *  component mounted, and the whole fix IS which events are heard. */
    it('is heard through the gestures themselves, not through scroll', () => {
        expect(threadViewSource).toContain("el.addEventListener('wheel', onWheel, { passive: true });");
        expect(threadViewSource).toContain("el.addEventListener('touchmove', onTouchMove, { passive: true });");
        expect(threadViewSource).toContain('if (!reachesUp || !atScrollTop(el)) return;');
    });

    /** A wheel event fires BEFORE the browser moves the container, so a reader
     *  leaving the top downward still measures as pinned there. Acting on it
     *  renders turns they are scrolling away from, then jumps them. */
    it('asks only when the gesture reaches upward', () => {
        expect(threadViewSource).toContain('const onWheel = (e: WheelEvent) => reachIfUp(e.deltaY < 0);');
        expect(threadViewSource).toContain('reachIfUp(lastTouchY !== null && y !== null && y > lastTouchY);');
    });

    /** Touch carries no delta, so direction is the travel between two moves.
     *  A stale start point would read the next gesture's first move wrongly. */
    it('forgets the touch it was tracking when the finger lifts', () => {
        expect(threadViewSource).toContain('const onTouchEnd = () => { lastTouchY = null; };');
        expect(threadViewSource).toContain("el.addEventListener('touchstart', onTouchStart, { passive: true });");
    });

    /** Both handlers must reach the same decision. A second copy would drift,
     *  and the drift would only show on a thread several pages long. */
    it('asks for exactly what a scroll near the top asks for', () => {
        const effect = threadViewSource.slice(
            threadViewSource.indexOf('const reachForOlder = () => {'),
            threadViewSource.indexOf('}, [threadId, eventsLoaded]);'),
        );
        expect(effect.match(/requestBackfill\(/g)).toHaveLength(1);
        expect(effect.match(/growRenderWindow\(/g)).toHaveLength(1);
        expect(effect).toContain('if (el.scrollTop > WINDOW_EXPAND_MARGIN_PX) return;\n            reachForOlder();');
    });

    /** Leaving them attached would keep a dead thread's listeners alive on a
     *  container the next thread reuses. */
    it('stops listening when the thread goes', () => {
        for (const pair of ['wheel\', onWheel', 'touchstart\', onTouchStart',
            'touchmove\', onTouchMove', 'touchend\', onTouchEnd',
            'keydown\', onKeyDown']) {
            expect(threadViewSource).toContain(`el.removeEventListener('${pair});`);
        }
    });

    /** The transcript is `tabindex=0`, so a keyboard reader hits the same wall
     *  a wheel reader did: at the top these keys scroll nothing and fire no
     *  event. Downward keys are excluded for the same reason wheel is. */
    it('hears an upward key, since the transcript takes keys directly', () => {
        for (const key of ['ArrowUp', 'PageUp', 'Home']) {
            expect(UPWARD_SCROLL_KEYS.has(key)).toBe(true);
        }
        for (const key of ['ArrowDown', 'PageDown', 'End', ' ']) {
            expect(UPWARD_SCROLL_KEYS.has(key)).toBe(false);
        }
        expect(threadViewSource).toContain('UPWARD_SCROLL_KEYS.has(e.key));');
    });

    /** A modifier makes it a shortcut, not a scroll. */
    it('ignores a key carrying a modifier', () => {
        expect(threadViewSource).toContain('if (e.metaKey || e.ctrlKey || e.altKey) return;');
    });

    /** Space is the one key a modifier REVERSES rather than reserves: it pages
     *  down bare and up with Shift, so the set cannot answer for it. */
    it('reads Shift+Space as upward, and bare Space as not', () => {
        expect(UPWARD_SCROLL_KEYS.has(' ')).toBe(false);
        expect(threadViewSource).toContain('reachIfUp(space ? e.shiftKey : UPWARD_SCROLL_KEYS.has(e.key));');
    });

    /** `keydown` bubbles, and a key on a DESCENDANT is the ordinary case: a
     *  choice card parks focus on its button, and the browser scrolls the
     *  transcript for whatever that button does not take.
     *
     *  So the question is whether the key was CONSUMED, not where it landed.
     *  A target check would refuse the reader exactly where focus usually is. */
    it('stands down only for a key the focused control consumed', () => {
        expect(threadViewSource).toContain('if (e.defaultPrevented) return;');
        const handler = threadViewSource.slice(
            threadViewSource.indexOf('const onKeyDown = (e: KeyboardEvent) => {'),
            threadViewSource.indexOf("el.addEventListener('scroll', onScroll"),
        );
        expect(handler).not.toContain('e.target');
    });

    /** A transcript that never grows a scrollbar sits at the top for good, so
     *  every wheel event of a flick would grow it again. `fillWindow` owns
     *  that shape, and its round caps are what bound it. */
    it('leaves a transcript that cannot scroll to the capped filler', () => {
        expect(threadViewSource).toContain('if (!transcriptScrolls(el)) return;');
    });
});
