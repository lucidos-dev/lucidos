
import { describe, it, expect } from 'vitest';
import { hasCollapsedAncestor, computeFamilyGraph } from './ThreadDrawer';
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
        summaryVersion: 0,
        messageCount: 1,
        section: 'archived',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0, archivableSubThreadCount: 0,
        codingAgentChangeState: { kind: 'none' },
        codingAgentIsExternalRepo: false,
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

describe('hasCollapsedAncestor', () => {
    it('returns false when no ancestor is collapsed', () => {
        const root = makeThread('root');
        const child = makeThread('child', 'root');
        const graph = computeFamilyGraph([root, child]);

        expect(hasCollapsedAncestor('child', new Set(), graph)).toBe(false);
    });

    it('returns true when the direct parent is collapsed', () => {
        const root = makeThread('root');
        const child = makeThread('child', 'root');
        const graph = computeFamilyGraph([root, child]);

        expect(hasCollapsedAncestor('child', new Set(['root']), graph)).toBe(true);
    });

    it('returns true when a grandparent is collapsed', () => {
        const root = makeThread('root');
        const mid = makeThread('mid', 'root');
        const leaf = makeThread('leaf', 'mid');
        const graph = computeFamilyGraph([root, mid, leaf]);

        expect(hasCollapsedAncestor('leaf', new Set(['root']), graph)).toBe(true);
    });

    it('hides the deeper subtree when an intermediate parent is collapsed, but leaves siblings alone', () => {
        const root = makeThread('root');
        const mid = makeThread('mid', 'root');
        const leaf = makeThread('leaf', 'mid');
        const siblingOfMid = makeThread('sibling', 'root');
        const graph = computeFamilyGraph([root, mid, leaf, siblingOfMid]);

        expect(hasCollapsedAncestor('leaf', new Set(['mid']), graph)).toBe(true);
        expect(hasCollapsedAncestor('sibling', new Set(['mid']), graph)).toBe(false);
        expect(hasCollapsedAncestor('mid', new Set(['mid']), graph)).toBe(false);
    });

    it('returns false for the collapsed thread itself — only descendants hide', () => {
        const root = makeThread('root');
        const child = makeThread('child', 'root');
        const graph = computeFamilyGraph([root, child]);

        expect(hasCollapsedAncestor('root', new Set(['root']), graph)).toBe(false);
    });

    it('returns false for a root thread (no parent)', () => {
        const root = makeThread('root');
        const graph = computeFamilyGraph([root]);

        expect(hasCollapsedAncestor('root', new Set(['someone-else']), graph)).toBe(false);
    });

    it('returns false when the thread is not in the graph (paginated out / unknown)', () => {
        const root = makeThread('root');
        const graph = computeFamilyGraph([root]);

        expect(hasCollapsedAncestor('missing', new Set(['root']), graph)).toBe(false);
    });

    it('returns false on a parent cycle when nothing in the cycle is collapsed', () => {
        const a = makeThread('a', 'b');
        const b = makeThread('b', 'a');
        const graph = computeFamilyGraph([a, b]);

        expect(hasCollapsedAncestor('a', new Set(), graph)).toBe(false);
        expect(hasCollapsedAncestor('b', new Set(), graph)).toBe(false);
    });

    it('returns true on a parent cycle when a cycle member is collapsed', () => {
        const a = makeThread('a', 'b');
        const b = makeThread('b', 'a');
        const graph = computeFamilyGraph([a, b]);

        expect(hasCollapsedAncestor('b', new Set(['a']), graph)).toBe(true);
    });
});
