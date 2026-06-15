/**
 * Tests for the needs-attention-view filter helper.
 *
 * The attention toggle in the threads header collapses the drawer to a single
 * "Needs attention" section that shows every Current/Saved thread where the user
 * must actively respond or fix something — awaiting an answer/permission
 * (waiting_for_user_answer), a failed turn, or a change ready to apply
 * (codingAgentProposed) the user must Apply or Discard. The view bypasses the
 * channel/trigger/repo filters and the lifecycle section grouping, ordered by
 * review tier (User Q / permission ahead of changes ready to apply / failed),
 * then most-recent-first within each tier. Mirrors the drafts-view filter; the
 * `threadNeedsAttention` predicate is shared with the filter-badge count
 * (`attentionThreadCount`) so the two can never disagree.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { attentionThreads } from './ThreadDrawer';
import { attentionThreadCount, threadNeedsAttention, threadMap } from '../../store/store';
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

    it('includes a Current thread with a change ready to apply', () => {
        // A change ready to apply still needs the user — they must Apply or
        // Discard it before the thread can settle — so it IS attention.
        const proposed = makeThread('a', { section: 'inbox', codingAgentProposed: true });
        expect(ids(attentionThreads(asMap([proposed])))).toEqual(['a']);
    });

    it('excludes a running thread (the system\'s turn)', () => {
        const running = makeThread('a', { section: 'inbox', status: 'running' });
        expect(attentionThreads(asMap([running]))).toEqual([]);
    });

    it('excludes a running thread that also carries a proposed change', () => {
        // A change proposed while a follow-up turn is still in flight is not yet
        // *ready* to apply — the WaitingBanner shows Cancel, not Apply — so it
        // must stay out of the attention view until the turn idles.
        const runningProposed = makeThread('a', { section: 'inbox', status: 'running', codingAgentProposed: true });
        expect(attentionThreads(asMap([runningProposed]))).toEqual([]);
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

    it('includes an archived thread that has a change ready to apply', () => {
        // A pending change lifts an archived thread back to Current (it still
        // needs an Apply/Discard), and a ready-to-apply change is attention, so
        // it surfaces in the attention view.
        const archivedProposed = makeThread('a', { section: 'archived', codingAgentProposed: true });
        expect(ids(attentionThreads(asMap([archivedProposed])))).toEqual(['a']);
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

    it('floats User Q / permission ahead of changes ready to apply', () => {
        // waiting_for_user_answer (tier 0 — the agent is stalled until the user
        // answers) sorts above a change ready to apply (tier 1), even when the
        // change is fresher. Recency only breaks ties within a tier.
        const proposed = makeThread('proposed', { codingAgentProposed: true, updatedAt: '2026-05-04T00:00:00Z' });
        const waiting = makeThread('waiting', { status: 'waiting_for_user_answer', updatedAt: '2026-05-01T00:00:00Z' });
        expect(ids(attentionThreads(asMap([proposed, waiting])))).toEqual(['waiting', 'proposed']);
    });

    it('orders by tier first, then recency within each tier', () => {
        const waitOld = makeThread('wait-old', { status: 'waiting_for_user_answer', updatedAt: '2026-05-01T00:00:00Z' });
        const waitNew = makeThread('wait-new', { status: 'waiting_for_user_answer', updatedAt: '2026-05-03T00:00:00Z' });
        const proposedNew = makeThread('proposed-new', { codingAgentProposed: true, updatedAt: '2026-05-05T00:00:00Z' });
        const failedOld = makeThread('failed-old', { status: 'failed', updatedAt: '2026-05-02T00:00:00Z' });
        // Tier 0 (waiting) ahead of tier 1 (proposed + failed); fresher first inside each.
        expect(ids(attentionThreads(asMap([failedOld, proposedNew, waitOld, waitNew]))))
            .toEqual(['wait-new', 'wait-old', 'proposed-new', 'failed-old']);
    });

    it('returns empty when nothing needs attention', () => {
        const a = makeThread('a', { status: 'idle' });
        const b = makeThread('b', { status: 'running' });
        expect(attentionThreads(asMap([a, b]))).toEqual([]);
    });
});

describe('attentionThreadCount mirrors attentionThreads', () => {
    // The filter badge and the filtered list share `threadNeedsAttention`, so
    // the count must always equal the list length for the same threadMap.
    it('counts exactly the threads the list would render', () => {
        const threads = [
            makeThread('waiting', { status: 'waiting_for_user_answer' }),
            makeThread('failed', { status: 'failed' }),
            makeThread('proposed', { codingAgentProposed: true }),  // change ready to apply
            makeThread('running', { status: 'running' }),           // excluded
            makeThread('idle', { status: 'idle' }),                 // excluded
            makeThread('archived-fail', { section: 'archived', status: 'failed' }), // excluded
        ];
        threadMap.value = asMap(threads);
        expect(attentionThreadCount.value).toBe(3);
        expect(attentionThreadCount.value).toBe(attentionThreads(threadMap.value).length);
    });

    it('is zero when no thread needs attention', () => {
        threadMap.value = asMap([makeThread('a', { status: 'idle' })]);
        expect(attentionThreadCount.value).toBe(0);
    });
});

describe('threadNeedsAttention', () => {
    it('is true for an awaiting-answer thread and false for a running one', () => {
        expect(threadNeedsAttention(makeThread('a', { status: 'waiting_for_user_answer' }))).toBe(true);
        expect(threadNeedsAttention(makeThread('b', { status: 'running' }))).toBe(false);
    });

    it('is true for a thread with a change ready to apply', () => {
        expect(threadNeedsAttention(makeThread('a', { codingAgentProposed: true }))).toBe(true);
    });

    it('is false for a running thread whose change is not yet ready to apply', () => {
        // Mid-turn is the agent's turn; the proposed change can\'t be applied
        // until the follow-up turn idles.
        expect(threadNeedsAttention(makeThread('a', { status: 'running', codingAgentProposed: true }))).toBe(false);
    });
});
