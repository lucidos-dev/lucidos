/**
 * Tests for `sortDrawerSections` — the drawer's per-section display sort.
 *
 * The load-bearing case: `createdAt` and `lastUserAction` DIVERGE (a thread
 * created earlier but user-touched later, vs one created later but never touched
 * again). Current + Archive must order by `createdAt DESC` (matching the date
 * each row displays and the axis `loadOlderThreads` pages the Archive by), while
 * Saved keeps the family's freshest `lastUserAction`. This is the regression
 * guard for "archived threads shown out of order" — Archive used to sort by
 * `lastUserAction`, which is clustered at the migration default for never-touched
 * trigger threads.
 */

import { describe, it, expect } from 'vitest';
import { sortDrawerSections, computeFamilyKeys } from './family-graph';
import type { ThreadSections } from './family-graph';
import type { ThreadState, ThreadMeta } from '../../store/thread-events';

function makeThread(id: string, createdAt: string, lastUserAction: string): ThreadState {
    const meta: ThreadMeta = {
        id,
        title: id,
        channel: 'chat',
        initiator: 'user',
        saved: false,
        createdAt,
        updatedAt: lastUserAction,
        lastUserAction,
        status: 'idle',
        messageCount: 1,
        section: 'archived',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0,
        attentionDescendantCount: 0,
        codingAgentHasDiff: false,
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        lastRevivedAt: '',
        state: 'active',
        latestTodoList: null,
    liveEventWaits: [],
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

const ids = (ts: ThreadState[]) => ts.map(t => t.meta.id);

// createdEarlyTouchedLate: created first, but the user came back to it later.
// createdLateNeverTouched: created later, last_user_action == creation (a trigger
// thread the user never replied to). By creation order, "late" is newest.
const createdEarlyTouchedLate = () => makeThread('early-touched-late', '2026-06-10T00:00:00Z', '2026-06-25T00:00:00Z');
const createdLateNeverTouched = () => makeThread('late-untouched', '2026-06-20T00:00:00Z', '2026-06-20T00:00:00Z');

function sections(list: ThreadState[]): { current: ThreadState[]; saved: ThreadState[]; archive: ThreadState[] } {
    const s: ThreadSections = {
        current: [...list],
        saved: [...list],
        archive: [...list],
        statusMap: new Map(),
    };
    sortDrawerSections(s, computeFamilyKeys(list));
    return s;
}

describe('sortDrawerSections', () => {
    it('orders Archive by createdAt DESC even when lastUserAction diverges', () => {
        const { archive } = sections([createdEarlyTouchedLate(), createdLateNeverTouched()]);
        // Newest-created first. The OLD behavior (byFamilyRecent / lastUserAction)
        // would put 'early-touched-late' first because its user action is newer.
        expect(ids(archive)).toEqual(['late-untouched', 'early-touched-late']);
    });

    it('orders Current by createdAt DESC (same axis as Archive)', () => {
        const { current } = sections([createdEarlyTouchedLate(), createdLateNeverTouched()]);
        expect(ids(current)).toEqual(['late-untouched', 'early-touched-late']);
    });

    it('keeps Saved on the family freshest lastUserAction (unchanged)', () => {
        const { saved } = sections([createdEarlyTouchedLate(), createdLateNeverTouched()]);
        // Saved bubbles to the latest USER touch, so the early-but-recently-touched
        // thread sorts above the late-but-untouched one — the inverse of Archive.
        expect(ids(saved)).toEqual(['early-touched-late', 'late-untouched']);
    });
});
