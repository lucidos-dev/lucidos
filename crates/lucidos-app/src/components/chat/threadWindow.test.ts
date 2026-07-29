import { describe, it, expect } from 'vitest';
import {
    INITIAL_WINDOW,
    WINDOW_STEP,
    computeRenderFromIndex,
    hasMoreAbove,
    expandRenderCount,
    scrollToTopNeedsRenderAll,
} from './threadWindow';

describe('thread render windowing', () => {
    describe('computeRenderFromIndex', () => {
        it('renders the whole list when it fits in the window', () => {
            expect(computeRenderFromIndex(5, INITIAL_WINDOW)).toBe(0);
            expect(computeRenderFromIndex(INITIAL_WINDOW, INITIAL_WINDOW)).toBe(0);
        });

        it('renders only the trailing window for a large list', () => {
            expect(computeRenderFromIndex(100, 20)).toBe(80);
            expect(computeRenderFromIndex(100, 40)).toBe(60);
        });

        it('treats Infinity (deep-link render-all) as render everything', () => {
            expect(computeRenderFromIndex(1000, Infinity)).toBe(0);
        });

        it('never returns a negative start', () => {
            expect(computeRenderFromIndex(0, 20)).toBe(0);
            expect(computeRenderFromIndex(3, 20)).toBe(0);
        });
    });

    describe('hasMoreAbove', () => {
        it('is true only when older exchanges sit above the window', () => {
            expect(hasMoreAbove(100, 20)).toBe(true);
            expect(hasMoreAbove(20, 20)).toBe(false);
            expect(hasMoreAbove(5, 20)).toBe(false);
        });
        it('is false once everything is rendered', () => {
            expect(hasMoreAbove(1000, Infinity)).toBe(false);
        });
    });

    describe('expandRenderCount', () => {
        it('grows by WINDOW_STEP', () => {
            expect(expandRenderCount(1000, 20)).toBe(20 + WINDOW_STEP);
        });
        it('caps at the total so it stabilizes once all is shown', () => {
            expect(expandRenderCount(30, 20)).toBe(30);
            expect(expandRenderCount(30, 30)).toBe(30);
        });
    });

    describe('scrollToTopNeedsRenderAll', () => {
        it('requires render-all while older exchanges sit above the window', () => {
            // A long thread opens windowed to the tail — scroll-to-top must render
            // the whole thread first, or it stalls partway (the "needed N clicks" bug).
            expect(scrollToTopNeedsRenderAll(100, INITIAL_WINDOW)).toBe(true);
            // One scroll-up expansion isn't enough on a long thread — still windowed.
            expect(scrollToTopNeedsRenderAll(100, INITIAL_WINDOW + WINDOW_STEP)).toBe(true);
        });

        it('skips render-all once the full thread is already rendered', () => {
            // Short thread fits in the initial window — scroll straight to top.
            expect(scrollToTopNeedsRenderAll(INITIAL_WINDOW, INITIAL_WINDOW)).toBe(false);
            expect(scrollToTopNeedsRenderAll(5, INITIAL_WINDOW)).toBe(false);
            // Already expanded to everything (deep-link render-all / a prior top jump).
            expect(scrollToTopNeedsRenderAll(1000, Infinity)).toBe(false);
        });
    });

    it('windowing then expanding converges to the full list', () => {
        const total = 95;
        let count = INITIAL_WINDOW;
        let from = computeRenderFromIndex(total, count);
        expect(from).toBe(75);
        // Scroll up repeatedly until nothing remains above.
        let guard = 0;
        while (hasMoreAbove(total, count) && guard++ < 100) {
            count = expandRenderCount(total, count);
        }
        from = computeRenderFromIndex(total, count);
        expect(from).toBe(0);
        expect(hasMoreAbove(total, count)).toBe(false);
    });
});
