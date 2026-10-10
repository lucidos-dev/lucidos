/**
 * Tests for the Blocked, review, and in-flight group helpers.
 *
 * The Ongoing grouping lists threads in these groups, among others:
 *   - "Blocked" — every Current/Saved thread where the agent is stuck
 *     waiting on the user: awaiting an answer/permission (waiting_for_user_answer)
 *     or a failed turn. Ordered by review tier (User Q / permission ahead of a
 *     failed turn), then most-recent-first within each tier.
 *   - "Review" — every settled Current/Saved thread carrying a change ready
 *     to apply or a read request. Most-recent-first.
 *   - "In flight" — every Current/Saved thread whose status dot reads Running
 *     or Waiting: a running turn, its own event wait, or unfinished sub-threads.
 *     Roots most-recent-first, each sub-thread nested under its parent.
 * All views bypass the channel/trigger/repo filters and the lifecycle section
 * grouping. Each group's header counts its own rows, and the Blocked
 * badge (`blockedThreadCount`) shares `threadIsBlocked`, so a count and
 * its list can never disagree. A failed thread with a
 * proposed change surfaces in both Blocked and review. One awaiting an
 * answer is Blocked only, since the open question withholds Apply.
 * In flight is mutually exclusive with both.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { blockedThreads, reviewThreads, inFlightThreads, inFlightBreakdown, threadIsInFlight } from './ThreadDrawer';
import type { NestedThread } from './ThreadDrawer';
import { filterInFlightRows } from './family-graph';
import { blockedThreadCount, threadIsBlocked, threadInReview, threadMap } from '../../store/store';
import type { ThreadState, ThreadMeta, ThreadStatus } from '../../store/thread-events';
import type { ArchiveState } from '../../generated/thread-lifecycle';
import type { CodingAgentChangeState } from '../../api/threads';

type ThreadOpts = {
    section?: ArchiveState;
    status?: ThreadStatus;
    saved?: boolean;
    codingAgentChangeState?: CodingAgentChangeState;
    state?: ThreadMeta['state'];
    updatedAt?: string;
    liveEventWaitCount?: number;
    activeChildrenCount?: number;
    waitingChildrenCount?: number;
    parentThreadId?: string;
    isStoppedChild?: boolean;
    readRequested?: boolean;
};

function makeThread(id: string, opts: ThreadOpts = {}): ThreadState {
    const meta: ThreadMeta = {
        id,
        title: id,
        channel: 'chat',
        initiator: 'user',
        saved: opts.saved ?? false,
        createdAt: opts.updatedAt ?? '2026-05-01T00:00:00Z',
        updatedAt: opts.updatedAt ?? '2026-05-01T00:00:00Z',
        status: opts.status ?? 'idle',
        summaryVersion: 0,
        messageCount: 1,
        section: opts.section ?? 'inbox',
        activeChildrenCount: opts.activeChildrenCount ?? 0,
        waitingChildrenCount: opts.waitingChildrenCount ?? 0,
        totalChildrenCount: (opts.activeChildrenCount ?? 0) + (opts.waitingChildrenCount ?? 0),
        blockingDescendantCount: 0,
        attentionDescendantCount: 0,
        codingAgentChangeState: opts.codingAgentChangeState ?? { kind: 'none' },
        codingAgentIsExternalRepo: false,
        lastRevivedAt: '',
        state: opts.state ?? 'active',
        latestTodoList: null,
        liveEventWaitCount: opts.liveEventWaitCount ?? 0,
        liveEventWaits: [],
        parentThreadId: opts.parentThreadId,
        isStoppedChild: opts.isStoppedChild,
        readRequested: opts.readRequested,
    };
    return {
        meta,
        events: new Map(),
        streamingBuffer: '',
        eventsLoaded: false,
        eventsLoadFailed: false,
        lastDbSeq: 0,
        pendingUserMessages: [],
    };
}

function asMap(threads: ThreadState[]): Map<string, ThreadState> {
    return new Map(threads.map(t => [t.meta.id, t]));
}

function ids(list: ThreadState[]): string[] {
    return list.map(t => t.meta.id);
}

function shape(list: NestedThread[]): [string, number][] {
    return list.map(n => [n.thread.meta.id, n.depth]);
}

function rootIds(list: NestedThread[]): string[] {
    return list.map(n => n.thread.meta.id);
}

beforeEach(() => {
    threadMap.value = new Map();
});

describe('blockedThreads', () => {
    it('includes a Current thread awaiting a user answer', () => {
        const waiting = makeThread('a', { section: 'inbox', status: 'waiting_for_user_answer' });
        expect(ids(blockedThreads(asMap([waiting])))).toEqual(['a']);
    });

    it('includes a Current thread whose last turn failed', () => {
        const failed = makeThread('a', { section: 'inbox', status: 'failed' });
        expect(ids(blockedThreads(asMap([failed])))).toEqual(['a']);
    });

    it('includes a blocked Saved thread', () => {
        // A saved thread routes to Saved; an awaiting-answer state still demands
        // attention there.
        const saved = makeThread('a', { saved: true, status: 'waiting_for_user_answer' });
        expect(ids(blockedThreads(asMap([saved])))).toEqual(['a']);
    });

    it('excludes a proposed-only thread (that is Review, not attention)', () => {
        // A change merely ready to apply is the Review view\'s job now; with no
        // waiting/failed state the agent is not stuck on the user, so it is NOT
        // Blocked.
        const proposed = makeThread('a', { section: 'inbox', codingAgentChangeState: { kind: 'proposed', requires_restart: false }});
        expect(blockedThreads(asMap([proposed]))).toEqual([]);
    });

    it('excludes a running thread (the system\'s turn)', () => {
        const running = makeThread('a', { section: 'inbox', status: 'running' });
        expect(blockedThreads(asMap([running]))).toEqual([]);
    });

    it('excludes an idle thread with nothing pending', () => {
        const idle = makeThread('a', { section: 'inbox', status: 'idle' });
        expect(blockedThreads(asMap([idle]))).toEqual([]);
    });

    it('excludes an acknowledged (archived) failed thread', () => {
        // Archived = the user acknowledged the failure; it leaves the Current
        // section, so it must not resurface in the attention view.
        const archivedFail = makeThread('a', { section: 'archived', status: 'failed' });
        expect(blockedThreads(asMap([archivedFail]))).toEqual([]);
    });

    it('excludes composing and discarded threads', () => {
        // Composing carries an otherwise-attention status to prove the
        // composing-state exclusion overrides it.
        const composing = makeThread('a', { state: 'composing', status: 'waiting_for_user_answer' });
        const discarded = makeThread('b', { state: 'discarded', status: 'failed' });
        expect(blockedThreads(asMap([composing, discarded]))).toEqual([]);
    });

    it('sorts most-recent-first within a tier', () => {
        const old = makeThread('old', { status: 'failed', updatedAt: '2026-05-01T00:00:00Z' });
        const fresh = makeThread('fresh', { status: 'failed', updatedAt: '2026-05-04T00:00:00Z' });
        const mid = makeThread('mid', { status: 'failed', updatedAt: '2026-05-02T00:00:00Z' });
        expect(ids(blockedThreads(asMap([old, fresh, mid])))).toEqual(['fresh', 'mid', 'old']);
    });

    it('floats User Q / permission ahead of a failed turn', () => {
        // waiting_for_user_answer (tier 0 — the agent is stalled until the user
        // answers) sorts above a failed turn (tier 1), even when the failure is
        // fresher. Recency only breaks ties within a tier.
        const failed = makeThread('failed', { status: 'failed', updatedAt: '2026-05-04T00:00:00Z' });
        const waiting = makeThread('waiting', { status: 'waiting_for_user_answer', updatedAt: '2026-05-01T00:00:00Z' });
        expect(ids(blockedThreads(asMap([failed, waiting])))).toEqual(['waiting', 'failed']);
    });

    it('orders by tier first, then recency within each tier', () => {
        const waitOld = makeThread('wait-old', { status: 'waiting_for_user_answer', updatedAt: '2026-05-01T00:00:00Z' });
        const waitNew = makeThread('wait-new', { status: 'waiting_for_user_answer', updatedAt: '2026-05-03T00:00:00Z' });
        const failedNew = makeThread('failed-new', { status: 'failed', updatedAt: '2026-05-05T00:00:00Z' });
        const failedOld = makeThread('failed-old', { status: 'failed', updatedAt: '2026-05-02T00:00:00Z' });
        // Tier 0 (waiting) ahead of tier 1 (failed); fresher first inside each.
        expect(ids(blockedThreads(asMap([failedOld, failedNew, waitOld, waitNew]))))
            .toEqual(['wait-new', 'wait-old', 'failed-new', 'failed-old']);
    });

    it('returns empty when nothing is blocked', () => {
        const a = makeThread('a', { status: 'idle' });
        const b = makeThread('b', { status: 'running' });
        expect(blockedThreads(asMap([a, b]))).toEqual([]);
    });
});

describe('reviewThreads', () => {
    it('includes a Current thread with a change ready to apply', () => {
        const proposed = makeThread('a', { section: 'inbox', codingAgentChangeState: { kind: 'proposed', requires_restart: false }});
        expect(ids(reviewThreads(asMap([proposed])))).toEqual(['a']);
    });

    it('includes a Saved thread with a change ready to apply', () => {
        const saved = makeThread('a', { saved: true, codingAgentChangeState: { kind: 'proposed', requires_restart: false }});
        expect(ids(reviewThreads(asMap([saved])))).toEqual(['a']);
    });

    it('includes an archived thread that has a change ready to apply', () => {
        // A pending change lifts an archived thread back to Current (it still
        // needs an Apply/Discard), so it surfaces in the review view.
        const archivedProposed = makeThread('a', { section: 'archived', codingAgentChangeState: { kind: 'proposed', requires_restart: false }});
        expect(ids(reviewThreads(asMap([archivedProposed])))).toEqual(['a']);
    });

    it('excludes a running thread whose change is not yet ready to apply', () => {
        // A change proposed while a follow-up turn is still in flight is not yet
        // *ready* to apply — the WaitingBanner shows Cancel, not Apply — so it
        // must stay out of the review view until the turn idles.
        const runningProposed = makeThread('a', { section: 'inbox', status: 'running', codingAgentChangeState: { kind: 'proposed', requires_restart: false }});
        expect(reviewThreads(asMap([runningProposed]))).toEqual([]);
    });

    it('excludes a thread parked on an event wait, whose change is not final', () => {
        // It wakes on its delivery and commits on to the same branch, so the
        // engine withholds Apply and the dot reads Waiting, not Changes.
        const parked = makeThread('a', { section: 'inbox', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, liveEventWaitCount: 1 });
        expect(reviewThreads(asMap([parked]))).toEqual([]);
    });

    it('excludes a thread whose work a turn end withheld, since nothing is proposed (ADR 0400)', () => {
        const stopped = makeThread('a', { section: 'inbox', codingAgentChangeState: { kind: 'unproposed', reason: 'turn_incomplete' } });
        expect(reviewThreads(asMap([stopped]))).toEqual([]);
        expect(threadInReview(stopped)).toBe(false);
    });

    it('excludes a waiting/failed thread with no proposed change (that is attention)', () => {
        const waiting = makeThread('a', { status: 'waiting_for_user_answer' });
        const failed = makeThread('b', { status: 'failed' });
        expect(reviewThreads(asMap([waiting, failed]))).toEqual([]);
    });

    it('excludes composing and discarded threads', () => {
        const composing = makeThread('a', { state: 'composing', codingAgentChangeState: { kind: 'proposed', requires_restart: false }});
        const discarded = makeThread('b', { state: 'discarded', codingAgentChangeState: { kind: 'proposed', requires_restart: false }});
        expect(reviewThreads(asMap([composing, discarded]))).toEqual([]);
    });

    it('sorts most-recent-first', () => {
        const old = makeThread('old', { codingAgentChangeState: { kind: 'proposed', requires_restart: false }, updatedAt: '2026-05-01T00:00:00Z' });
        const fresh = makeThread('fresh', { codingAgentChangeState: { kind: 'proposed', requires_restart: false }, updatedAt: '2026-05-04T00:00:00Z' });
        const mid = makeThread('mid', { codingAgentChangeState: { kind: 'proposed', requires_restart: false }, updatedAt: '2026-05-02T00:00:00Z' });
        expect(ids(reviewThreads(asMap([old, fresh, mid])))).toEqual(['fresh', 'mid', 'old']);
    });
});

describe('inFlightThreads', () => {
    it('includes a Current thread actively working', () => {
        const running = makeThread('a', { section: 'inbox', status: 'running' });
        expect(rootIds(inFlightThreads(asMap([running])))).toEqual(['a']);
    });

    it('includes a Saved thread actively working', () => {
        const saved = makeThread('a', { saved: true, status: 'running' });
        expect(rootIds(inFlightThreads(asMap([saved])))).toEqual(['a']);
    });

    it('includes a thread that is running with a proposed change', () => {
        // Its change is not ready to apply until the turn idles, so it is NOT
        // in Review yet.
        const runningProposed = makeThread('a', { section: 'inbox', status: 'running', codingAgentChangeState: { kind: 'proposed', requires_restart: false }});
        expect(rootIds(inFlightThreads(asMap([runningProposed])))).toEqual(['a']);
    });

    it('includes a thread parked on its own event wait', () => {
        const parked = makeThread('a', { liveEventWaitCount: 1 });
        expect(rootIds(inFlightThreads(asMap([parked])))).toEqual(['a']);
    });

    it('includes a thread parked on an event wait with a proposed change', () => {
        // Review leaves it out, since the change is not final. It must land here.
        const parked = makeThread('a', { codingAgentChangeState: { kind: 'proposed', requires_restart: false }, liveEventWaitCount: 1 });
        expect(rootIds(inFlightThreads(asMap([parked])))).toEqual(['a']);
    });

    it('includes a parent waiting on a working sub-thread', () => {
        const parent = makeThread('a', { activeChildrenCount: 1 });
        expect(rootIds(inFlightThreads(asMap([parent])))).toEqual(['a']);
    });

    it('includes a parent waiting on a sub-thread asleep on its own event wait', () => {
        const parent = makeThread('a', { waitingChildrenCount: 1 });
        expect(rootIds(inFlightThreads(asMap([parent])))).toEqual(['a']);
    });

    it('excludes a parent whose change is ready and that waits only on sub-threads', () => {
        // Its dot reads Changes to review and it offers Apply, so it is Review's.
        const parent = makeThread('a', { codingAgentChangeState: { kind: 'proposed', requires_restart: false }, activeChildrenCount: 1 });
        expect(inFlightThreads(asMap([parent]))).toEqual([]);
        expect(threadInReview(parent)).toBe(true);
    });

    it('excludes idle / question / failed / paused threads', () => {
        const idle = makeThread('a', { status: 'idle' });
        const question = makeThread('b', { status: 'waiting_for_user_answer' });
        const failed = makeThread('c', { status: 'failed' });
        const paused = makeThread('d', { status: 'paused' });
        expect(inFlightThreads(asMap([idle, question, failed, paused]))).toEqual([]);
    });

    it('excludes a question or failure even while an event wait is live', () => {
        // The dot reads the turn's own state first, and Blocked owns it.
        const question = makeThread('a', { status: 'waiting_for_user_answer', liveEventWaitCount: 1 });
        const failed = makeThread('b', { status: 'failed', liveEventWaitCount: 1 });
        expect(inFlightThreads(asMap([question, failed]))).toEqual([]);
    });

    it('excludes a stopped sub-thread, which needs the user', () => {
        const stopped = makeThread('a', { isStoppedChild: true, activeChildrenCount: 1 });
        expect(inFlightThreads(asMap([stopped]))).toEqual([]);
        expect(threadIsBlocked(stopped)).toBe(true);
    });

    it('excludes composing, discarded and archived threads', () => {
        const composing = makeThread('a', { state: 'composing', status: 'running' });
        const discarded = makeThread('b', { state: 'discarded', status: 'running' });
        const archived = makeThread('c', { section: 'archived', liveEventWaitCount: 1 });
        expect(inFlightThreads(asMap([composing, discarded, archived]))).toEqual([]);
    });

    it('sorts roots most-recent-first', () => {
        const old = makeThread('old', { status: 'running', updatedAt: '2026-05-01T00:00:00Z' });
        const fresh = makeThread('fresh', { liveEventWaitCount: 1, updatedAt: '2026-05-04T00:00:00Z' });
        const mid = makeThread('mid', { activeChildrenCount: 1, updatedAt: '2026-05-02T00:00:00Z' });
        expect(rootIds(inFlightThreads(asMap([old, fresh, mid])))).toEqual(['fresh', 'mid', 'old']);
    });

    it('nests each sub-thread under its in-flight parent', () => {
        const parent = makeThread('parent', { activeChildrenCount: 2, updatedAt: '2026-05-01T00:00:00Z' });
        const working = makeThread('working', { status: 'running', parentThreadId: 'parent', updatedAt: '2026-05-03T00:00:00Z' });
        const parked = makeThread('parked', { liveEventWaitCount: 1, parentThreadId: 'parent', updatedAt: '2026-05-02T00:00:00Z' });
        const other = makeThread('other', { status: 'running', updatedAt: '2026-05-04T00:00:00Z' });
        expect(shape(inFlightThreads(asMap([parent, working, parked, other])))).toEqual([
            ['other', 0],
            ['parent', 0],
            ['working', 1],
            ['parked', 1],
        ]);
    });

    it('renders a sub-thread at root level when its parent is not in flight', () => {
        // The parent waits on the user, so it is in Blocked instead.
        const parent = makeThread('parent', { status: 'waiting_for_user_answer' });
        const child = makeThread('child', { status: 'running', parentThreadId: 'parent' });
        expect(shape(inFlightThreads(asMap([parent, child])))).toEqual([['child', 0]]);
    });
});

describe('the In flight count', () => {
    it('counts exactly the threads the predicate takes', () => {
        const threads = [
            makeThread('running-1', { status: 'running' }),
            makeThread('running-2', { status: 'running', codingAgentChangeState: { kind: 'proposed', requires_restart: false }}),
            makeThread('parked', { liveEventWaitCount: 1 }),
            makeThread('parent', { activeChildrenCount: 1 }),
            makeThread('waiting', { status: 'waiting_for_user_answer' }), // attention, not in flight
            makeThread('proposed', { codingAgentChangeState: { kind: 'proposed', requires_restart: false }}),        // review, not in flight
            makeThread('idle', { status: 'idle' }),                       // excluded
        ];
        threadMap.value = asMap(threads);
        expect(inFlightThreads(threadMap.value)).toHaveLength(4);
    });

    it('is zero when nothing is in flight', () => {
        threadMap.value = asMap([makeThread('a', { status: 'idle' })]);
        expect(inFlightThreads(threadMap.value)).toHaveLength(0);
    });
});

describe('threadIsInFlight', () => {
    it('is true for a running thread and a parked one', () => {
        expect(threadIsInFlight(makeThread('a', { status: 'running' }))).toBe(true);
        expect(threadIsInFlight(makeThread('b', { liveEventWaitCount: 1 }))).toBe(true);
    });

    it('is false for idle / question / failed threads', () => {
        expect(threadIsInFlight(makeThread('a', { status: 'idle' }))).toBe(false);
        expect(threadIsInFlight(makeThread('b', { status: 'waiting_for_user_answer' }))).toBe(false);
        expect(threadIsInFlight(makeThread('c', { status: 'failed' }))).toBe(false);
    });

    it('is false for a composing thread even when its status is running', () => {
        expect(threadIsInFlight(makeThread('a', { state: 'composing', status: 'running' }))).toBe(false);
    });
});

describe('inFlightBreakdown', () => {
    it('counts running rows apart from rows parked on a wait', () => {
        const rows = inFlightThreads(asMap([
            makeThread('parked', { liveEventWaitCount: 1 }),
            makeThread('parent', { activeChildrenCount: 1 }),
            makeThread('running', { status: 'running' }),
        ]));
        expect(inFlightBreakdown(rows)).toEqual({ running: 1, waiting: 2 });
    });

    it('reports no running row when every row is parked on a wait', () => {
        const rows = inFlightThreads(asMap([
            makeThread('parked', { liveEventWaitCount: 1 }),
            makeThread('parent', { activeChildrenCount: 1 }),
        ]));
        expect(inFlightBreakdown(rows)).toEqual({ running: 0, waiting: 2 });
    });
});

describe('filterInFlightRows', () => {
    const rows = () => inFlightThreads(asMap([
        makeThread('parent', { activeChildrenCount: 1, updatedAt: '2026-05-02T00:00:00Z' }),
        makeThread('child', { status: 'running', parentThreadId: 'parent' }),
        makeThread('parked', { liveEventWaitCount: 1 }),
    ]));
    const shape = (nested: NestedThread[]) => nested.map(n => [n.thread.meta.id, n.depth]);

    it('keeps every row, nested, for all', () => {
        expect(shape(filterInFlightRows(rows(), 'all'))).toEqual([['parent', 0], ['child', 1], ['parked', 0]]);
    });

    it('keeps the running rows, a child whose parent waits standing at root level', () => {
        expect(shape(filterInFlightRows(rows(), 'running'))).toEqual([['child', 0]]);
    });

    it('keeps the waiting rows', () => {
        expect(shape(filterInFlightRows(rows(), 'waiting'))).toEqual([['parent', 0], ['parked', 0]]);
    });
});

describe('In flight never claims a thread Blocked or Review holds', () => {
    it('keeps Blocked, Review and In flight disjoint', () => {
        const threads = [
            makeThread('running', { status: 'running' }),
            makeThread('running-proposed', { status: 'running', codingAgentChangeState: { kind: 'proposed', requires_restart: false }}),
            makeThread('parked', { liveEventWaitCount: 1 }),
            makeThread('parked-proposed', { liveEventWaitCount: 1, codingAgentChangeState: { kind: 'proposed', requires_restart: false }}),
            makeThread('parent', { activeChildrenCount: 1 }),
            makeThread('parent-proposed', { activeChildrenCount: 1, codingAgentChangeState: { kind: 'proposed', requires_restart: false }}),
            makeThread('question', { status: 'waiting_for_user_answer', liveEventWaitCount: 1 }),
            makeThread('failed', { status: 'failed', activeChildrenCount: 1 }),
            makeThread('stopped', { isStoppedChild: true, waitingChildrenCount: 1 }),
            makeThread('proposed', { codingAgentChangeState: { kind: 'proposed', requires_restart: false }}),
            makeThread('paused', { status: 'paused', liveEventWaitCount: 1 }),
        ];
        const map = asMap(threads);
        const attention = new Set(ids(blockedThreads(map)));
        const review = new Set(ids(reviewThreads(map)));
        const inFlight = rootIds(inFlightThreads(map));
        for (const id of inFlight) {
            expect(attention.has(id), `${id} is in attention too`).toBe(false);
            expect(review.has(id), `${id} is in review too`).toBe(false);
        }
        expect(inFlight.sort()).toEqual(['parent', 'parked', 'parked-proposed', 'running', 'running-proposed']);
    });
});

describe('a thread that is both awaiting an answer and carrying a proposed change', () => {
    // The open question withholds Apply, so the change is not ready yet. It
    // returns to Review once the user answers and the turn settles.
    it('appears in attention only', () => {
        const both = makeThread('both', { status: 'waiting_for_user_answer', codingAgentChangeState: { kind: 'proposed', requires_restart: false }});
        expect(ids(blockedThreads(asMap([both])))).toEqual(['both']);
        expect(reviewThreads(asMap([both]))).toEqual([]);
    });
});

describe('blockedThreadCount mirrors blockedThreads', () => {
    // The badge and the filtered list share `threadIsBlocked`, so the count
    // must always equal the list length for the same threadMap.
    it('counts exactly the threads the list would render', () => {
        const threads = [
            makeThread('waiting', { status: 'waiting_for_user_answer' }),
            makeThread('failed', { status: 'failed' }),
            makeThread('proposed', { codingAgentChangeState: { kind: 'proposed', requires_restart: false }}),  // Review, not attention
            makeThread('running', { status: 'running' }),           // excluded
            makeThread('idle', { status: 'idle' }),                 // excluded
            makeThread('archived-fail', { section: 'archived', status: 'failed' }), // excluded
        ];
        threadMap.value = asMap(threads);
        expect(blockedThreadCount.value).toBe(2);
        expect(blockedThreadCount.value).toBe(blockedThreads(threadMap.value).length);
    });

    it('is zero when no thread is blocked', () => {
        threadMap.value = asMap([makeThread('a', { status: 'idle' })]);
        expect(blockedThreadCount.value).toBe(0);
    });
});

describe('the Review count', () => {
    it('counts exactly the threads the predicate takes', () => {
        const threads = [
            makeThread('proposed', { codingAgentChangeState: { kind: 'proposed', requires_restart: false }}),
            makeThread('archived-proposed', { section: 'archived', codingAgentChangeState: { kind: 'proposed', requires_restart: false }}),
            makeThread('running-proposed', { status: 'running', codingAgentChangeState: { kind: 'proposed', requires_restart: false }}), // excluded
            makeThread('parked-proposed', { codingAgentChangeState: { kind: 'proposed', requires_restart: false }, liveEventWaitCount: 1 }), // excluded
            makeThread('waiting', { status: 'waiting_for_user_answer' }),                     // attention, not review
            makeThread('idle', { status: 'idle' }),                                           // excluded
        ];
        threadMap.value = asMap(threads);
        expect(reviewThreads(threadMap.value)).toHaveLength(2);
    });

    it('is zero when nothing is ready to review', () => {
        threadMap.value = asMap([makeThread('a', { status: 'idle' })]);
        expect(reviewThreads(threadMap.value)).toHaveLength(0);
    });
});

describe('threadIsBlocked', () => {
    it('is true for an awaiting-answer thread and a failed thread', () => {
        expect(threadIsBlocked(makeThread('a', { status: 'waiting_for_user_answer' }))).toBe(true);
        expect(threadIsBlocked(makeThread('b', { status: 'failed' }))).toBe(true);
    });

    it('is false for a running thread', () => {
        expect(threadIsBlocked(makeThread('a', { status: 'running' }))).toBe(false);
    });

    it('is false for a proposed-only thread (that is Review)', () => {
        expect(threadIsBlocked(makeThread('a', { codingAgentChangeState: { kind: 'proposed', requires_restart: false }}))).toBe(false);
    });
});

describe('threadInReview', () => {
    it('is true for a thread with a change ready to apply', () => {
        expect(threadInReview(makeThread('a', { codingAgentChangeState: { kind: 'proposed', requires_restart: false }}))).toBe(true);
    });

    it('is false for a running thread whose change is not yet ready to apply', () => {
        expect(threadInReview(makeThread('a', { status: 'running', codingAgentChangeState: { kind: 'proposed', requires_restart: false }}))).toBe(false);
    });

    it('is false for a waiting/failed thread with no proposed change', () => {
        expect(threadInReview(makeThread('a', { status: 'waiting_for_user_answer' }))).toBe(false);
        expect(threadInReview(makeThread('b', { status: 'failed' }))).toBe(false);
    });
});

// A read request (ADR 0409): the thread's agent asked the user to read its
// reply. Review lists it once the turn has ended, and Blocked never does.
describe('a read request', () => {
    it('puts a settled thread in Review', () => {
        expect(threadInReview(makeThread('a', { readRequested: true }))).toBe(true);
        expect(ids(reviewThreads(asMap([makeThread('a', { readRequested: true })])))).toEqual(['a']);
    });

    it('waits for the turn to end, so a running or watching thread stays in flight', () => {
        const running = makeThread('running', { status: 'running', readRequested: true });
        const watching = makeThread('watching', { liveEventWaitCount: 1, readRequested: true });
        expect(threadInReview(running)).toBe(false);
        expect(threadInReview(watching)).toBe(false);
        expect(ids(inFlightThreads(asMap([running, watching])).map(n => n.thread))).toEqual(expect.arrayContaining(['running', 'watching']));
    });

    it('never makes a thread Blocked, or counts toward its badge', () => {
        const asked = makeThread('a', { readRequested: true });
        expect(threadIsBlocked(asked)).toBe(false);
        threadMap.value = asMap([asked]);
        expect(blockedThreadCount.value).toBe(0);
        threadMap.value = new Map();
    });

    it('leaves an archived thread out, since an archive dismisses the request', () => {
        expect(threadInReview(makeThread('a', { section: 'archived', readRequested: true }))).toBe(false);
    });
});
