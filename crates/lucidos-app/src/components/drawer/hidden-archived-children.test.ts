/**
 * Tests for `computeHiddenArchivedThreads` / `filterHiddenArchived` /
 * `visibleChildrenCount` — the fix for an archived child thread staying
 * listed under a live parent (slack-20261002-hide-archived-child-threads).
 *
 * The existing `archivedSubThreads` cue (lifted-family.test.ts) dims such a
 * row but keeps rendering it. This suite covers the layer on top: hiding a
 * FULLY archived branch by default, and revealing it again on request,
 * without ever hiding a branch that still carries live work.
 */

import { describe, it, expect } from 'vitest';
import {
    computeFamilyGraph,
    computeFamilyDecorations,
    computeHiddenArchivedThreads,
    filterHiddenArchived,
    inboxThreadCount,
    visibleChildrenCount,
    orderedCurrentForReview,
    renderedFamilyRows,
} from './ThreadDrawer';
import type { ThreadState, ThreadMeta, ThreadStatus } from '../../store/thread-events';
import type { ArchiveState } from '../../generated/thread-lifecycle';
import type { CodingAgentChangeState } from '../../api/threads';

type ThreadOpts = {
    parentId?: string;
    section?: ArchiveState;
    status?: ThreadStatus;
    saved?: boolean;
    totalChildrenCount?: number;
    codingAgentChangeState?: CodingAgentChangeState;
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
        summaryVersion: 0,
        messageCount: 1,
        section: opts.section ?? 'inbox',
        activeChildrenCount: 0,
        totalChildrenCount: opts.totalChildrenCount ?? 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentChangeState: opts.codingAgentChangeState ?? { kind: 'none' },
        codingAgentIsExternalRepo: false,
        lastRevivedAt: '',
        parentThreadId: opts.parentId,
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

const running = (id: string, opts: ThreadOpts = {}) => makeThread(id, { ...opts, status: 'running' });
const inArchive = (id: string, opts: ThreadOpts = {}) => makeThread(id, { ...opts, section: 'archived' });

describe('computeHiddenArchivedThreads', () => {
    it('hides a fully-archived child under a live parent', () => {
        const parent = running('parent', { totalChildrenCount: 1 });
        const child = inArchive('child', { parentId: 'parent' });
        const graph = computeFamilyGraph([parent, child]);
        const { hidden, hiddenDirectChildCount } = computeHiddenArchivedThreads([parent, child], graph);

        expect([...hidden]).toEqual(['child']);
        expect(hiddenDirectChildCount.get('parent')).toBe(1);
    });

    it('never hides the family root', () => {
        // A fully-archived root with no live descendants naturally routes the
        // whole family to Archive, so there's nothing to hide there either.
        const parent = inArchive('parent');
        const graph = computeFamilyGraph([parent]);
        const { hidden } = computeHiddenArchivedThreads([parent], graph);

        expect(hidden.size).toBe(0);
    });

    it('keeps an archived branch visible when it still carries a live descendant', () => {
        // `child` is archived with no direct live children, but its OWN child
        // is running. That running grandchild makes child's natural section
        // Current too, so child was never a hiding candidate.
        const parent = running('parent', { totalChildrenCount: 1 });
        const child = makeThread('child', { parentId: 'parent', section: 'archived' });
        const grandchild = running('grandchild', { parentId: 'child' });
        const threads = [parent, child, grandchild];
        const graph = computeFamilyGraph(threads);
        const { hidden } = computeHiddenArchivedThreads(threads, graph);

        expect(hidden.size).toBe(0);
    });

    it('hides a whole fully-archived sub-branch, not just its topmost node', () => {
        const parent = running('parent', { totalChildrenCount: 1 });
        const child = inArchive('child', { parentId: 'parent' });
        const grandchild = inArchive('grandchild', { parentId: 'child' });
        const threads = [parent, child, grandchild];
        const graph = computeFamilyGraph(threads);
        const { hidden, hiddenDirectChildCount } = computeHiddenArchivedThreads(threads, graph);

        expect([...hidden].sort()).toEqual(['child', 'grandchild']);
        expect(hiddenDirectChildCount.get('parent')).toBe(1);
        expect(hiddenDirectChildCount.get('child')).toBe(1);
    });

    it('does not hide anything when the whole family already routes to Archive', () => {
        const parent = inArchive('parent');
        const child = inArchive('child', { parentId: 'parent' });
        const threads = [parent, child];
        const graph = computeFamilyGraph(threads);
        const { hidden } = computeHiddenArchivedThreads(threads, graph);

        expect(hidden.size).toBe(0);
    });

    it('does not hide a stored-archived child that still has live work of its own', () => {
        const parent = running('parent', { totalChildrenCount: 1 });
        const runningChild = makeThread('child', { parentId: 'parent', section: 'archived', status: 'running' });
        const threads = [parent, runningChild];
        const graph = computeFamilyGraph(threads);
        const { hidden } = computeHiddenArchivedThreads(threads, graph);

        expect(hidden.size).toBe(0);
    });

    it('resolves a parentThreadId cycle to "not hidden" rather than looping', () => {
        const a = inArchive('a', { parentId: 'b' });
        const b = inArchive('b', { parentId: 'a' });
        const graph = computeFamilyGraph([a, b]);
        const { hidden } = computeHiddenArchivedThreads([a, b], graph);

        expect(hidden.size).toBe(0);
    });
});

describe('filterHiddenArchived', () => {
    it('drops hidden threads from the list by default', () => {
        const parent = running('parent', { totalChildrenCount: 1 });
        const child = inArchive('child', { parentId: 'parent' });
        const threads = [parent, child];
        const graph = computeFamilyGraph(threads);
        const { hidden } = computeHiddenArchivedThreads(threads, graph);

        const visible = filterHiddenArchived(threads, hidden, new Set(), graph);
        expect(visible.map(t => t.meta.id)).toEqual(['parent']);
    });

    it('reveals a hidden child when its direct parent is in the revealed set', () => {
        const parent = running('parent', { totalChildrenCount: 1 });
        const child = inArchive('child', { parentId: 'parent' });
        const threads = [parent, child];
        const graph = computeFamilyGraph(threads);
        const { hidden } = computeHiddenArchivedThreads(threads, graph);

        const visible = filterHiddenArchived(threads, hidden, new Set(['parent']), graph);
        expect(visible.map(t => t.meta.id).sort()).toEqual(['child', 'parent']);
    });

    it('does not cascade a reveal past one level', () => {
        const parent = running('parent', { totalChildrenCount: 1 });
        const child = inArchive('child', { parentId: 'parent' });
        const grandchild = inArchive('grandchild', { parentId: 'child' });
        const threads = [parent, child, grandchild];
        const graph = computeFamilyGraph(threads);
        const { hidden } = computeHiddenArchivedThreads(threads, graph);

        // Revealing the PARENT's archived children shows `child`, but
        // `grandchild` needs its own reveal on `child` to show too.
        const visible = filterHiddenArchived(threads, hidden, new Set(['parent']), graph);
        expect(visible.map(t => t.meta.id).sort()).toEqual(['child', 'parent']);

        const bothRevealed = filterHiddenArchived(threads, hidden, new Set(['parent', 'child']), graph);
        expect(bothRevealed.map(t => t.meta.id).sort()).toEqual(['child', 'grandchild', 'parent']);
    });

    it('is a no-op when nothing is hidden', () => {
        const threads = [running('solo')];
        const graph = computeFamilyGraph(threads);
        expect(filterHiddenArchived(threads, new Set(), new Set(), graph)).toBe(threads);
    });
});

describe('renderedFamilyRows', () => {
    // A family collapsed while it had live sub-threads, all of which were
    // archived since. Its row draws no chevron any more, so that collapse can
    // never be undone. It must not swallow the rows the reveal toggle shows.
    it('shows revealed archived children under a family that draws no chevron', () => {
        const parent = running('parent', { totalChildrenCount: 2 });
        const a = inArchive('a', { parentId: 'parent' });
        const b = inArchive('b', { parentId: 'parent' });
        const threads = [parent, a, b];
        const graph = computeFamilyGraph(threads);
        const decorations = computeFamilyDecorations(threads, graph);
        expect(visibleChildrenCount(parent, decorations)).toBe(0);

        const visible = filterHiddenArchived(threads, decorations.hiddenArchivedThreads, new Set(['parent']), graph);
        const rows = renderedFamilyRows(visible, new Set(['parent']), graph, decorations);
        expect(rows.map(n => n.thread.meta.id).sort()).toEqual(['a', 'b', 'parent']);
    });

    it('still honours the collapse of a family with a live sub-thread', () => {
        const parent = running('parent', { totalChildrenCount: 2 });
        const live = running('live', { parentId: 'parent' });
        const archived = inArchive('archived', { parentId: 'parent' });
        const threads = [parent, live, archived];
        const graph = computeFamilyGraph(threads);
        const decorations = computeFamilyDecorations(threads, graph);

        const visible = filterHiddenArchived(threads, decorations.hiddenArchivedThreads, new Set(['parent']), graph);
        const rows = renderedFamilyRows(visible, new Set(['parent']), graph, decorations);
        expect(rows.map(n => n.thread.meta.id)).toEqual(['parent']);
    });
});

describe('visibleChildrenCount', () => {
    it('subtracts hidden archived children from totalChildrenCount', () => {
        const parent = running('parent', { totalChildrenCount: 3 });
        const child1 = inArchive('child1', { parentId: 'parent' });
        const child2 = inArchive('child2', { parentId: 'parent' });
        const liveChild = running('child3', { parentId: 'parent' });
        const threads = [parent, child1, child2, liveChild];
        const graph = computeFamilyGraph(threads);
        const decorations = computeFamilyDecorations(threads, graph);

        expect(visibleChildrenCount(parent, decorations)).toBe(1);
    });

    it('is unaffected by whether the hidden children are currently revealed', () => {
        // The "N sub-threads" count stays constant; only the separate archived
        // toggle's own label/visibility changes with reveal state.
        const parent = running('parent', { totalChildrenCount: 2 });
        const child = inArchive('child', { parentId: 'parent' });
        const liveChild = running('child2', { parentId: 'parent' });
        const threads = [parent, child, liveChild];
        const graph = computeFamilyGraph(threads);
        const decorations = computeFamilyDecorations(threads, graph);

        expect(visibleChildrenCount(parent, decorations)).toBe(1);
    });

    it('returns the full count when nothing is hidden', () => {
        const parent = running('parent', { totalChildrenCount: 2 });
        const graph = computeFamilyGraph([parent]);
        const decorations = computeFamilyDecorations([parent], graph);

        expect(visibleChildrenCount(parent, decorations)).toBe(2);
    });
});

describe('orderedCurrentForReview excludes default-hidden archived branches', () => {
    it('never offers a hidden archived child as the next-focus candidate', () => {
        const parent = running('parent', { totalChildrenCount: 1 });
        const hiddenChild = inArchive('hidden-child', { parentId: 'parent' });
        const threads = [parent, hiddenChild];
        const graph = computeFamilyGraph(threads);

        const ordered = orderedCurrentForReview(threads, graph).map(t => t.meta.id);
        expect(ordered).toEqual(['parent']);
    });
});

describe('inboxThreadCount, the section badge', () => {
    it('counts inbox threads only, as Archive all counts them', () => {
        // A lifted archived parent, a dimmed archived middle over a live
        // grandchild, and a revealed archived branch all render in Current.
        const lifted = inArchive('lifted', { totalChildrenCount: 2 });
        const live = running('live', { parentId: 'lifted' });
        const middle = inArchive('middle', { parentId: 'lifted', totalChildrenCount: 1 });
        const grandchild = running('grandchild', { parentId: 'middle' });
        const revealed = inArchive('revealed', { parentId: 'live' });
        const idle = makeThread('idle');

        expect(inboxThreadCount([lifted, live, middle, grandchild, revealed, idle])).toBe(3);
    });
});
