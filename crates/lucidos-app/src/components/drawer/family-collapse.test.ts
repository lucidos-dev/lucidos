/**
 * Tests for hasCollapsedAncestor — the predicate that decides whether a thread
 * should be hidden because one of its ancestors is in the collapsedFamilies
 * set. Pairs with nestByParent: after nesting produces the flat depth-annotated
 * list, this filter walks each row's parent chain and drops rows whose ancestor
 * was collapsed by the user. Each parent-with-children has its own independent
 * chevron, so the collapse state composes — collapsing A hides B and C;
 * re-expanding A preserves B's own collapse state because B's filter check is
 * independent of A's chevron.
 */

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

        // Collapsing the root hides the whole subtree, including grandchildren.
        expect(hasCollapsedAncestor('leaf', new Set(['root']), graph)).toBe(true);
    });

    it('hides the deeper subtree when an intermediate parent is collapsed, but leaves siblings alone', () => {
        const root = makeThread('root');
        const mid = makeThread('mid', 'root');
        const leaf = makeThread('leaf', 'mid');
        const siblingOfMid = makeThread('sibling', 'root');
        const graph = computeFamilyGraph([root, mid, leaf, siblingOfMid]);

        // Collapsing `mid` hides `leaf` (its child) but leaves `sibling` (mid's
        // peer) visible. Each parent has an independent chevron.
        expect(hasCollapsedAncestor('leaf', new Set(['mid']), graph)).toBe(true);
        expect(hasCollapsedAncestor('sibling', new Set(['mid']), graph)).toBe(false);
        expect(hasCollapsedAncestor('mid', new Set(['mid']), graph)).toBe(false);
    });

    it('returns false for the collapsed thread itself — only descendants hide', () => {
        const root = makeThread('root');
        const child = makeThread('child', 'root');
        const graph = computeFamilyGraph([root, child]);

        // The parent row stays visible; its chevron lets the user re-expand.
        expect(hasCollapsedAncestor('root', new Set(['root']), graph)).toBe(false);
    });

    it('returns false for a root thread (no parent)', () => {
        const root = makeThread('root');
        const graph = computeFamilyGraph([root]);

        expect(hasCollapsedAncestor('root', new Set(['someone-else']), graph)).toBe(false);
    });

    it('returns false when the thread is not in the graph (paginated out / unknown)', () => {
        // Defensive: a render between batches may briefly hand us an id whose
        // thread is no longer in the graph. Don't blow up — just don't hide.
        const root = makeThread('root');
        const graph = computeFamilyGraph([root]);

        expect(hasCollapsedAncestor('missing', new Set(['root']), graph)).toBe(false);
    });

    it('returns false on a parent cycle when nothing in the cycle is collapsed', () => {
        // Data corruption: a → b → a. Walking the parent chain must terminate
        // and return false rather than looping forever.
        const a = makeThread('a', 'b');
        const b = makeThread('b', 'a');
        const graph = computeFamilyGraph([a, b]);

        expect(hasCollapsedAncestor('a', new Set(), graph)).toBe(false);
        expect(hasCollapsedAncestor('b', new Set(), graph)).toBe(false);
    });

    it('returns true on a parent cycle when a cycle member is collapsed', () => {
        // Same cycle, but `a` is collapsed. `b`'s parent chain hits `a` on
        // the first hop and the predicate returns true.
        const a = makeThread('a', 'b');
        const b = makeThread('b', 'a');
        const graph = computeFamilyGraph([a, b]);

        expect(hasCollapsedAncestor('b', new Set(['a']), graph)).toBe(true);
    });
});
