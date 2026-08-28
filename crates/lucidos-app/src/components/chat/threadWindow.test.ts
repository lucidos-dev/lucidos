import { describe, it, expect } from 'vitest';
import {
    INITIAL_WINDOW,
    MAX_FILL_EXPANSIONS,
    SCROLLABLE_SLACK_PX,
    STEP_BUDGET,
    WINDOW_STEP,
    canSeedRenderWindow,
    computeRenderFromIndex,
    countWithinBudget,
    exchangeRenderCost,
    hasMoreAbove,
    expandRenderCount,
    renderCountFromFloor,
    seedRenderCount,
    scrollToTopNeedsRenderAll,
    windowMustReachIndex,
    windowNeedsFill,
    windowReachesIndex,
} from './threadWindow';

/** `n` exchanges each costing `cost` steps. */
const flat = (n: number, cost: number): number[] => Array.from({ length: n }, () => cost);

/** The reported thread's real turn sizes, one entry per exchange, oldest first.
 *  653 events over 19 exchanges, all of which rendered on open under the old
 *  turn-counting window. See
 *  docs/plans/2026-08-26-the-transcript-window-counts-steps-not-turns.md. */
const REPORTED_THREAD = [23, 80, 10, 52, 47, 7, 12, 53, 11, 60, 32, 32, 53, 23, 11, 0, 36, 17, 34]
    .map(steps => steps + 1);

/** Two more reported threads' real turn sizes, oldest first, read out of the
 *  workspace event log. Both are coding-agent threads ending on a small
 *  `ChangeApplied` boundary behind a huge working turn, which is the shape
 *  `windowNeedsFill` exists for. */
const ARCHIVED_THREAD = [357, 1, 115, 0, 0, 2, 0, 288, 7].map(steps => steps + 1);
const SKELETON_THREAD = [177, 1, 323, 1].map(steps => steps + 1);

/** A viewport the rendered slice is measured against. */
const view = (scrollHeight: number, clientHeight = 800) => ({ scrollHeight, clientHeight });

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

    describe('exchangeRenderCost', () => {
        it('counts the steps plus the exchange\'s own user bubble', () => {
            expect(exchangeRenderCost({ steps: [] })).toBe(1);
            expect(exchangeRenderCost({ steps: [1, 2, 3] })).toBe(4);
        });
    });

    describe('seedRenderCount', () => {
        it('holds an ordinary chat at the exchange cap, unchanged', () => {
            // 4 steps a turn: 40 turns would cost 160, so the budget never binds
            // and the cap decides. Exactly the window this thread had before.
            expect(seedRenderCount(flat(100, 4))).toBe(INITIAL_WINDOW);
        });

        it('renders a short thread whole', () => {
            expect(seedRenderCount(flat(5, 4))).toBe(5);
            expect(seedRenderCount([])).toBe(0);
        });

        it('cuts the reported thread from every turn to a budgeted tail', () => {
            const seeded = seedRenderCount(REPORTED_THREAD);
            expect(seeded).toBeLessThan(REPORTED_THREAD.length);
            const steps = REPORTED_THREAD.slice(REPORTED_THREAD.length - seeded)
                .reduce((a, c) => a + c, 0);
            expect(steps).toBeLessThanOrEqual(STEP_BUDGET);
            // Everything above the seeded tail is still reachable by scrolling up.
            expect(hasMoreAbove(REPORTED_THREAD.length, seeded)).toBe(true);
        });

        it('renders one exchange even when it alone blows the budget', () => {
            // Without the floor the transcript would be empty, which emptyReason
            // reads as `corrupt`.
            expect(seedRenderCount([5, STEP_BUDGET * 3])).toBe(1);
        });

        it('never exceeds the exchange cap, however cheap the turns', () => {
            expect(seedRenderCount(flat(1000, 1))).toBe(INITIAL_WINDOW);
        });
    });

    describe('canSeedRenderWindow', () => {
        const state = (o: Partial<Parameters<typeof canSeedRenderWindow>[0]>) => ({
            hasExchanges: true, eventsLoaded: false, eventsLoadFailed: false, ...o,
        });

        it('waits for the load, so a fragment cannot pin the window', () => {
            // A pending message or an early SSE event puts one exchange on screen
            // before the snapshot lands. The write happens once, so seeding here
            // would leave the full history in a one-exchange window.
            expect(canSeedRenderWindow(state({}))).toBe(false);
        });

        it('seeds once the snapshot has landed', () => {
            expect(canSeedRenderWindow(state({ eventsLoaded: true }))).toBe(true);
        });

        it('seeds on a failed load, which settles it just as firmly', () => {
            expect(canSeedRenderWindow(state({ eventsLoadFailed: true }))).toBe(true);
        });

        it('never seeds an empty transcript', () => {
            expect(canSeedRenderWindow(state({ hasExchanges: false, eventsLoaded: true }))).toBe(false);
        });
    });

    describe('countWithinBudget', () => {
        it('walks backwards from the given end', () => {
            expect(countWithinBudget([10, 10, 10, 10], 4, 25, 99)).toBe(2);
            expect(countWithinBudget([10, 10, 10, 10], 2, 25, 99)).toBe(2);
            expect(countWithinBudget([10, 10, 10, 10], 0, 25, 99)).toBe(0);
        });
    });

    describe('expandRenderCount', () => {
        it('grows by one budget of older exchanges', () => {
            // 40 steps a turn: four fit in a 160 budget.
            expect(expandRenderCount(flat(100, 40), 20)).toBe(24);
        });
        it('grows an ordinary chat by WINDOW_STEP, unchanged', () => {
            // 4 steps a turn: the budget clears 32, so the cap decides.
            expect(expandRenderCount(flat(1000, 4), 20)).toBe(20 + WINDOW_STEP);
        });
        it('caps at the total so it stabilizes once all is shown', () => {
            expect(expandRenderCount(flat(30, 4), 20)).toBe(30);
            expect(expandRenderCount(flat(30, 4), 30)).toBe(30);
        });
        it('takes one oversized exchange rather than stalling', () => {
            expect(expandRenderCount([STEP_BUDGET * 3, 4, 4], 2)).toBe(3);
        });
        it('treats a render-all count as nothing left to grow', () => {
            expect(expandRenderCount(flat(30, 4), Infinity)).toBe(30);
        });
    });

    describe('windowNeedsFill', () => {
        it('asks nothing once the whole thread is rendered', () => {
            expect(windowNeedsFill(view(120), 5, INITIAL_WINDOW)).toBe(false);
            expect(windowNeedsFill(view(120), 1000, Infinity)).toBe(false);
        });

        it('is true while a windowed slice is shorter than the pane', () => {
            expect(windowNeedsFill(view(120), 9, 1)).toBe(true);
            expect(windowNeedsFill(view(800 + SCROLLABLE_SLACK_PX), 9, 1)).toBe(true);
        });

        it('is false once the slice overflows by more than the slack', () => {
            expect(windowNeedsFill(view(800 + SCROLLABLE_SLACK_PX + 1), 9, 1)).toBe(false);
        });

        it('seeds both reported threads to a single trailing card', () => {
            expect(seedRenderCount(ARCHIVED_THREAD)).toBe(1);
            expect(seedRenderCount(SKELETON_THREAD)).toBe(1);
        });

        it('fills that card, so scrolling can take the window over', () => {
            for (const costs of [ARCHIVED_THREAD, SKELETON_THREAD]) {
                const seeded = seedRenderCount(costs);
                expect(windowNeedsFill(view(200), costs.length, seeded)).toBe(true);
                const filled = expandRenderCount(costs, seeded);
                // One round takes the working turn behind the boundary, which
                // is the biggest turn in either thread.
                expect(filled).toBe(2);
                expect(windowNeedsFill(view(4000), costs.length, filled)).toBe(false);
            }
        });

        it('stops at the cap when nothing the window adds ever draws', () => {
            // Degenerate: every turn blows the budget alone and draws no
            // height. The loop has to stop rather than render the thread out.
            const costs = flat(200, STEP_BUDGET * 2);
            let count = seedRenderCount(costs);
            let rounds = 0;
            while (windowNeedsFill(view(0), costs.length, count) && rounds < MAX_FILL_EXPANSIONS) {
                count = expandRenderCount(costs, count);
                rounds++;
            }
            expect(rounds).toBe(MAX_FILL_EXPANSIONS);
            expect(count).toBe(1 + MAX_FILL_EXPANSIONS);
        });
    });

    describe('scrollToTopNeedsRenderAll', () => {
        it('requires render-all while older exchanges sit above the window', () => {
            // A long thread opens windowed to the tail — scroll-to-top must render
            // the whole thread first, or it stalls partway (the "needed N clicks" bug).
            expect(scrollToTopNeedsRenderAll(100, INITIAL_WINDOW)).toBe(true);
            // One scroll-up expansion isn't enough on a long thread, still windowed.
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
        const costs = flat(95, 4);
        const total = costs.length;
        let count = seedRenderCount(costs);
        let from = computeRenderFromIndex(total, count);
        expect(from).toBe(75);
        // Scroll up repeatedly until nothing remains above.
        let guard = 0;
        while (hasMoreAbove(total, count) && guard++ < 100) {
            count = expandRenderCount(costs, count);
        }
        from = computeRenderFromIndex(total, count);
        expect(from).toBe(0);
        expect(hasMoreAbove(total, count)).toBe(false);
    });

    it('converges on the reported thread too, one budget at a time', () => {
        const total = REPORTED_THREAD.length;
        let count = seedRenderCount(REPORTED_THREAD);
        let guard = 0;
        while (hasMoreAbove(total, count) && guard++ < 100) {
            const next = expandRenderCount(REPORTED_THREAD, count);
            expect(next).toBeGreaterThan(count);
            count = next;
        }
        expect(computeRenderFromIndex(total, count)).toBe(0);
    });

    describe('renderCountFromFloor', () => {
        it('inverts computeRenderFromIndex', () => {
            expect(renderCountFromFloor(100, computeRenderFromIndex(100, 20))).toBe(20);
            expect(renderCountFromFloor(19, computeRenderFromIndex(19, INITIAL_WINDOW))).toBe(19);
        });

        it('reads a floor of zero as the whole thread', () => {
            expect(renderCountFromFloor(1000, 0)).toBe(1000);
            expect(hasMoreAbove(1000, renderCountFromFloor(1000, 0))).toBe(false);
        });

        it('still renders one exchange when a shrunk list leaves the edge past the end', () => {
            expect(renderCountFromFloor(3, 10)).toBe(1);
            expect(renderCountFromFloor(1, 1)).toBe(1);
            expect(computeRenderFromIndex(1, renderCountFromFloor(1, 1))).toBe(0);
        });

        it('has nothing to render on an empty transcript', () => {
            expect(renderCountFromFloor(0, 4)).toBe(0);
        });
    });

    /* The window's promise is about POSITION. `renderCountByThread` held a SIZE,
     * so appending a turn slid the window forward and dropped the oldest one out
     * of the DOM. `windowNeedsFill` grew it back only while the transcript was
     * too short to scroll, which a 375px pane leaves behind in two turns. */
    describe('a turn appended after the seed', () => {
        it('never pushes an already-rendered turn out of the window', () => {
            const costs = flat(2, 3);
            const floor = computeRenderFromIndex(costs.length, seedRenderCount(costs));
            expect(floor).toBe(0);
            // The reader sends a third and a fourth. The edge is what is held, so
            // every turn they have seen stays rendered.
            for (const total of [3, 4]) {
                expect(computeRenderFromIndex(total, renderCountFromFloor(total, floor))).toBe(0);
            }
        });

        it('holds a windowed thread at the same edge as it grows', () => {
            const costs = flat(95, 4);
            const floor = computeRenderFromIndex(costs.length, seedRenderCount(costs));
            expect(floor).toBe(75);
            // Twenty turns later the reader can still scroll back to turn 76.
            const grown = costs.length + 20;
            expect(computeRenderFromIndex(grown, renderCountFromFloor(grown, floor))).toBe(75);
        });
    });
});

// ---------------------------------------------------------------------------
// WALKING UP TO THE TURN THE READER PARKED ON.
//
// A saved *reading position* names a turn (`hooks/useScrollMemory.ts`), because
// this window decides the transcript's height afresh on every reload. The
// restore cannot place the reader until that turn is rendered, so ThreadView
// grows the window until it is.
//
// One budgeted round per commit, never a seed that jumps. The jump would render
// every turn in between in one synchronous pass, which is the blocking render
// windowing exists to prevent (ADR 0081).
// ---------------------------------------------------------------------------
describe('reaching the anchored turn', () => {
    describe('windowReachesIndex', () => {
        it('is true for a turn inside the window, false for one above it', () => {
            // 100 turns, the newest 20 rendered: the window starts at index 80.
            expect(windowReachesIndex(100, 20, 80)).toBe(true);
            expect(windowReachesIndex(100, 20, 99)).toBe(true);
            expect(windowReachesIndex(100, 20, 79)).toBe(false);
            expect(windowReachesIndex(100, 20, 0)).toBe(false);
        });
    });

    describe('windowMustReachIndex', () => {
        it('says no when the turn is already rendered', () => {
            expect(windowMustReachIndex(100, 20, 85)).toBe(false);
        });

        it('says no for a record naming a turn this thread has not got', () => {
            // `findIndex` answers -1. Acting on it would walk the window to the
            // very top of a thread the reader never asked to see.
            expect(windowMustReachIndex(100, 20, -1)).toBe(false);
        });

        it('says no when the thread is rendered whole', () => {
            expect(windowMustReachIndex(20, 20, 0)).toBe(false);
        });

        it('says yes for a turn above the window, with more above to take', () => {
            expect(windowMustReachIndex(100, 20, 40)).toBe(true);
        });
    });

    it('walks to the turn one budgeted round at a time, and stops there', () => {
        // The reported thread's shape, read as the walk ThreadView drives: each
        // commit spends exactly one `expandRenderCount`, and the loop ends the
        // moment the turn is in the window. It must not overshoot to the top.
        const costs = flat(95, 4);
        const target = 20;
        let count = seedRenderCount(costs);
        const rounds: number[] = [];
        while (windowMustReachIndex(costs.length, count, target)) {
            const next = expandRenderCount(costs, count);
            expect(next).toBeGreaterThan(count); // every round takes something
            expect(next - count).toBeLessThanOrEqual(WINDOW_STEP); // and never a jump
            rounds.push(next - count);
            count = next;
        }
        expect(rounds.length).toBeGreaterThan(1); // a real walk, not one hop
        expect(windowReachesIndex(costs.length, count, target)).toBe(true);
        // Stopped AT the turn rather than continuing to the top of the thread.
        expect(computeRenderFromIndex(costs.length, count)).toBeGreaterThan(0);
    });

    it('terminates on a turn at the very top, having rendered the thread whole', () => {
        const costs = flat(60, 9);
        let count = seedRenderCount(costs);
        let rounds = 0;
        while (windowMustReachIndex(costs.length, count, 0)) {
            count = expandRenderCount(costs, count);
            rounds++;
            expect(rounds).toBeLessThan(costs.length + 1); // no unbounded walk
        }
        expect(computeRenderFromIndex(costs.length, count)).toBe(0);
    });
});
