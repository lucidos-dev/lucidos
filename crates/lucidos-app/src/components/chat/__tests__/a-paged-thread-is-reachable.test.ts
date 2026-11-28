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
    clampedScrollTop,
    scrollAsksForOlder,
    UPWARD_SCROLL_KEYS,
    scrollToTopNeedsRenderAll,
    type WindowEdge,
} from '../threadWindow';

const here: string = dirname(fileURLToPath(import.meta.url));
const threadViewSource: string = readFileSync(resolve(here, '../ThreadView.tsx'), 'utf-8');

function depsAfter(marker: string): string | undefined {
    const from = threadViewSource.indexOf(marker);
    if (from < 0) return undefined;
    return threadViewSource.slice(from).match(/\}, \[([^\]]*)\]\);/)?.[1];
}

const view = (scrolls: boolean) => ({ scrollHeight: scrolls ? 4000 : 100, clientHeight: 800 });

const clamped: WindowEdge = { exchange: 3, rowsHidden: 0 };

describe('a paged thread is always reachable', () => {
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

    it('counts a turn whose head is clamped off', () => {
        expect(anythingAbove(false, { exchange: 0, rowsHidden: 40 }, false)).toBe(true);
    });
});

describe('a scroll asking for older history', () => {
    const short = (scrollTop: number) => ({ scrollTop, scrollHeight: 1386, clientHeight: 449 });
    const bottom = clampedScrollTop(short(937));

    it('asks when the reader moves up inside the margin', () => {
        expect(scrollAsksForOlder(bottom, clampedScrollTop(short(900)))).toBe(true);
    });

    it('never asks when the reader moves down, the reported case', () => {
        expect(scrollAsksForOlder(clampedScrollTop(short(900)), bottom)).toBe(false);
    });

    it('never asks for a scroll that did not move', () => {
        expect(scrollAsksForOlder(bottom, bottom)).toBe(false);
    });

    it('never asks above the margin, whichever way the reader went', () => {
        const tall = (scrollTop: number) => ({ scrollTop, scrollHeight: 20000, clientHeight: 449 });
        expect(scrollAsksForOlder(clampedScrollTop(tall(9000)), clampedScrollTop(tall(8000)))).toBe(false);
    });

    it('reads an overscroll past the bottom as the bottom', () => {
        expect(clampedScrollTop(short(1000))).toBe(937);
        expect(scrollAsksForOlder(clampedScrollTop(short(1000)), bottom)).toBe(false);
    });

    it('reads an overscroll past the top as the top', () => {
        expect(clampedScrollTop(short(-40))).toBe(0);
    });

    it('re-takes the baseline before the navigation guard', () => {
        const handler = threadViewSource.slice(
            threadViewSource.indexOf('const onScroll = (scroll: ScrollClass) => {'),
            threadViewSource.indexOf('let lastTouchY'),
        );
        const baseline = handler.indexOf('lastTop = clampedScrollTop(el);');
        expect(baseline).toBeGreaterThan(-1);
        expect(baseline).toBeLessThan(handler.indexOf('if (scroll.navigation || scroll.onLastWrite) return;'));
    });
});

describe('the visit a claimed window belongs to', () => {
    it('ends whenever the thread pane lets the thread go', () => {
        expect(threadViewSource).toContain('if (lastSeededVisit?.threadId === threadId && lastSeededVisit.owner === visitOwner) {');
    });

    it('is marked only once the load has settled', () => {
        const seed = threadViewSource.slice(
            threadViewSource.indexOf('if (threadId && canSeedWindow) {'),
            threadViewSource.indexOf('const edge = threadId ? currentEdge('),
        );
        expect(seed.length).toBeGreaterThan(0);
        expect(seed).toContain('lastSeededVisit = { threadId, owner: visitOwner };');
        expect(seed.indexOf('canSeedWindow')).toBeLessThan(seed.indexOf('lastSeededVisit = { threadId, owner: visitOwner };'));
    });

    it('outlives the component that reads it', () => {
        expect(threadViewSource).toContain('let lastSeededVisit: { threadId: string; owner: object } | null = null;');
    });
});

describe('pressing the chevron on a paged thread', () => {
    it('loads the rest of the thread rather than gliding to the loaded top', () => {
        expect(scrollToTopNeedsRenderAll(WHOLE_THREAD, true)).toBe(true);
        expect(scrollToTopNeedsRenderAll(WHOLE_THREAD, false)).toBe(false);
    });

    it('leaves a thread rendered whole to glide, as before', () => {
        expect(scrollToTopNeedsRenderAll({ exchange: 2, rowsHidden: 0 }, false)).toBe(true);
        expect(scrollToTopNeedsRenderAll(WHOLE_THREAD)).toBe(false);
    });

    it('keys the jump on the window tick, not on the edge alone', () => {
        expect(depsAfter('if (!pendingScrollTopRef.current) return;')).toBe('edgeKey, winTick');
    });

    it('drops a jump the thread never consumed', () => {
        const from = threadViewSource.indexOf('if (threadId) historyHoldByThread.delete(threadId);');
        const teardown = threadViewSource.slice(from, threadViewSource.indexOf('}, [threadId]);', from));
        expect(teardown).toContain('pendingScrollTopRef.current = false;');
    });
});

describe('a page landing on a re-pointed window', () => {
    it('consumes the grow capture on the tick, not on the edge alone', () => {
        expect(depsAfter('const pend = pendingExpandRef.current;')).toBe('edgeKey, winTick');
    });

    it('asks the fill again after every window write', () => {
        const from = threadViewSource.indexOf('useLayoutEffect(() => { fillWindowRef.current(); }');
        expect(threadViewSource.slice(from, threadViewSource.indexOf(']);', from))).toContain('winTick');
    });

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

describe('the pane-resize observer after a cold open', () => {
    it('re-attaches once the thread enters the map', () => {
        const deps = depsAfter('const observer = new ResizeObserver(() => {\n            fillWindowRef.current();');
        expect(deps).toBe('threadId, threadInMap');
    });
});

describe('a reader pinned at the very top', () => {
    it('is recognised there, fractional offsets included', () => {
        expect(atScrollTop({ scrollTop: 0 })).toBe(true);
        expect(atScrollTop({ scrollTop: 0.5 })).toBe(true);
    });

    it('is not recognised anywhere a scroll event can still fire', () => {
        expect(atScrollTop({ scrollTop: 2 })).toBe(false);
        expect(atScrollTop({ scrollTop: 400 })).toBe(false);
    });

    it('is heard through the gestures themselves, not through scroll', () => {
        expect(threadViewSource).toContain("el.addEventListener('wheel', onWheel, { passive: true });");
        expect(threadViewSource).toContain("el.addEventListener('touchmove', onTouchMove, { passive: true });");
        expect(threadViewSource).toContain('if (!reachesUp || !atScrollTop(el)) return;');
    });

    it('asks only when the gesture reaches upward', () => {
        expect(threadViewSource).toContain('const onWheel = (e: WheelEvent) => reachIfUp(e.deltaY < 0);');
        expect(threadViewSource).toContain('reachIfUp(lastTouchY !== null && y !== null && y > lastTouchY);');
    });

    it('forgets the touch it was tracking when the finger lifts', () => {
        expect(threadViewSource).toContain('const onTouchEnd = () => { lastTouchY = null; };');
        expect(threadViewSource).toContain("el.addEventListener('touchstart', onTouchStart, { passive: true });");
    });

    it('asks for exactly what a scroll near the top asks for', () => {
        const effect = threadViewSource.slice(
            threadViewSource.indexOf('const reachForOlder = () => {'),
            threadViewSource.indexOf('}, [threadId, eventsLoaded]);'),
        );
        expect(effect.match(/requestBackfill\(/g)).toHaveLength(1);
        expect(effect.match(/growRenderWindow\(/g)).toHaveLength(1);
        expect(effect).toContain('if (!scrollAsksForOlder(previousTop, lastTop)) return;\n            reachForOlder();');
    });

    it('never hears the app landing the reader, however late its event', () => {
        expect(threadViewSource).toContain('if (scroll.navigation || scroll.onLastWrite) return;');
    });

    it('stops listening when the thread goes', () => {
        for (const pair of ['wheel\', onWheel', 'touchstart\', onTouchStart',
            'touchmove\', onTouchMove', 'touchend\', onTouchEnd',
            'keydown\', onKeyDown']) {
            expect(threadViewSource).toContain(`el.removeEventListener('${pair});`);
        }
    });

    it('hears an upward key, since the transcript takes keys directly', () => {
        for (const key of ['ArrowUp', 'PageUp', 'Home']) {
            expect(UPWARD_SCROLL_KEYS.has(key)).toBe(true);
        }
        for (const key of ['ArrowDown', 'PageDown', 'End', ' ']) {
            expect(UPWARD_SCROLL_KEYS.has(key)).toBe(false);
        }
        expect(threadViewSource).toContain('UPWARD_SCROLL_KEYS.has(e.key));');
    });

    it('ignores a key carrying a modifier', () => {
        expect(threadViewSource).toContain('if (e.metaKey || e.ctrlKey || e.altKey) return;');
    });

    it('reads Shift+Space as upward, and bare Space as not', () => {
        expect(UPWARD_SCROLL_KEYS.has(' ')).toBe(false);
        expect(threadViewSource).toContain('reachIfUp(space ? e.shiftKey : UPWARD_SCROLL_KEYS.has(e.key));');
    });

    it('stands down only for a key the focused control consumed', () => {
        expect(threadViewSource).toContain('if (e.defaultPrevented) return;');
        const handler = threadViewSource.slice(
            threadViewSource.indexOf('const onKeyDown = (e: KeyboardEvent) => {'),
            threadViewSource.indexOf('const stopScroll = onClassifiedScroll(el, onScroll);'),
        );
        expect(handler).not.toContain('e.target');
    });

    it('leaves a transcript that cannot scroll to the capped filler', () => {
        expect(threadViewSource).toContain('if (!transcriptScrolls(el)) return;');
    });
});
