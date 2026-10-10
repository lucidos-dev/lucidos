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
    SCROLLABLE_SLACK_PX,
    STEP_BUDGET,
    WINDOW_STEP,
    ROW_BUDGET,
    ROW_CEILING,
    WHOLE_THREAD,
    canSeedRenderWindow,
    computeRenderFromIndex,
    countWithinBudget,
    deepLinkMustPersist,
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
    transcriptScrolls,
    MAX_FILL_BACKFILLS,
    EMPTY_FILL_LEDGER,
    chargeFillRound,
    drawnRowsInWindow,
    fillRoundAllowed,
    settleFillLedger,
    type FillLedger,
    type WindowEdge,
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
 *  `fillAction` exists for. */
const ARCHIVED_THREAD = [357, 1, 115, 0, 0, 2, 0, 288, 7].map(steps => steps + 1);
const SKELETON_THREAD = [177, 1, 323, 1].map(steps => steps + 1);

/** A viewport the rendered slice is measured against. */
const view = (scrollHeight: number, clientHeight = 800) => ({ scrollHeight, clientHeight });

/** The edge a trailing-exchange count implies, with nothing clamped inside the
 *  floor turn. Lets the cases below stay written in the unit they are about. */
const edgeOf = (total: number, renderCount: number): WindowEdge =>
    ({ exchange: computeRenderFromIndex(total, renderCount), rowsHidden: 0 });

/** `n` rows, every one drawn: the default view, steps and details on. */
const allDrawn = (n: number): boolean[] => Array.from({ length: n }, () => true);

/** A `rowsAt` over a fixed list of row counts, every row drawn, so a case can
 *  say how big each turn is without folding anything. Turns not listed have no
 *  rows. */
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
            expect(edgeHasMoreAbove(edgeOf(REPORTED_THREAD.length, seeded))).toBe(true);
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

    describe('fillAction', () => {
        /** A thread served whole: nothing older is coming. */
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

        /** The reported stick. Every turn this client held was already
         *  rendered, so nothing was left to grow. Meanwhile 662 events sat on
         *  the server, behind a scroll event a short transcript never fires. */
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
                // One round takes the working turn behind the boundary, which
                // is the biggest turn in either thread.
                expect(filled).toBe(2);
                expect(whole(view(4000), edgeOf(costs.length, filled))).toBe('none');
            }
        });

        it('charges no round that drew nothing, so the loop ends at the top', () => {
            // Every turn blows the budget alone and draws no row. Charging those
            // rounds stopped the loop at the cap with the pane still empty. Each
            // one moved the edge up, so the thread's own top ends the loop.
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

        /** The page arm answers from the SERVER's watermark, so it keeps saying
         *  `'page'` while a page draws no height. Nothing here can end that,
         *  which is exactly why `MAX_FILL_BACKFILLS` exists in the caller. A
         *  loop written against this function alone would only ever prove its
         *  own counter, so the cap is asserted where it lives. */
        it('never ends a loop of pages by itself', () => {
            expect(fillAction(view(0), WHOLE_THREAD, true)).toBe('page');
            expect(MAX_FILL_BACKFILLS).toBeGreaterThan(0);
            const threadView = readFileSync(
                resolve(dirname(fileURLToPath(import.meta.url)), 'ThreadView.tsx'), 'utf-8');
            // BOTH halves, for both kinds. A guard over a ledger nothing charges
            // allows every round for ever, and every suite stays green while
            // the fill pages without bound.
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

    /** A render-all is a NAVIGATION's claim, and it used to outlive the visit
     *  that made it. One reported thread paid 4,069 rendered rows on every open
     *  afterwards, against the 37 its window would have drawn. */
    describe('reseedOnReopen', () => {
        it('drops a render-all the last visit left behind', () => {
            expect(reseedOnReopen('render-all', false)).toBe(true);
        });

        it('keeps the one THIS visit is asking for', () => {
            expect(reseedOnReopen('render-all', true)).toBe(false);
        });

        /** The reader grew this one by scrolling, so it describes where they
         *  are. Re-seeding would make them walk back up on every return.
         *
         *  That holds for a window grown all the way to the first turn. Its
         *  edge equals `WHOLE_THREAD` by value. Read as a claim, the thread
         *  would open on the newest turns and jump when the walk arrives. */
        it('keeps a window the reader grew, even one reaching the first turn', () => {
            expect(reseedOnReopen('grown', false)).toBe(false);
            expect(reseedOnReopen('grown', true)).toBe(false);
        });

        it('leaves a thread with no stored edge to the ordinary seed', () => {
            expect(reseedOnReopen(undefined, false)).toBe(false);
        });
    });

    describe('scrollToTopNeedsRenderAll', () => {
        it('requires render-all while older exchanges sit above the window', () => {
            // A long thread opens windowed to the tail — scroll-to-top must render
            // the whole thread first, or it stalls partway (the "needed N clicks" bug).
            expect(scrollToTopNeedsRenderAll(edgeOf(100, INITIAL_WINDOW))).toBe(true);
            // One scroll-up expansion isn't enough on a long thread, still windowed.
            expect(scrollToTopNeedsRenderAll(edgeOf(100, INITIAL_WINDOW + WINDOW_STEP))).toBe(true);
        });

        it('skips render-all once the full thread is already rendered', () => {
            // Short thread fits in the initial window — scroll straight to top.
            expect(scrollToTopNeedsRenderAll(edgeOf(INITIAL_WINDOW, INITIAL_WINDOW))).toBe(false);
            expect(scrollToTopNeedsRenderAll(edgeOf(5, INITIAL_WINDOW))).toBe(false);
            // Already expanded to everything (deep-link render-all / a prior top jump).
            expect(scrollToTopNeedsRenderAll(edgeOf(1000, Infinity))).toBe(false);
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

    /* The window's promise is about POSITION. `renderCountByThread` held a SIZE,
     * so appending a turn slid the window forward and dropped the oldest one out
     * of the DOM. `fillAction` grew it back only while the transcript was
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
    describe('edgeReachesRow over an exchange-only edge', () => {
        it('is true for a turn inside the window, false for one above it', () => {
            // 100 turns, the newest 20 rendered: the window starts at index 80.
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
            // `findIndex` answers -1. Acting on it would walk the window to the
            // very top of a thread the reader never asked to see.
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
        // The reported thread's shape, read as the walk ThreadView drives: each
        // commit spends exactly one `expandRenderCount`, and the loop ends the
        // moment the turn is in the window. It must not overshoot to the top.
        const costs = flat(95, 4);
        const target = 20;
        let count = seedRenderCount(costs);
        const rounds: number[] = [];
        while (edgeMustReachRow(edgeOf(costs.length, count), target, 0)) {
            const next = expandRenderCount(costs, count);
            expect(next).toBeGreaterThan(count); // every round takes something
            expect(next - count).toBeLessThanOrEqual(WINDOW_STEP); // and never a jump
            rounds.push(next - count);
            count = next;
        }
        expect(rounds.length).toBeGreaterThan(1); // a real walk, not one hop
        expect(edgeReachesRow(edgeOf(costs.length, count), target, 0)).toBe(true);
        // Stopped AT the turn rather than continuing to the top of the thread.
        expect(computeRenderFromIndex(costs.length, count)).toBeGreaterThan(0);
    });

    it('terminates on a turn at the very top, having rendered the thread whole', () => {
        const costs = flat(60, 9);
        let count = seedRenderCount(costs);
        let rounds = 0;
        while (edgeMustReachRow(edgeOf(costs.length, count), 0, 0)) {
            count = expandRenderCount(costs, count);
            rounds++;
            expect(rounds).toBeLessThan(costs.length + 1); // no unbounded walk
        }
        expect(computeRenderFromIndex(costs.length, count)).toBe(0);
    });
});

/** The row dimension: what the exchange budget above could never bound.
 *
 *  The reported coding-agent thread holds five turns costing 1, 65, 684, 425
 *  and 1 steps against a `STEP_BUDGET` of 160. Every real turn blows the whole
 *  budget alone. So "at least one exchange" was the only rule that ever fired,
 *  and the window rendered a turn of everything. */
describe('clamping the floor turn', () => {
    /** The reported thread's turn sizes, oldest first, in raw events. */
    const VOICE_LOOP = [1, 65, 684, 425, 1].map(steps => steps + 1);
    /** The same turns as rows drawn, roughly half their raw events. */
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
            // The floor `countWithinBudget` keeps one level up: a turn admitted
            // to the window draws something, or the transcript reads as empty.
            expect(seedRowsHidden(allDrawn(500), 0)).toBe(499);
            expect(seedRowsHidden(allDrawn(1), 0)).toBe(0);
            expect(seedRowsHidden([])).toBe(0);
        });
    });

    describe('seedWindowEdge', () => {
        it('opens the reported thread on a bounded slice of one turn', () => {
            const edge = seedWindowEdge(VOICE_LOOP, voiceRows);
            // The step budget takes only the trailing boundary turn, which
            // draws nothing, so the seed alone still shows an empty pane. What
            // matters is that it is an EDGE the fill can grow from.
            expect(edge.exchange).toBe(4);
            expect(edge.rowsHidden).toBe(0);
        });

        it('clamps a floor turn that is bigger than the row budget', () => {
            // One turn, larger than the whole step budget. The exchange rule's
            // "always take at least one" admits it, and the row rule is what
            // then stops it drawing 880 rows on the open.
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
            // And clamps THAT turn in its own turn: 683 rows do not all draw.
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
                expect(edge).not.toEqual(before); // every round takes something
                // No round may hand the reader an unbounded turn.
                const uncovered = before.exchange === edge.exchange
                    ? before.rowsHidden - edge.rowsHidden
                    : voiceRows(edge.exchange).length - edge.rowsHidden;
                drawn.push(uncovered);
                expect(uncovered).toBeLessThanOrEqual(ROW_BUDGET);
                rounds++;
                expect(rounds).toBeLessThan(200); // no unbounded walk
            }
            expect(edge).toEqual(WHOLE_THREAD);
            // The old window did this in ONE round of 1175 rows.
            expect(Math.max(...drawn)).toBeLessThanOrEqual(ROW_BUDGET);
        });
    });

    describe('reaching a reading position', () => {
        it('does not call a half-drawn turn reached', () => {
            // ADR 0152 restores by the turn's own top edge, so a clamped head
            // means the restore has nothing to measure from.
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
            // The up chevron and the fill both read this. A turn whose head is
            // off screen is exactly as unreachable as a turn that is.
            expect(edgeHasMoreAbove({ exchange: 0, rowsHidden: 40 })).toBe(true);
            expect(edgeHasMoreAbove({ exchange: 1, rowsHidden: 0 })).toBe(true);
            expect(edgeHasMoreAbove(WHOLE_THREAD)).toBe(false);
        });

        it('makes the scroll-to-top chevron render all for a clamped turn', () => {
            expect(scrollToTopNeedsRenderAll({ exchange: 0, rowsHidden: 40 })).toBe(true);
            expect(scrollToTopNeedsRenderAll(WHOLE_THREAD)).toBe(false);
        });
    });

    describe('deepLinkMustPersist', () => {
        it('writes when nothing is stored yet, which is the cold push tap', () => {
            // The claim lands before the seed on a cold open, so this arm is
            // the whole point. Skip it and the seed stores the tail, and the
            // thread snaps back to it when the claim clears.
            expect(deepLinkMustPersist(undefined)).toBe(true);
        });

        it('writes over a windowed edge', () => {
            expect(deepLinkMustPersist({ exchange: 3, rowsHidden: 0 })).toBe(true);
            expect(deepLinkMustPersist({ exchange: 0, rowsHidden: 40 })).toBe(true);
        });

        it('leaves an edge that already renders the whole thread alone', () => {
            expect(deepLinkMustPersist(WHOLE_THREAD)).toBe(false);
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

/** The row budget counts rows the reader's view DRAWS.
 *
 *  The reported turn: 748 text events, 45 with prose, and runs of 352 and 206
 *  tool calls between two lines of prose. With steps hidden, each call and the
 *  blank text before it draw nothing. Counted as rows, they filled the seed and
 *  every fill round, and the reader got a header over an empty body. */
describe('budgeting by the rows the view draws', () => {
    /** A coding-agent turn as its rows: each run is one prose chunk, then that
     *  many tool calls, each preceded by the blank text the agent writes. */
    const turn = (runs: readonly number[], showSteps: boolean): boolean[] => runs.flatMap(calls => [
        true,
        ...Array.from({ length: calls }, () => [false, showSteps]).flat(),
    ]);
    const REPORTED_RUNS = [12, 17, 206, 16, 352];
    const hiddenSteps = turn(REPORTED_RUNS, false);
    const onlyTurn = () => hiddenSteps;

    it('reaches a steps-hidden turn\'s prose in bounded rounds', () => {
        // The seed stops at the ceiling, deep inside the 352-call run. Each
        // round the fill takes after it is uncharged, since it draws nothing.
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
        // The old unit: the last ROW_BUDGET rows, all inside the 352-call run.
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
        // The edge is not re-seeded on the toggle, so the rows it uncovered
        // while steps were hidden all draw the moment they are shown.
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

/** A fill round that drew nothing is not charged. The caps bound rounds that
 *  DREW, and a refund always follows progress, so the loop still ends. */
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
        // The live turn streams in at the NEWEST end. The floor holds still,
        // so a failed page on a live thread is not mistaken for progress.
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
            // A failed read folds nothing in: no progress, no refund.
            ledger = settleFillLedger(ledger, at(0, 5_000));
            expect(asks).toBeLessThanOrEqual(MAX_FILL_BACKFILLS);
        }
        expect(asks).toBe(MAX_FILL_BACKFILLS);
    });

    it('pages through silent history to the first page that draws, past the cap', () => {
        // Ten silent pages, then one with prose: the shape of a long run of
        // tool calls a reader with steps hidden opens in the middle of.
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
