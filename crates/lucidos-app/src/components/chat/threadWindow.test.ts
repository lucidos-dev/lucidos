import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import {
    INITIAL_WINDOW,
    MAX_FILL_EXPANSIONS,
    STEP_BUDGET,
    WINDOW_STEP,
    ROW_BUDGET,
    ROW_CEILING,
    WHOLE_THREAD,
    canSeedRenderWindow,
    computeRenderFromIndex,
    countWithinBudget,
    deepLinkEdge,
    edgeHasMoreAbove,
    edgeMustReachRow,
    MAX_READING_CHASES,
    readingChaseAction,
    edgeReachesRow,
    exchangeRenderCost,
    expandWindowEdge,
    expandRenderCount,
    renderCountFromFloor,
    seedRenderCount,
    seedRowsHidden,
    seedWindowEdge,
    scrollToTopNeedsRenderAll,
    reseedOnReopen,
    fillAction,
    MAX_FILL_BACKFILLS,
    EMPTY_FILL_LEDGER,
    chargeFillRound,
    drawnRowsInWindow,
    fillRoundAllowed,
    settleFillLedger,
    type FillLedger,
    type WindowEdge,
} from './threadWindow';
import { SCROLLABLE_SLACK_PX, transcriptScrolls } from './transcriptScrolls';

const flat = (n: number, cost: number): number[] => Array.from({ length: n }, () => cost);

const REPORTED_THREAD = [23, 80, 10, 52, 47, 7, 12, 53, 11, 60, 32, 32, 53, 23, 11, 0, 36, 17, 34]
    .map(steps => steps + 1);

const ARCHIVED_THREAD = [357, 1, 115, 0, 0, 2, 0, 288, 7].map(steps => steps + 1);
const SKELETON_THREAD = [177, 1, 323, 1].map(steps => steps + 1);

const view = (scrollHeight: number, clientHeight = 800) => ({ scrollHeight, clientHeight });

const edgeOf = (total: number, renderCount: number): WindowEdge =>
    ({ exchange: computeRenderFromIndex(total, renderCount), rowsHidden: 0 });

const allDrawn = (n: number): boolean[] => Array.from({ length: n }, () => true);

const rowsFrom = (rows: readonly number[]) => (index: number) => allDrawn(rows[index] ?? 0);

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

    describe('edgeHasMoreAbove over an exchange-only edge', () => {
        it('is true only when older exchanges sit above the window', () => {
            expect(edgeHasMoreAbove(edgeOf(100, 20))).toBe(true);
            expect(edgeHasMoreAbove(edgeOf(20, 20))).toBe(false);
            expect(edgeHasMoreAbove(edgeOf(5, 20))).toBe(false);
        });
        it('is false once everything is rendered', () => {
            expect(edgeHasMoreAbove(edgeOf(1000, Infinity))).toBe(false);
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
            expect(edgeHasMoreAbove(edgeOf(REPORTED_THREAD.length, seeded))).toBe(true);
        });

        it('renders one exchange even when it alone blows the budget', () => {
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
            expect(expandRenderCount(flat(100, 40), 20)).toBe(24);
        });
        it('grows an ordinary chat by WINDOW_STEP, unchanged', () => {
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

    describe('fillAction', () => {
        const whole = (v: ReturnType<typeof view>, edge: WindowEdge) => fillAction(v, edge, false);

        it('asks nothing once the whole thread is rendered', () => {
            expect(whole(view(120), edgeOf(5, INITIAL_WINDOW))).toBe('none');
            expect(whole(view(120), edgeOf(1000, Infinity))).toBe('none');
        });

        it('grows while a windowed slice is shorter than the pane', () => {
            expect(whole(view(120), edgeOf(9, 1))).toBe('grow');
            expect(whole(view(800 + SCROLLABLE_SLACK_PX), edgeOf(9, 1))).toBe('grow');
        });

        it('stops once the slice overflows by more than the slack', () => {
            expect(whole(view(800 + SCROLLABLE_SLACK_PX + 1), edgeOf(9, 1))).toBe('none');
            expect(transcriptScrolls(view(800 + SCROLLABLE_SLACK_PX + 1))).toBe(true);
            expect(transcriptScrolls(view(800 + SCROLLABLE_SLACK_PX))).toBe(false);
        });

        it('pages when the window is out of loaded turns and the server has more', () => {
            expect(fillAction(view(120), WHOLE_THREAD, true)).toBe('page');
        });

        it('grows before it pages, so a page is asked for only when owed', () => {
            expect(fillAction(view(120), edgeOf(9, 1), true)).toBe('grow');
        });

        it('never pages a transcript the reader can already scroll', () => {
            expect(fillAction(view(4000), WHOLE_THREAD, true)).toBe('none');
        });

        it('seeds both reported threads to a single trailing card', () => {
            expect(seedRenderCount(ARCHIVED_THREAD)).toBe(1);
            expect(seedRenderCount(SKELETON_THREAD)).toBe(1);
        });

        it('fills that card, so scrolling can take the window over', () => {
            for (const costs of [ARCHIVED_THREAD, SKELETON_THREAD]) {
                const seeded = seedRenderCount(costs);
                expect(whole(view(200), edgeOf(costs.length, seeded))).toBe('grow');
                const filled = expandRenderCount(costs, seeded);
                expect(filled).toBe(2);
                expect(whole(view(4000), edgeOf(costs.length, filled))).toBe('none');
            }
        });

        it('charges no round that drew nothing, so the loop ends at the top', () => {
            const costs = flat(200, STEP_BUDGET * 2);
            const silent = () => [false, false];
            let edge = seedWindowEdge(costs, silent);
            let ledger: FillLedger = EMPTY_FILL_LEDGER;
            let rounds = 0;
            while (whole(view(0), edge) === 'grow') {
                const now = { drawn: drawnRowsInWindow(edge, costs.length, silent), floor: Infinity };
                ledger = settleFillLedger(ledger, now);
                if (!fillRoundAllowed(ledger, 'grow')) break;
                edge = expandWindowEdge(edge, costs, silent);
                ledger = chargeFillRound(ledger, 'grow', now);
                rounds++;
                expect(rounds).toBeLessThanOrEqual(costs.length);
            }
            expect(edge).toEqual(WHOLE_THREAD);
        });

        it('never ends a loop of pages by itself', () => {
            expect(fillAction(view(0), WHOLE_THREAD, true)).toBe('page');
            expect(MAX_FILL_BACKFILLS).toBeGreaterThan(0);
            const threadView = readFileSync(
                resolve(dirname(fileURLToPath(import.meta.url)), 'ThreadView.tsx'), 'utf-8');
            for (const kind of ['page', 'grow']) {
                expect(threadView).toContain(`if (!fillRoundAllowed(ledger, '${kind}')) return;`);
                expect(threadView).toContain(`chargeFillRound(ledger, '${kind}', reading)`);
            }
            expect(threadView).toContain('settleFillLedger(stored, reading)');
        });

        it('ends the moment the server says nothing is older', () => {
            expect(fillAction(view(0), WHOLE_THREAD, false)).toBe('none');
        });
    });

    describe('reseedOnReopen', () => {
        it('drops a claim the last visit left behind', () => {
            expect(reseedOnReopen('claimed')).toBe(true);
        });

        it('keeps a window the reader grew, even one reaching the first turn', () => {
            expect(reseedOnReopen('grown')).toBe(false);
        });

        it('leaves a thread with no stored edge to the ordinary seed', () => {
            expect(reseedOnReopen(undefined)).toBe(false);
        });
    });

    describe('scrollToTopNeedsRenderAll', () => {
        it('requires render-all while older exchanges sit above the window', () => {
            expect(scrollToTopNeedsRenderAll(edgeOf(100, INITIAL_WINDOW))).toBe(true);
            expect(scrollToTopNeedsRenderAll(edgeOf(100, INITIAL_WINDOW + WINDOW_STEP))).toBe(true);
        });

        it('skips render-all once the full thread is already rendered', () => {
            expect(scrollToTopNeedsRenderAll(edgeOf(INITIAL_WINDOW, INITIAL_WINDOW))).toBe(false);
            expect(scrollToTopNeedsRenderAll(edgeOf(5, INITIAL_WINDOW))).toBe(false);
            expect(scrollToTopNeedsRenderAll(edgeOf(1000, Infinity))).toBe(false);
        });
    });

    it('windowing then expanding converges to the full list', () => {
        const costs = flat(95, 4);
        const total = costs.length;
        let count = seedRenderCount(costs);
        let from = computeRenderFromIndex(total, count);
        expect(from).toBe(75);
        let guard = 0;
        while (edgeHasMoreAbove(edgeOf(total, count)) && guard++ < 100) {
            count = expandRenderCount(costs, count);
        }
        from = computeRenderFromIndex(total, count);
        expect(from).toBe(0);
        expect(edgeHasMoreAbove(edgeOf(total, count))).toBe(false);
    });

    it('converges on the reported thread too, one budget at a time', () => {
        const total = REPORTED_THREAD.length;
        let count = seedRenderCount(REPORTED_THREAD);
        let guard = 0;
        while (edgeHasMoreAbove(edgeOf(total, count)) && guard++ < 100) {
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
            expect(edgeHasMoreAbove(edgeOf(1000, renderCountFromFloor(1000, 0)))).toBe(false);
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

    describe('a turn appended after the seed', () => {
        it('never pushes an already-rendered turn out of the window', () => {
            const costs = flat(2, 3);
            const floor = computeRenderFromIndex(costs.length, seedRenderCount(costs));
            expect(floor).toBe(0);
            for (const total of [3, 4]) {
                expect(computeRenderFromIndex(total, renderCountFromFloor(total, floor))).toBe(0);
            }
        });

        it('holds a windowed thread at the same edge as it grows', () => {
            const costs = flat(95, 4);
            const floor = computeRenderFromIndex(costs.length, seedRenderCount(costs));
            expect(floor).toBe(75);
            const grown = costs.length + 20;
            expect(computeRenderFromIndex(grown, renderCountFromFloor(grown, floor))).toBe(75);
        });
    });
});

describe('reaching the anchored turn', () => {
    describe('edgeReachesRow over an exchange-only edge', () => {
        it('is true for a turn inside the window, false for one above it', () => {
            expect(edgeReachesRow(edgeOf(100, 20), 80, 0)).toBe(true);
            expect(edgeReachesRow(edgeOf(100, 20), 99, 0)).toBe(true);
            expect(edgeReachesRow(edgeOf(100, 20), 79, 0)).toBe(false);
            expect(edgeReachesRow(edgeOf(100, 20), 0, 0)).toBe(false);
        });
    });

    describe('edgeMustReachRow over an exchange-only edge', () => {
        it('says no when the turn is already rendered', () => {
            expect(edgeMustReachRow(edgeOf(100, 20), 85, 0)).toBe(false);
        });

        it('says no for a record naming a turn this thread has not got', () => {
            expect(edgeMustReachRow(edgeOf(100, 20), -1, 0)).toBe(false);
        });

        it('says no when the thread is rendered whole', () => {
            expect(edgeMustReachRow(edgeOf(20, 20), 0, 0)).toBe(false);
        });

        it('says yes for a turn above the window, with more above to take', () => {
            expect(edgeMustReachRow(edgeOf(100, 20), 40, 0)).toBe(true);
        });
    });

    it('walks to the turn one budgeted round at a time, and stops there', () => {
        const costs = flat(95, 4);
        const target = 20;
        let count = seedRenderCount(costs);
        const rounds: number[] = [];
        while (edgeMustReachRow(edgeOf(costs.length, count), target, 0)) {
            const next = expandRenderCount(costs, count);
            expect(next).toBeGreaterThan(count);
            expect(next - count).toBeLessThanOrEqual(WINDOW_STEP);
            rounds.push(next - count);
            count = next;
        }
        expect(rounds.length).toBeGreaterThan(1);
        expect(edgeReachesRow(edgeOf(costs.length, count), target, 0)).toBe(true);
        expect(computeRenderFromIndex(costs.length, count)).toBeGreaterThan(0);
    });

    it('terminates on a turn at the very top, having rendered the thread whole', () => {
        const costs = flat(60, 9);
        let count = seedRenderCount(costs);
        let rounds = 0;
        while (edgeMustReachRow(edgeOf(costs.length, count), 0, 0)) {
            count = expandRenderCount(costs, count);
            rounds++;
            expect(rounds).toBeLessThan(costs.length + 1);
        }
        expect(computeRenderFromIndex(costs.length, count)).toBe(0);
    });
});

describe('clamping the floor turn', () => {
    const VOICE_LOOP = [1, 65, 684, 425, 1].map(steps => steps + 1);
    const VOICE_LOOP_ROWS = [0, 64, 683, 424, 0];
    const voiceRows = rowsFrom(VOICE_LOOP_ROWS);

    describe('seedRowsHidden', () => {
        it('hides nothing in a turn that fits', () => {
            expect(seedRowsHidden(allDrawn(10))).toBe(0);
            expect(seedRowsHidden(allDrawn(ROW_BUDGET))).toBe(0);
        });

        it('hides everything past the budget', () => {
            expect(seedRowsHidden(allDrawn(ROW_BUDGET + 40))).toBe(40);
        });

        it('always leaves a row, whatever the budget says', () => {
            expect(seedRowsHidden(allDrawn(500), 0)).toBe(499);
            expect(seedRowsHidden(allDrawn(1), 0)).toBe(0);
            expect(seedRowsHidden([])).toBe(0);
        });
    });

    describe('seedWindowEdge', () => {
        it('opens the reported thread on a bounded slice of one turn', () => {
            const edge = seedWindowEdge(VOICE_LOOP, voiceRows);
            expect(edge.exchange).toBe(4);
            expect(edge.rowsHidden).toBe(0);
        });

        it('clamps a floor turn that is bigger than the row budget', () => {
            const edge = seedWindowEdge([900], rowsFrom([880]));
            expect(edge.exchange).toBe(0);
            expect(edge.rowsHidden).toBe(880 - ROW_BUDGET);
        });
    });

    describe('expandWindowEdge', () => {
        it('uncovers the floor turn before reaching past it', () => {
            const edge = { exchange: 2, rowsHidden: 200 };
            const next = expandWindowEdge(edge, VOICE_LOOP, voiceRows);
            expect(next.exchange).toBe(2);
            expect(next.rowsHidden).toBe(200 - ROW_BUDGET);
        });

        it('steps to the previous turn only once the floor one is whole', () => {
            const edge = { exchange: 3, rowsHidden: 0 };
            const next = expandWindowEdge(edge, VOICE_LOOP, voiceRows);
            expect(next.exchange).toBe(2);
            expect(next.rowsHidden).toBe(683 - ROW_BUDGET);
        });

        it('reports no move when the thread is rendered whole', () => {
            const edge = WHOLE_THREAD;
            expect(expandWindowEdge(edge, VOICE_LOOP, voiceRows)).toEqual(edge);
        });

        it('converges on the whole thread, one bounded round at a time', () => {
            let edge = seedWindowEdge(VOICE_LOOP, voiceRows);
            let rounds = 0;
            const drawn: number[] = [];
            while (edgeHasMoreAbove(edge)) {
                const before = edge;
                edge = expandWindowEdge(edge, VOICE_LOOP, voiceRows);
                expect(edge).not.toEqual(before);
                const uncovered = before.exchange === edge.exchange
                    ? before.rowsHidden - edge.rowsHidden
                    : voiceRows(edge.exchange).length - edge.rowsHidden;
                drawn.push(uncovered);
                expect(uncovered).toBeLessThanOrEqual(ROW_BUDGET);
                rounds++;
                expect(rounds).toBeLessThan(200);
            }
            expect(edge).toEqual(WHOLE_THREAD);
            expect(Math.max(...drawn)).toBeLessThanOrEqual(ROW_BUDGET);
        });
    });

    describe('reaching a reading position', () => {
        it('does not call a half-drawn turn reached', () => {
            expect(edgeReachesRow({ exchange: 2, rowsHidden: 40 }, 2, 0)).toBe(false);
            expect(edgeReachesRow({ exchange: 2, rowsHidden: 0 }, 2, 0)).toBe(true);
            expect(edgeReachesRow({ exchange: 2, rowsHidden: 40 }, 3, 0)).toBe(true);
        });

        it('keeps walking until the anchored turn is whole', () => {
            let edge = seedWindowEdge(VOICE_LOOP, voiceRows);
            let rounds = 0;
            while (edgeMustReachRow(edge, 2, 0)) {
                edge = expandWindowEdge(edge, VOICE_LOOP, voiceRows);
                rounds++;
                expect(rounds).toBeLessThan(200);
            }
            expect(edge.exchange).toBe(2);
            expect(edge.rowsHidden).toBe(0);
        });

        it('stops at the anchored turn rather than the top of the thread', () => {
            let edge = seedWindowEdge(VOICE_LOOP, voiceRows);
            while (edgeMustReachRow(edge, 2, 0)) edge = expandWindowEdge(edge, VOICE_LOOP, voiceRows);
            expect(edgeHasMoreAbove(edge)).toBe(true);
        });

        it('is already done for a turn below the floor, and for no turn at all', () => {
            expect(edgeMustReachRow({ exchange: 2, rowsHidden: 40 }, 3, 0)).toBe(false);
            expect(edgeMustReachRow({ exchange: 2, rowsHidden: 40 }, -1, 0)).toBe(false);
            expect(edgeMustReachRow(WHOLE_THREAD, 0, 0)).toBe(false);
        });
    });

    describe('edgeHasMoreAbove', () => {
        it('counts a clamped floor turn, not only older turns', () => {
            expect(edgeHasMoreAbove({ exchange: 0, rowsHidden: 40 })).toBe(true);
            expect(edgeHasMoreAbove({ exchange: 1, rowsHidden: 0 })).toBe(true);
            expect(edgeHasMoreAbove(WHOLE_THREAD)).toBe(false);
        });

        it('makes the scroll-to-top chevron render all for a clamped turn', () => {
            expect(scrollToTopNeedsRenderAll({ exchange: 0, rowsHidden: 40 })).toBe(true);
            expect(scrollToTopNeedsRenderAll(WHOLE_THREAD)).toBe(false);
        });
    });

    describe('deepLinkEdge', () => {
        const edge = { exchange: 200, rowsHidden: 0 };

        it('moves the floor up to a target above the window, whole', () => {
            expect(deepLinkEdge(edge, 120)).toEqual({ exchange: 120, rowsHidden: 0 });
        });

        it('keeps the window when it already draws the target', () => {
            expect(deepLinkEdge(edge, 200)).toBe(edge);
            expect(deepLinkEdge(edge, 230)).toBe(edge);
        });

        it('uncovers the head of a clamped floor turn that holds the target', () => {
            expect(deepLinkEdge({ exchange: 200, rowsHidden: 40 }, 200)).toEqual({ exchange: 200, rowsHidden: 0 });
        });

        it('keeps the window for a target no loaded turn holds', () => {
            expect(deepLinkEdge(edge, -1)).toBe(edge);
        });

        it('never moves the floor down', () => {
            for (let target = 0; target < 260; target++) {
                const next = deepLinkEdge({ exchange: 150, rowsHidden: 7 }, target);
                expect(next.exchange).toBeLessThanOrEqual(150);
            }
        });
    });
});

describe('edgeMustReachRow: a reading position that names a row', () => {
    const edge = { exchange: 5, rowsHidden: 300 };

    it('walks while the named row is still clamped off the floor turn', () => {
        expect(edgeMustReachRow(edge, 5, 120)).toBe(true);
    });

    it('stops as soon as the row is drawn, with the turn head still clamped', () => {
        expect(edgeMustReachRow(edge, 5, 300)).toBe(false);
        expect(edgeMustReachRow(edge, 5, 450)).toBe(false);
    });

    it('walks into an older turn, and stops for any row of a turn already drawn', () => {
        expect(edgeMustReachRow(edge, 3, 999)).toBe(true);
        expect(edgeMustReachRow(edge, 6, 0)).toBe(false);
    });

    it('gives up on a row the loaded turns do not hold', () => {
        expect(edgeMustReachRow(edge, -1, 0)).toBe(false);
    });
});

describe('readingChaseAction: a reading position the loaded pages do not hold', () => {
    const state = { readInFlight: false, paneMeasurable: true, hasOlderEvents: true, chasesSpent: 0 };

    it('chases while older history remains and the bound is not spent', () => {
        expect(readingChaseAction(state)).toBe('chase');
        expect(readingChaseAction({ ...state, chasesSpent: MAX_READING_CHASES - 1 })).toBe('chase');
    });

    it('gives up once the bound is spent, or nothing older remains', () => {
        expect(readingChaseAction({ ...state, chasesSpent: MAX_READING_CHASES })).toBe('give-up');
        expect(readingChaseAction({ ...state, hasOlderEvents: false })).toBe('give-up');
    });

    it('waits for a read already running, whose fold re-runs the walk', () => {
        expect(readingChaseAction({ ...state, readInFlight: true })).toBe('wait');
        expect(readingChaseAction({ ...state, readInFlight: true, chasesSpent: MAX_READING_CHASES })).toBe('wait');
    });

    it('waits, rather than giving up, while the pane has no box', () => {
        expect(readingChaseAction({ ...state, paneMeasurable: false })).toBe('wait');
    });
});

describe('budgeting by the rows the view draws', () => {
    const turn = (runs: readonly number[], showSteps: boolean): boolean[] => runs.flatMap(calls => [
        true,
        ...Array.from({ length: calls }, () => [false, showSteps]).flat(),
    ]);
    const REPORTED_RUNS = [12, 17, 206, 16, 352];
    const hiddenSteps = turn(REPORTED_RUNS, false);
    const onlyTurn = () => hiddenSteps;

    it('reaches a steps-hidden turn\'s prose in bounded rounds', () => {
        let edge = seedWindowEdge([hiddenSteps.length], onlyTurn);
        let rounds = 0;
        while (drawnRowsInWindow(edge, 1, onlyTurn) === 0) {
            const next = expandWindowEdge(edge, [hiddenSteps.length], onlyTurn);
            expect(edge.rowsHidden - next.rowsHidden).toBeLessThanOrEqual(ROW_CEILING);
            edge = next;
            expect(++rounds).toBeLessThan(10);
        }
        expect(rounds).toBeGreaterThan(0);
    });

    it('used to open on nothing, which is the report', () => {
        const oldHidden = hiddenSteps.length - ROW_BUDGET;
        expect(drawnRowsInWindow({ exchange: 0, rowsHidden: oldHidden }, 1, onlyTurn)).toBe(0);
    });

    it('uncovers ROW_BUDGET drawn rows a round, and at least one row', () => {
        const rows = turn(Array.from({ length: 300 }, () => 1), false);
        const at = () => rows;
        let edge = seedWindowEdge([rows.length], at);
        expect(drawnRowsInWindow(edge, 1, at)).toBe(ROW_BUDGET);
        let rounds = 0;
        while (edgeHasMoreAbove(edge)) {
            const before = drawnRowsInWindow(edge, 1, at);
            const next = expandWindowEdge(edge, [rows.length], at);
            expect(next.rowsHidden).toBeLessThan(edge.rowsHidden);
            expect(drawnRowsInWindow(next, 1, at) - before).toBeLessThanOrEqual(ROW_BUDGET);
            edge = next;
            expect(++rounds).toBeLessThan(10);
        }
        expect(drawnRowsInWindow(edge, 1, at)).toBe(300);
    });

    it('crosses a silent head in rounds of at most ROW_CEILING rows', () => {
        const rows = [true, ...Array.from({ length: 704 }, () => false), true];
        let edge: WindowEdge = { exchange: 0, rowsHidden: 705 };
        let rounds = 0;
        while (edgeHasMoreAbove(edge)) {
            const next = expandWindowEdge(edge, [rows.length], () => rows);
            expect(edge.rowsHidden - next.rowsHidden).toBeLessThanOrEqual(ROW_CEILING);
            edge = next;
            rounds++;
        }
        expect(rounds).toBe(Math.ceil(705 / ROW_CEILING));
    });

    it('bounds what showing steps can make draw at once', () => {
        const edge = seedWindowEdge([hiddenSteps.length], onlyTurn);
        const shown = () => hiddenSteps.map(() => true);
        expect(drawnRowsInWindow(edge, 1, shown)).toBeLessThanOrEqual(ROW_CEILING);
    });

    it('passes over the blank text between calls when steps are shown', () => {
        const withSteps = turn(REPORTED_RUNS, true);
        const hidden = seedRowsHidden(withSteps);
        expect(withSteps.slice(hidden).filter(Boolean)).toHaveLength(ROW_BUDGET);
        expect(withSteps.length - hidden).toBeGreaterThan(ROW_BUDGET);
    });

    it('counts the floor turn from its edge and every later turn whole', () => {
        const rows = [[true, true], [true, false, true], [false, true]];
        expect(drawnRowsInWindow({ exchange: 1, rowsHidden: 1 }, 3, i => rows[i])).toBe(2);
        expect(drawnRowsInWindow(WHOLE_THREAD, 3, i => rows[i])).toBe(5);
    });
});

describe('the fill ledger', () => {
    const at = (drawn: number, floor = Infinity) => ({ drawn, floor });

    it('charges a round that drew something', () => {
        let ledger = chargeFillRound(EMPTY_FILL_LEDGER, 'grow', at(0));
        ledger = settleFillLedger(ledger, at(12));
        expect(ledger.grow).toBe(1);
        expect(ledger.last).toBeNull();
    });

    it('stops after MAX_FILL_EXPANSIONS grows that each drew something', () => {
        let ledger: FillLedger = EMPTY_FILL_LEDGER;
        let drawn = 0;
        let rounds = 0;
        while (fillRoundAllowed(ledger, 'grow')) {
            ledger = chargeFillRound(ledger, 'grow', at(drawn));
            drawn += ROW_BUDGET;
            ledger = settleFillLedger(ledger, at(drawn));
            rounds++;
        }
        expect(rounds).toBe(MAX_FILL_EXPANSIONS);
    });

    it('refunds a grow that drew nothing', () => {
        let ledger = chargeFillRound(EMPTY_FILL_LEDGER, 'grow', at(3));
        ledger = settleFillLedger(ledger, at(3));
        expect(ledger.grow).toBe(0);
    });

    it('refunds a page that folded history in yet drew nothing', () => {
        let ledger = chargeFillRound(EMPTY_FILL_LEDGER, 'page', at(0, 5_000));
        ledger = settleFillLedger(ledger, at(0, 4_600));
        expect(ledger.page).toBe(0);
    });

    it('keeps a page charged when only the live turn grew', () => {
        let ledger = chargeFillRound(EMPTY_FILL_LEDGER, 'page', at(0, 5_000));
        ledger = settleFillLedger(ledger, at(0, 5_000));
        expect(ledger.page).toBe(1);
    });

    it('keeps a failed page charged, so a failing endpoint is asked a bounded number of times', () => {
        let ledger: FillLedger = EMPTY_FILL_LEDGER;
        let asks = 0;
        while (fillRoundAllowed(ledger, 'page')) {
            ledger = chargeFillRound(ledger, 'page', at(0, 5_000));
            asks++;
            ledger = settleFillLedger(ledger, at(0, 5_000));
            expect(asks).toBeLessThanOrEqual(MAX_FILL_BACKFILLS);
        }
        expect(asks).toBe(MAX_FILL_BACKFILLS);
    });

    it('pages through silent history to the first page that draws, past the cap', () => {
        let ledger: FillLedger = EMPTY_FILL_LEDGER;
        let floor = 10_000;
        let drawn = 0;
        let pages = 0;
        while (drawn === 0) {
            ledger = settleFillLedger(ledger, at(drawn, floor));
            expect(fillRoundAllowed(ledger, 'page')).toBe(true);
            ledger = chargeFillRound(ledger, 'page', at(drawn, floor));
            floor -= 400;
            pages++;
            if (pages === 11) drawn = 5;
        }
        expect(pages).toBeGreaterThan(MAX_FILL_BACKFILLS);
        expect(settleFillLedger(ledger, at(drawn, floor)).page).toBe(1);
    });
});
