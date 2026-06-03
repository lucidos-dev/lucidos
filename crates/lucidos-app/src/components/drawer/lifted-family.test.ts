/**
 * Tests for computeFamilyDecorations — the helper that decides which families
 * are "lifted" (the family root's own natural section differs from the section
 * the family was routed to, because a descendant earned the lift). Drives two
 * coordinated drawer cues:
 *
 * - **Demoted parent**: the family root renders in its native-section style
 *   even while sitting in the lifted section, so a row that looks archived in
 *   Review reads as "I'm here under protest — the real work is one of my
 *   children."
 * - **Bright child rail**: descendants whose own natural section matches the
 *   routed section (i.e. they earned the lift) get the section-accent rail
 *   variant. Descendants dragged along but not responsible (lower-priority
 *   natural section) render with the default gray rail.
 *
 * When every family member naturally belongs to the routed section, neither
 * cue fires.
 */

import { describe, it, expect } from 'vitest';
import { computeFamilyGraph, computeFamilyDecorations } from './ThreadDrawer';
import type { ThreadState, ThreadMeta, ThreadStatus } from '../../store/thread-events';
import type { ArchiveState } from '../../generated/thread-lifecycle';

type ThreadOpts = {
    parentId?: string;
    section?: ArchiveState;
    status?: ThreadStatus;
    saved?: boolean;
    activeChildrenCount?: number;
    codingAgentProposed?: boolean;
};

function makeThread(id: string, opts: ThreadOpts = {}): ThreadState {
    const meta: ThreadMeta = {
        id,
        title: id,
        channel: 'chat',
        initiator: 'user',
        saved: opts.saved ?? false,
        createdAt: '2026-04-12T00:00:00Z',
        updatedAt: '2026-04-12T00:00:00Z',
        status: opts.status ?? 'idle',
        messageCount: 1,
        section: opts.section ?? 'inbox',
        activeChildrenCount: opts.activeChildrenCount ?? 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentHasDiff: false,
        codingAgentProposed: opts.codingAgentProposed ?? false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        lastRevivedAt: '',
        parentThreadId: opts.parentId,
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

const inReview = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, codingAgentProposed: true, section: 'inbox' });
const inActive = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, status: 'running' });
const inSaved = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, saved: true });
const inArchive = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, section: 'archived' });

describe('computeFamilyDecorations', () => {
    it('marks no family as lifted when parent and children share the same natural section', () => {
        // Two-thread family, both naturally in Active. No lift happened — the
        // family is in Active because both members are. Neither cue should fire.
        const parent = inActive('parent');
        const child = inActive('child', { parentId: 'parent' });
        const graph = computeFamilyGraph([parent, child]);
        const decorations = computeFamilyDecorations([parent, child], graph);

        expect(decorations.liftedRoots.size).toBe(0);
        expect(decorations.routedByThread.get('parent')).toBe('active');
        expect(decorations.routedByThread.get('child')).toBe('active');
    });

    it('marks the root as lifted when a child drags the family into a higher-priority section', () => {
        // Parent naturally archived, child active. Family routes to Active.
        // The root is lifted; the child is the one responsible.
        const parent = inArchive('parent');
        const child = inActive('child', { parentId: 'parent' });
        const graph = computeFamilyGraph([parent, child]);
        const decorations = computeFamilyDecorations([parent, child], graph);

        expect(decorations.liftedRoots.has('parent')).toBe(true);
        expect(decorations.routedByThread.get('parent')).toBe('active');
        expect(decorations.routedByThread.get('child')).toBe('active');
    });

    it('marks the root as lifted for a deeper chain when a grandchild earned the lift', () => {
        // grandparent archived, mid saved, leaf active → family in Active.
        const grandparent = inArchive('grandparent');
        const mid = inSaved('mid', { parentId: 'grandparent' });
        const leaf = inActive('leaf', { parentId: 'mid' });
        const graph = computeFamilyGraph([grandparent, mid, leaf]);
        const decorations = computeFamilyDecorations([grandparent, mid, leaf], graph);

        expect(decorations.liftedRoots.has('grandparent')).toBe(true);
        expect(decorations.routedByThread.get('grandparent')).toBe('active');
        expect(decorations.routedByThread.get('mid')).toBe('active');
        expect(decorations.routedByThread.get('leaf')).toBe('active');
    });

    it('marks separate families independently', () => {
        const liftedRoot = inArchive('lifted-root');
        const liftingChild = inActive('lifting-child', { parentId: 'lifted-root' });
        const naturalRoot = inActive('natural-root');
        const naturalChild = inActive('natural-child', { parentId: 'natural-root' });
        const threads = [liftedRoot, liftingChild, naturalRoot, naturalChild];
        const graph = computeFamilyGraph(threads);
        const decorations = computeFamilyDecorations(threads, graph);

        expect(decorations.liftedRoots.has('lifted-root')).toBe(true);
        expect(decorations.liftedRoots.has('natural-root')).toBe(false);
    });

    it('marks an orphan child as its own (un-lifted) family when the parent is paginated out', () => {
        // Orphan child has no anchor — it becomes its own family root. Its
        // natural section IS the routed section (because the family is just
        // itself), so it isn't lifted.
        const orphan = inReview('orphan', { parentId: 'missing-parent' });
        const graph = computeFamilyGraph([orphan]);
        const decorations = computeFamilyDecorations([orphan], graph);

        expect(decorations.liftedRoots.size).toBe(0);
        expect(decorations.routedByThread.get('orphan')).toBe('review');
    });

    it('excludes composing and discarded threads from the routing map', () => {
        const parent = inActive('parent');
        const composing = inReview('composing-child', { parentId: 'parent' });
        composing.meta.state = 'composing';
        const discarded = inReview('discarded-child', { parentId: 'parent' });
        discarded.meta.state = 'discarded';
        const graph = computeFamilyGraph([parent, composing, discarded]);
        const decorations = computeFamilyDecorations([parent, composing, discarded], graph);

        // Excluded threads don't get a routing entry and don't influence the lift.
        expect(decorations.routedByThread.has('composing-child')).toBe(false);
        expect(decorations.routedByThread.has('discarded-child')).toBe(false);
        expect(decorations.liftedRoots.size).toBe(0);
        expect(decorations.routedByThread.get('parent')).toBe('active');
    });

    it('routes a saved-parent + review-child family to Saved without a lift', () => {
        // Saved root pins the family to Saved — root's natural section
        // matches the routed one, so no lift cue fires.
        const parent = inSaved('parent');
        const child = inReview('child', { parentId: 'parent' });
        const graph = computeFamilyGraph([parent, child]);
        const decorations = computeFamilyDecorations([parent, child], graph);

        expect(decorations.liftedRoots.size).toBe(0);
        expect(decorations.routedByThread.get('parent')).toBe('saved');
        expect(decorations.routedByThread.get('child')).toBe('saved');
    });
});
