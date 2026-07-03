/**
 * Tests for toggleFocusedThreadFamily — the action behind the `toggleSubthreads`
 * keyboard shortcut. It collapses/expands the OPEN (focused) thread's own
 * sub-thread family, working from any pane (no drawer-focus gate). It always
 * acts on the focused thread itself — never climbing to a parent — and no-ops
 * when there is no focused thread or it has no sub-threads. Collapse state is
 * the same localStorage-backed `collapsedFamilies` set the disclosure chevron
 * and the ←/→ tree nav write, so we assert via the persisted set.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { toggleFocusedThreadFamily } from './ThreadDrawer';
import { focusedThreadId, threadMap } from '../../store/store';
import type { ThreadState, ThreadMeta } from '../../store/thread-events';

const COLLAPSED_FAMILIES_KEY = 'lucidos-drawer-collapsed-families';

function makeThread(id: string, totalChildrenCount: number): ThreadState {
    const meta: ThreadMeta = {
        id,
        title: id,
        channel: 'chat',
        initiator: 'user',
        saved: false,
        createdAt: '2026-06-30T00:00:00Z',
        updatedAt: '2026-06-30T00:00:00Z',
        status: 'idle',
        messageCount: 1,
        section: 'archived',
        activeChildrenCount: 0,
        totalChildrenCount,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentHasDiff: false,
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        lastRevivedAt: '',
        parentThreadId: undefined,
        state: 'active',
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

function persistedFamilies(): string[] {
    const raw = localStorage.getItem(COLLAPSED_FAMILIES_KEY);
    return raw ? (JSON.parse(raw) as string[]) : [];
}

describe('toggleFocusedThreadFamily', () => {
    beforeEach(() => {
        localStorage.clear();
        focusedThreadId.value = null;
        threadMap.value = new Map();
    });

    it("collapses then expands the focused thread's own family", () => {
        const id = 'parent-toggle';
        threadMap.value = new Map([[id, makeThread(id, 3)]]);
        focusedThreadId.value = id;

        toggleFocusedThreadFamily();
        expect(persistedFamilies()).toContain(id);

        toggleFocusedThreadFamily();
        expect(persistedFamilies()).not.toContain(id);
    });

    it('is a no-op when no thread is focused', () => {
        focusedThreadId.value = null;

        toggleFocusedThreadFamily();

        expect(persistedFamilies()).toEqual([]);
    });

    it('is a no-op when the focused thread has no sub-threads', () => {
        const id = 'leaf-thread';
        threadMap.value = new Map([[id, makeThread(id, 0)]]);
        focusedThreadId.value = id;

        toggleFocusedThreadFamily();

        expect(persistedFamilies()).not.toContain(id);
    });

    it('is a no-op when the focused thread is not in the thread map', () => {
        focusedThreadId.value = 'unknown-thread';

        toggleFocusedThreadFamily();

        expect(persistedFamilies()).not.toContain('unknown-thread');
    });
});
