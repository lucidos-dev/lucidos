/**
 * Every workspace has a home thread (ADR 0362, ADR 0411). The thread list
 * never draws it: the Home entries in the thread header and the Lucidos menu
 * open it instead.
 */

import { describe, it, expect } from 'vitest';
import { categorizeThreads } from './ThreadDrawer';
import { findHomeThread } from '../../store/actions/homeThread';
import type { ThreadState } from '../../store/thread-events';

function thread(id: string, home = false): ThreadState {
    return {
        meta: {
            id,
            title: id,
            channel: 'chat',
            initiator: 'user',
            saved: false,
            createdAt: '2026-04-12T00:00:00Z',
            updatedAt: '2026-04-12T00:00:00Z',
            status: 'idle',
            summaryVersion: 0,
            messageCount: 1,
            section: 'inbox',
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
            ...(home ? { home: true as const } : {}),
        },
        events: new Map(),
        streamingBuffer: '',
        eventsLoaded: false,
        eventsLoadFailed: false,
        lastDbSeq: 0,
        pendingUserMessages: [],
    };
}

describe('the home thread', () => {
    const home = thread('home', true);
    const other = thread('other');

    it('is found by its marker', () => {
        expect(findHomeThread([other, home])).toBe(home);
    });

    it('is found nowhere before the list carries it', () => {
        expect(findHomeThread([other])).toBeUndefined();
    });

    it('is in no section of the thread list', () => {
        const sections = categorizeThreads([other, home]);
        const inSections = [...sections.current, ...sections.saved, ...sections.archive]
            .map(t => t.meta.id);
        expect(inSections).toEqual(['other']);
    });
});
