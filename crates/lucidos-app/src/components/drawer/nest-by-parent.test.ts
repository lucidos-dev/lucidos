/**
 * Tests for nestByParent — the helper that reorders a sorted thread list so
 * children appear immediately under their parent, indented by depth.
 *
 * Roots and siblings keep the input's relative order so the section sort still
 * drives the visible top-level sequence; only children get pulled out of their
 * original slot to attach under their parent.
 */

import { describe, it, expect } from 'vitest';
import { nestByParent } from './ThreadDrawer';
import type { ThreadState, ThreadMeta } from '../../store/thread-events';

function makeThread(id: string, parentId?: string): ThreadState {
    const meta: ThreadMeta = {
        id,
        title: id,
        channel: 'chat',
        initiator: 'user',
        saved: false,
        createdAt: '2026-04-12T00:00:00Z',
        updatedAt: '2026-04-12T00:00:00Z',
        status: 'idle',
        messageCount: 1,
        section: 'archived',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentHasDiff: false,
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        lastRevivedAt: '',
        parentThreadId: parentId,
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

function shape(nested: ReturnType<typeof nestByParent>): Array<[string, number]> {
    return nested.map(n => [n.thread.meta.id, n.depth] as [string, number]);
}

describe('nestByParent', () => {
    it('returns roots at depth 0 in input order when nothing has a parent', () => {
        const out = nestByParent([makeThread('a'), makeThread('b'), makeThread('c')]);
        expect(shape(out)).toEqual([['a', 0], ['b', 0], ['c', 0]]);
    });

    it('places a child immediately after its parent at depth 1', () => {
        // Input order: parent then unrelated then child. Child pulls up under parent.
        const out = nestByParent([
            makeThread('parent'),
            makeThread('other'),
            makeThread('child', 'parent'),
        ]);
        expect(shape(out)).toEqual([
            ['parent', 0],
            ['child', 1],
            ['other', 0],
        ]);
    });

    it('preserves sibling order from the input within a parent group', () => {
        const out = nestByParent([
            makeThread('parent'),
            makeThread('child-a', 'parent'),
            makeThread('child-b', 'parent'),
            makeThread('child-c', 'parent'),
        ]);
        expect(shape(out)).toEqual([
            ['parent', 0],
            ['child-a', 1],
            ['child-b', 1],
            ['child-c', 1],
        ]);
    });

    it('nests grandchildren under children, recursively', () => {
        const out = nestByParent([
            makeThread('root'),
            makeThread('mid', 'root'),
            makeThread('leaf', 'mid'),
        ]);
        expect(shape(out)).toEqual([
            ['root', 0],
            ['mid', 1],
            ['leaf', 2],
        ]);
    });

    it('renders an orphan child at root level when its parent is absent', () => {
        // The parent thread isn't in this section's slice (filtered, in
        // another section, or paginated out). The child must still render —
        // it just falls back to root level so it isn't silently hidden.
        const out = nestByParent([
            makeThread('orphan', 'missing-parent'),
            makeThread('standalone'),
        ]);
        expect(shape(out)).toEqual([
            ['orphan', 0],
            ['standalone', 0],
        ]);
    });

    it('handles two parents with their own subtrees interleaved in the input', () => {
        const out = nestByParent([
            makeThread('p1'),
            makeThread('p2'),
            makeThread('p1-child', 'p1'),
            makeThread('p2-child', 'p2'),
        ]);
        expect(shape(out)).toEqual([
            ['p1', 0],
            ['p1-child', 1],
            ['p2', 0],
            ['p2-child', 1],
        ]);
    });

    it('returns an empty list for an empty input', () => {
        expect(nestByParent([])).toEqual([]);
    });
});
