/**
 * Tests for computeFamilyDecorations — the helper that decides which families
 * are "lifted" (the family root's own natural section differs from the section
 * the family was routed to, because a descendant earned the lift). Drives two
 * coordinated drawer cues:
 *
 * - **Demoted parent**: the family root renders in its native-section style
 *   even while sitting in the lifted section, so a row that looks archived in
 *   Current reads as "I'm here under protest — the real work is one of my
 *   children."
 * - **Bright child rail**: descendants whose own natural section matches the
 *   routed section (i.e. they earned the lift) get the section-accent rail
 *   variant. Descendants dragged along but not responsible (lower-priority
 *   natural section) render with the default gray rail.
 *
 * When every family member naturally belongs to the routed section, neither
 * cue fires.
 *
 * It also drives a third, independent cue that does NOT require a lift:
 *
 * - **Archived sub-thread**: a non-root family member whose own natural
 *   section is Archive while the family renders in a live section (Current /
 *   Pinned). The drawer renders it disabled so a child the user already
 *   archived can't read as pending work under a live parent. The family root
 *   is excluded because the demoted-parent cue already owns that row.
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

const withCta = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, codingAgentProposed: true, section: 'inbox' });
const running = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, status: 'running' });
const inSaved = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, saved: true });
const inArchive = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, section: 'archived' });

describe('computeFamilyDecorations', () => {
    it('marks no family as lifted when parent and children share the same natural section', () => {
        // Two-thread family, both naturally in Current. No lift happened — the
        // family is in Current because both members are. Neither cue should fire.
        const parent = running('parent');
        const child = running('child', { parentId: 'parent' });
        const graph = computeFamilyGraph([parent, child]);
        const decorations = computeFamilyDecorations([parent, child], graph);

        expect(decorations.liftedRoots.size).toBe(0);
        expect(decorations.routedByThread.get('parent')).toBe('current');
        expect(decorations.routedByThread.get('child')).toBe('current');
    });

    it('marks the root as lifted when a child drags the family into a higher-priority section', () => {
        // Parent naturally archived, child running. Family routes to Current.
        // The root is lifted; the child is the one responsible.
        const parent = inArchive('parent');
        const child = running('child', { parentId: 'parent' });
        const graph = computeFamilyGraph([parent, child]);
        const decorations = computeFamilyDecorations([parent, child], graph);

        expect(decorations.liftedRoots.has('parent')).toBe(true);
        expect(decorations.routedByThread.get('parent')).toBe('current');
        expect(decorations.routedByThread.get('child')).toBe('current');
    });

    it('marks the root as lifted for a deeper chain when a grandchild earned the lift', () => {
        // grandparent archived, mid saved, leaf running → family in Current.
        const grandparent = inArchive('grandparent');
        const mid = inSaved('mid', { parentId: 'grandparent' });
        const leaf = running('leaf', { parentId: 'mid' });
        const graph = computeFamilyGraph([grandparent, mid, leaf]);
        const decorations = computeFamilyDecorations([grandparent, mid, leaf], graph);

        expect(decorations.liftedRoots.has('grandparent')).toBe(true);
        expect(decorations.routedByThread.get('grandparent')).toBe('current');
        expect(decorations.routedByThread.get('mid')).toBe('current');
        expect(decorations.routedByThread.get('leaf')).toBe('current');
    });

    it('marks separate families independently', () => {
        const liftedRoot = inArchive('lifted-root');
        const liftingChild = running('lifting-child', { parentId: 'lifted-root' });
        const naturalRoot = running('natural-root');
        const naturalChild = running('natural-child', { parentId: 'natural-root' });
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
        const orphan = withCta('orphan', { parentId: 'missing-parent' });
        const graph = computeFamilyGraph([orphan]);
        const decorations = computeFamilyDecorations([orphan], graph);

        expect(decorations.liftedRoots.size).toBe(0);
        expect(decorations.routedByThread.get('orphan')).toBe('current');
    });

    it('excludes composing and discarded threads from the routing map', () => {
        const parent = running('parent');
        const composing = withCta('composing-child', { parentId: 'parent' });
        composing.meta.state = 'composing';
        const discarded = withCta('discarded-child', { parentId: 'parent' });
        discarded.meta.state = 'discarded';
        const graph = computeFamilyGraph([parent, composing, discarded]);
        const decorations = computeFamilyDecorations([parent, composing, discarded], graph);

        // Excluded threads don't get a routing entry and don't influence the lift.
        expect(decorations.routedByThread.has('composing-child')).toBe(false);
        expect(decorations.routedByThread.has('discarded-child')).toBe(false);
        expect(decorations.liftedRoots.size).toBe(0);
        expect(decorations.routedByThread.get('parent')).toBe('current');
    });

    it('marks an archived child of a running parent as an archived sub-thread', () => {
        // The reported case: the parent is genuinely in Current, the child was
        // archived, and the child still renders under the parent because the
        // family routes as one unit. Nothing is lifted here, so the disabled
        // cue has to be independent of the lift.
        const parent = running('parent');
        const child = inArchive('child', { parentId: 'parent' });
        const graph = computeFamilyGraph([parent, child]);
        const decorations = computeFamilyDecorations([parent, child], graph);

        expect(decorations.liftedRoots.size).toBe(0);
        expect(decorations.routedByThread.get('child')).toBe('current');
        expect([...decorations.archivedSubThreads]).toEqual(['child']);
    });

    it('marks an archived grandchild under a pinned family', () => {
        // A saved root pins the family to Saved; Pinned is a live section too,
        // so an archived descendant at any depth still reads as put away.
        const root = inSaved('root');
        const mid = running('mid', { parentId: 'root' });
        const leaf = inArchive('leaf', { parentId: 'mid' });
        const threads = [root, mid, leaf];
        const graph = computeFamilyGraph(threads);
        const decorations = computeFamilyDecorations(threads, graph);

        expect(decorations.routedByThread.get('leaf')).toBe('saved');
        expect([...decorations.archivedSubThreads]).toEqual(['leaf']);
    });

    it('leaves the lifted root out of the archived set, so its demoted-parent cue owns the row', () => {
        // Archived root + running child. The root IS archived-in-a-live-section,
        // but it's already marked lifted; double-cueing the same row would just
        // make the two styles fight.
        const parent = inArchive('parent');
        const child = running('child', { parentId: 'parent' });
        const graph = computeFamilyGraph([parent, child]);
        const decorations = computeFamilyDecorations([parent, child], graph);

        expect(decorations.liftedRoots.has('parent')).toBe(true);
        expect(decorations.archivedSubThreads.size).toBe(0);
    });

    it('does not mark a stored-archived child that still has live work', () => {
        // `section: 'archived'` is the STORED state; a running (or
        // change-carrying) thread resolves to Current anyway, and dimming it
        // would hide live work.
        const parent = running('parent');
        const runningChild = makeThread('running-child', { parentId: 'parent', section: 'archived', status: 'running' });
        const ctaChild = makeThread('cta-child', { parentId: 'parent', section: 'archived', codingAgentProposed: true });
        const threads = [parent, runningChild, ctaChild];
        const graph = computeFamilyGraph(threads);
        const decorations = computeFamilyDecorations(threads, graph);

        expect(decorations.archivedSubThreads.size).toBe(0);
    });

    it('leaves an archived child alone when the whole family is in Archive', () => {
        // Everyone is archived, so the family renders in Archive and no row is
        // out of place. Dimming there would dim the entire section.
        const parent = inArchive('parent');
        const child = inArchive('child', { parentId: 'parent' });
        const graph = computeFamilyGraph([parent, child]);
        const decorations = computeFamilyDecorations([parent, child], graph);

        expect(decorations.routedByThread.get('child')).toBe('archive');
        expect(decorations.archivedSubThreads.size).toBe(0);
    });

    it('routes a saved-parent + review-child family to Saved without a lift', () => {
        // Saved root pins the family to Saved — root's natural section
        // matches the routed one, so no lift cue fires.
        const parent = inSaved('parent');
        const child = withCta('child', { parentId: 'parent' });
        const graph = computeFamilyGraph([parent, child]);
        const decorations = computeFamilyDecorations([parent, child], graph);

        expect(decorations.liftedRoots.size).toBe(0);
        expect(decorations.routedByThread.get('parent')).toBe('saved');
        expect(decorations.routedByThread.get('child')).toBe('saved');
    });
});
