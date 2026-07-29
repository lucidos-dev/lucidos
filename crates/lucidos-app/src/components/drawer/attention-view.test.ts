/**
 * Tests for the needs-attention, review, and running view filter helpers.
 *
 * The drawer view selector splits threads into flat single-section views:
 *   - "Needs attention" — every Current/Saved thread where the agent is stuck
 *     waiting on the user: awaiting an answer/permission (waiting_for_user_answer)
 *     or a failed turn. Ordered by review tier (User Q / permission ahead of a
 *     failed turn), then most-recent-first within each tier.
 *   - "Review" — every Current/Saved thread carrying a change ready to apply
 *     (codingAgentProposed, not mid-turn). Most-recent-first.
 *   - "Running" — every Current/Saved thread actively working on a response
 *     (effective status `running`). Most-recent-first.
 * All views bypass the channel/trigger/repo filters and the lifecycle section
 * grouping. The predicates (`threadNeedsAttention` / `threadInReview` /
 * `threadIsRunning`) are shared with the selector badge counts
 * (`attentionThreadCount` / `reviewThreadCount` / `runningThreadCount`) so the
 * counts and the filtered lists can never disagree. The needs-attention and
 * review predicates are independent: a thread that is BOTH awaiting an answer AND
 * carrying a proposed change legitimately surfaces in both views. Running is
 * mutually exclusive with both (they exclude `running`).
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { attentionThreads, reviewThreads, runningThreads } from './ThreadDrawer';
import { attentionThreadCount, reviewThreadCount, runningThreadCount, threadNeedsAttention, threadInReview, threadIsRunning, threadMap } from '../../store/store';
import type { ThreadState, ThreadMeta, ThreadStatus } from '../../store/thread-events';
import type { ArchiveState } from '../../generated/thread-lifecycle';

type ThreadOpts = {
    section?: ArchiveState;
    status?: ThreadStatus;
    saved?: boolean;
    codingAgentProposed?: boolean;
    state?: ThreadMeta['state'];
    updatedAt?: string;
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
        messageCount: 1,
        section: opts.section ?? 'inbox',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0,
        attentionDescendantCount: 0,
        codingAgentHasDiff: false,
        codingAgentProposed: opts.codingAgentProposed ?? false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        lastRevivedAt: '',
        state: opts.state ?? 'active',
        latestTodoList: null,
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

beforeEach(() => {
    threadMap.value = new Map();
});

describe('attentionThreads', () => {
    it('includes a Current thread awaiting a user answer', () => {
        const waiting = makeThread('a', { section: 'inbox', status: 'waiting_for_user_answer' });
        expect(ids(attentionThreads(asMap([waiting])))).toEqual(['a']);
    });

    it('includes a Current thread whose last turn failed', () => {
        const failed = makeThread('a', { section: 'inbox', status: 'failed' });
        expect(ids(attentionThreads(asMap([failed])))).toEqual(['a']);
    });

    it('includes a Saved thread that needs attention', () => {
        // A saved thread routes to Saved; an awaiting-answer state still demands
        // attention there.
        const saved = makeThread('a', { saved: true, status: 'waiting_for_user_answer' });
        expect(ids(attentionThreads(asMap([saved])))).toEqual(['a']);
    });

    it('excludes a proposed-only thread (that is Review, not attention)', () => {
        // A change merely ready to apply is the Review view\'s job now; with no
        // waiting/failed state the agent is not stuck on the user, so it is NOT
        // needs-attention.
        const proposed = makeThread('a', { section: 'inbox', codingAgentProposed: true });
        expect(attentionThreads(asMap([proposed]))).toEqual([]);
    });

    it('excludes a running thread (the system\'s turn)', () => {
        const running = makeThread('a', { section: 'inbox', status: 'running' });
        expect(attentionThreads(asMap([running]))).toEqual([]);
    });

    it('excludes an idle thread with nothing pending', () => {
        const idle = makeThread('a', { section: 'inbox', status: 'idle' });
        expect(attentionThreads(asMap([idle]))).toEqual([]);
    });

    it('excludes an acknowledged (archived) failed thread', () => {
        // Archived = the user acknowledged the failure; it leaves the Current
        // section, so it must not resurface in the attention view.
        const archivedFail = makeThread('a', { section: 'archived', status: 'failed' });
        expect(attentionThreads(asMap([archivedFail]))).toEqual([]);
    });

    it('excludes composing and discarded threads', () => {
        // Composing carries an otherwise-attention status to prove the
        // composing-state exclusion overrides it.
        const composing = makeThread('a', { state: 'composing', status: 'waiting_for_user_answer' });
        const discarded = makeThread('b', { state: 'discarded', status: 'failed' });
        expect(attentionThreads(asMap([composing, discarded]))).toEqual([]);
    });

    it('sorts most-recent-first within a tier', () => {
        const old = makeThread('old', { status: 'failed', updatedAt: '2026-05-01T00:00:00Z' });
        const fresh = makeThread('fresh', { status: 'failed', updatedAt: '2026-05-04T00:00:00Z' });
        const mid = makeThread('mid', { status: 'failed', updatedAt: '2026-05-02T00:00:00Z' });
        expect(ids(attentionThreads(asMap([old, fresh, mid])))).toEqual(['fresh', 'mid', 'old']);
    });

    it('floats User Q / permission ahead of a failed turn', () => {
        // waiting_for_user_answer (tier 0 — the agent is stalled until the user
        // answers) sorts above a failed turn (tier 1), even when the failure is
        // fresher. Recency only breaks ties within a tier.
        const failed = makeThread('failed', { status: 'failed', updatedAt: '2026-05-04T00:00:00Z' });
        const waiting = makeThread('waiting', { status: 'waiting_for_user_answer', updatedAt: '2026-05-01T00:00:00Z' });
        expect(ids(attentionThreads(asMap([failed, waiting])))).toEqual(['waiting', 'failed']);
    });

    it('orders by tier first, then recency within each tier', () => {
        const waitOld = makeThread('wait-old', { status: 'waiting_for_user_answer', updatedAt: '2026-05-01T00:00:00Z' });
        const waitNew = makeThread('wait-new', { status: 'waiting_for_user_answer', updatedAt: '2026-05-03T00:00:00Z' });
        const failedNew = makeThread('failed-new', { status: 'failed', updatedAt: '2026-05-05T00:00:00Z' });
        const failedOld = makeThread('failed-old', { status: 'failed', updatedAt: '2026-05-02T00:00:00Z' });
        // Tier 0 (waiting) ahead of tier 1 (failed); fresher first inside each.
        expect(ids(attentionThreads(asMap([failedOld, failedNew, waitOld, waitNew]))))
            .toEqual(['wait-new', 'wait-old', 'failed-new', 'failed-old']);
    });

    it('returns empty when nothing needs attention', () => {
        const a = makeThread('a', { status: 'idle' });
        const b = makeThread('b', { status: 'running' });
        expect(attentionThreads(asMap([a, b]))).toEqual([]);
    });
});

describe('reviewThreads', () => {
    it('includes a Current thread with a change ready to apply', () => {
        const proposed = makeThread('a', { section: 'inbox', codingAgentProposed: true });
        expect(ids(reviewThreads(asMap([proposed])))).toEqual(['a']);
    });

    it('includes a Saved thread with a change ready to apply', () => {
        const saved = makeThread('a', { saved: true, codingAgentProposed: true });
        expect(ids(reviewThreads(asMap([saved])))).toEqual(['a']);
    });

    it('includes an archived thread that has a change ready to apply', () => {
        // A pending change lifts an archived thread back to Current (it still
        // needs an Apply/Discard), so it surfaces in the review view.
        const archivedProposed = makeThread('a', { section: 'archived', codingAgentProposed: true });
        expect(ids(reviewThreads(asMap([archivedProposed])))).toEqual(['a']);
    });

    it('excludes a running thread whose change is not yet ready to apply', () => {
        // A change proposed while a follow-up turn is still in flight is not yet
        // *ready* to apply — the WaitingBanner shows Cancel, not Apply — so it
        // must stay out of the review view until the turn idles.
        const runningProposed = makeThread('a', { section: 'inbox', status: 'running', codingAgentProposed: true });
        expect(reviewThreads(asMap([runningProposed]))).toEqual([]);
    });

    it('excludes a waiting/failed thread with no proposed change (that is attention)', () => {
        const waiting = makeThread('a', { status: 'waiting_for_user_answer' });
        const failed = makeThread('b', { status: 'failed' });
        expect(reviewThreads(asMap([waiting, failed]))).toEqual([]);
    });

    it('excludes composing and discarded threads', () => {
        const composing = makeThread('a', { state: 'composing', codingAgentProposed: true });
        const discarded = makeThread('b', { state: 'discarded', codingAgentProposed: true });
        expect(reviewThreads(asMap([composing, discarded]))).toEqual([]);
    });

    it('sorts most-recent-first', () => {
        const old = makeThread('old', { codingAgentProposed: true, updatedAt: '2026-05-01T00:00:00Z' });
        const fresh = makeThread('fresh', { codingAgentProposed: true, updatedAt: '2026-05-04T00:00:00Z' });
        const mid = makeThread('mid', { codingAgentProposed: true, updatedAt: '2026-05-02T00:00:00Z' });
        expect(ids(reviewThreads(asMap([old, fresh, mid])))).toEqual(['fresh', 'mid', 'old']);
    });
});

describe('runningThreads', () => {
    it('includes a Current thread actively working', () => {
        const running = makeThread('a', { section: 'inbox', status: 'running' });
        expect(ids(runningThreads(asMap([running])))).toEqual(['a']);
    });

    it('includes a Saved thread actively working', () => {
        const saved = makeThread('a', { saved: true, status: 'running' });
        expect(ids(runningThreads(asMap([saved])))).toEqual(['a']);
    });

    it('includes a thread that is running with a proposed change', () => {
        // A running thread routes to Current regardless of a pending change; it
        // belongs in Running (its change is not yet ready to apply, so it is NOT
        // in Review until the turn idles).
        const runningProposed = makeThread('a', { section: 'inbox', status: 'running', codingAgentProposed: true });
        expect(ids(runningThreads(asMap([runningProposed])))).toEqual(['a']);
    });

    it('excludes idle / waiting / failed threads', () => {
        const idle = makeThread('a', { status: 'idle' });
        const waiting = makeThread('b', { status: 'waiting_for_user_answer' });
        const failed = makeThread('c', { status: 'failed' });
        expect(runningThreads(asMap([idle, waiting, failed]))).toEqual([]);
    });

    it('excludes composing and discarded threads', () => {
        const composing = makeThread('a', { state: 'composing', status: 'running' });
        const discarded = makeThread('b', { state: 'discarded', status: 'running' });
        expect(runningThreads(asMap([composing, discarded]))).toEqual([]);
    });

    it('sorts most-recent-first', () => {
        const old = makeThread('old', { status: 'running', updatedAt: '2026-05-01T00:00:00Z' });
        const fresh = makeThread('fresh', { status: 'running', updatedAt: '2026-05-04T00:00:00Z' });
        const mid = makeThread('mid', { status: 'running', updatedAt: '2026-05-02T00:00:00Z' });
        expect(ids(runningThreads(asMap([old, fresh, mid])))).toEqual(['fresh', 'mid', 'old']);
    });
});

describe('runningThreadCount mirrors runningThreads', () => {
    it('counts exactly the threads the list would render', () => {
        const threads = [
            makeThread('running-1', { status: 'running' }),
            makeThread('running-2', { status: 'running', codingAgentProposed: true }),
            makeThread('waiting', { status: 'waiting_for_user_answer' }), // attention, not running
            makeThread('proposed', { codingAgentProposed: true }),        // review, not running
            makeThread('idle', { status: 'idle' }),                       // excluded
        ];
        threadMap.value = asMap(threads);
        expect(runningThreadCount.value).toBe(2);
        expect(runningThreadCount.value).toBe(runningThreads(threadMap.value).length);
    });

    it('is zero when nothing is running', () => {
        threadMap.value = asMap([makeThread('a', { status: 'idle' })]);
        expect(runningThreadCount.value).toBe(0);
    });
});

describe('threadIsRunning', () => {
    it('is true for a running thread', () => {
        expect(threadIsRunning(makeThread('a', { status: 'running' }))).toBe(true);
    });

    it('is false for idle / waiting / failed threads', () => {
        expect(threadIsRunning(makeThread('a', { status: 'idle' }))).toBe(false);
        expect(threadIsRunning(makeThread('b', { status: 'waiting_for_user_answer' }))).toBe(false);
        expect(threadIsRunning(makeThread('c', { status: 'failed' }))).toBe(false);
    });

    it('is false for a composing thread even when its status is running', () => {
        expect(threadIsRunning(makeThread('a', { state: 'composing', status: 'running' }))).toBe(false);
    });
});

describe('a thread that is both awaiting an answer and carrying a proposed change', () => {
    // The two views are independent surfaces — such a thread legitimately appears
    // in both, counted once per view.
    it('appears in both attention and review', () => {
        const both = makeThread('both', { status: 'waiting_for_user_answer', codingAgentProposed: true });
        expect(ids(attentionThreads(asMap([both])))).toEqual(['both']);
        expect(ids(reviewThreads(asMap([both])))).toEqual(['both']);
    });
});

describe('attentionThreadCount mirrors attentionThreads', () => {
    // The badge and the filtered list share `threadNeedsAttention`, so the count
    // must always equal the list length for the same threadMap.
    it('counts exactly the threads the list would render', () => {
        const threads = [
            makeThread('waiting', { status: 'waiting_for_user_answer' }),
            makeThread('failed', { status: 'failed' }),
            makeThread('proposed', { codingAgentProposed: true }),  // Review, not attention
            makeThread('running', { status: 'running' }),           // excluded
            makeThread('idle', { status: 'idle' }),                 // excluded
            makeThread('archived-fail', { section: 'archived', status: 'failed' }), // excluded
        ];
        threadMap.value = asMap(threads);
        expect(attentionThreadCount.value).toBe(2);
        expect(attentionThreadCount.value).toBe(attentionThreads(threadMap.value).length);
    });

    it('is zero when no thread needs attention', () => {
        threadMap.value = asMap([makeThread('a', { status: 'idle' })]);
        expect(attentionThreadCount.value).toBe(0);
    });
});

describe('reviewThreadCount mirrors reviewThreads', () => {
    it('counts exactly the threads the list would render', () => {
        const threads = [
            makeThread('proposed', { codingAgentProposed: true }),
            makeThread('archived-proposed', { section: 'archived', codingAgentProposed: true }),
            makeThread('running-proposed', { status: 'running', codingAgentProposed: true }), // excluded
            makeThread('waiting', { status: 'waiting_for_user_answer' }),                     // attention, not review
            makeThread('idle', { status: 'idle' }),                                           // excluded
        ];
        threadMap.value = asMap(threads);
        expect(reviewThreadCount.value).toBe(2);
        expect(reviewThreadCount.value).toBe(reviewThreads(threadMap.value).length);
    });

    it('is zero when nothing is ready to review', () => {
        threadMap.value = asMap([makeThread('a', { status: 'idle' })]);
        expect(reviewThreadCount.value).toBe(0);
    });
});

describe('threadNeedsAttention', () => {
    it('is true for an awaiting-answer thread and a failed thread', () => {
        expect(threadNeedsAttention(makeThread('a', { status: 'waiting_for_user_answer' }))).toBe(true);
        expect(threadNeedsAttention(makeThread('b', { status: 'failed' }))).toBe(true);
    });

    it('is false for a running thread', () => {
        expect(threadNeedsAttention(makeThread('a', { status: 'running' }))).toBe(false);
    });

    it('is false for a proposed-only thread (that is Review)', () => {
        expect(threadNeedsAttention(makeThread('a', { codingAgentProposed: true }))).toBe(false);
    });
});

describe('threadInReview', () => {
    it('is true for a thread with a change ready to apply', () => {
        expect(threadInReview(makeThread('a', { codingAgentProposed: true }))).toBe(true);
    });

    it('is false for a running thread whose change is not yet ready to apply', () => {
        expect(threadInReview(makeThread('a', { status: 'running', codingAgentProposed: true }))).toBe(false);
    });

    it('is false for a waiting/failed thread with no proposed change', () => {
        expect(threadInReview(makeThread('a', { status: 'waiting_for_user_answer' }))).toBe(false);
        expect(threadInReview(makeThread('b', { status: 'failed' }))).toBe(false);
    });
});
