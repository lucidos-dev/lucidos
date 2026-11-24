
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
        summaryVersion: 0,
        messageCount: 1,
        section: 'archived',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0,
        attentionDescendantCount: 0,
        codingAgentChangeState: { kind: 'none' },
        codingAgentIsExternalRepo: false,
        lastRevivedAt: '',
        state: 'active',
        latestTodoList: null,
        liveEventWaitCount: 0,
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

const createdEarlyTouchedLate = () => makeThread('early-touched-late', '2026-06-10T00:00:00Z', '2026-06-25T00:00:00Z');
const createdLateNeverTouched = () => makeThread('late-untouched', '2026-06-20T00:00:00Z', '2026-06-20T00:00:00Z');

function sections(list: ThreadState[]): { current: ThreadState[]; saved: ThreadState[]; archive: ThreadState[] } {
    const s: ThreadSections = {
        current: [...list],
        onHold: [...list],
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
        expect(ids(archive)).toEqual(['late-untouched', 'early-touched-late']);
    });

    it('orders Current by createdAt DESC (same axis as Archive)', () => {
        const { current } = sections([createdEarlyTouchedLate(), createdLateNeverTouched()]);
        expect(ids(current)).toEqual(['late-untouched', 'early-touched-late']);
    });

    it('keeps Saved on the family freshest lastUserAction (unchanged)', () => {
        const { saved } = sections([createdEarlyTouchedLate(), createdLateNeverTouched()]);
        expect(ids(saved)).toEqual(['early-touched-late', 'late-untouched']);
    });
});
