
import { describe, it, expect } from 'vitest';
import { categorizeThreads, computeFamilyKeys, THREAD_DRAWER_SECTION_ORDER } from './ThreadDrawer';
import type { ThreadState, ThreadMeta, ThreadStatus } from '../../store/thread-events';
import type { ArchiveState } from '../../generated/thread-lifecycle';
import type { CodingAgentChangeState } from '../../api/threads';

type ThreadOpts = {
    parentId?: string;
    section?: ArchiveState;
    status?: ThreadStatus;
    saved?: boolean;
    activeChildrenCount?: number;
    codingAgentChangeState?: CodingAgentChangeState;
    updatedAt?: string;
    lastRevivedAt?: string;
    heldBy?: string;
};

function makeThread(id: string, opts: ThreadOpts = {}): ThreadState {
    const meta: ThreadMeta = {
        id,
        title: id,
        channel: 'chat',
        initiator: 'user',
        saved: opts.saved ?? false,
        createdAt: opts.updatedAt ?? '2026-04-12T00:00:00Z',
        updatedAt: opts.updatedAt ?? '2026-04-12T00:00:00Z',
        status: opts.status ?? 'idle',
        summaryVersion: 0,
        messageCount: 1,
        section: opts.section ?? 'inbox',
        activeChildrenCount: opts.activeChildrenCount ?? 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0, archivableSubThreadCount: 0,
        codingAgentChangeState: opts.codingAgentChangeState ?? { kind: 'none' },
        codingAgentIsExternalRepo: false,
        lastRevivedAt: opts.lastRevivedAt ?? '',
        parentThreadId: opts.parentId,
        state: 'active',
        latestTodoList: null,
        liveEventWaitCount: 0,
        liveEventWaits: [],
        hold: opts.heldBy
            ? { reason: 'Waiting on the vendor', root_thread_id: opts.heldBy, placed_at: '2026-04-12T00:00:00Z' }
            : undefined,
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

const running = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, status: 'running' });
const withCta = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, codingAgentChangeState: { kind: 'proposed', requires_restart: false }, section: 'inbox' });
const inSaved = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, saved: true });
const inArchive = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, section: 'archived' });

function ids(list: ThreadState[]): string[] {
    return list.map(t => t.meta.id);
}

describe('categorizeThreads — family-aware section routing', () => {
    it('keeps a family together in Current when any descendant is running, even with a sibling CTA', () => {
        const parent = running('parent');
        const child = withCta('child', { parentId: 'parent' });
        const out = categorizeThreads([parent, child]);
        expect(ids(out.current)).toEqual(['parent', 'child']);
    });

    it('keeps a family in Current when idle no-CTA siblings sit under a running child', () => {
        const parent = makeThread('parent', { activeChildrenCount: 1 });
        const runningChild = running('running-child', { parentId: 'parent' });
        const doneA = makeThread('done-a', { parentId: 'parent' });
        const doneB = makeThread('done-b', { parentId: 'parent' });
        const out = categorizeThreads([parent, runningChild, doneA, doneB]);
        expect(ids(out.current).sort()).toEqual(['done-a', 'done-b', 'parent', 'running-child']);
    });

    it('pins family to Saved when the root is saved, regardless of a CTA descendant', () => {
        const parent = inSaved('parent');
        const child = withCta('child', { parentId: 'parent' });
        const out = categorizeThreads([parent, child]);
        expect(ids(out.saved)).toEqual(['parent', 'child']);
        expect(out.current).toEqual([]);
    });

    it('pins family to Saved when the root is saved, even with a running descendant', () => {
        const parent = inSaved('parent');
        const runningChild = running('running-child', { parentId: 'parent' });
        const ctaChild = withCta('cta-child', { parentId: 'parent' });
        const out = categorizeThreads([parent, runningChild, ctaChild]);
        expect(ids(out.saved).sort()).toEqual(['cta-child', 'parent', 'running-child']);
        expect(out.current).toEqual([]);
    });

    it('lifts an archived parent into Current when a child is running', () => {
        const parent = inArchive('parent');
        const child = running('child', { parentId: 'parent' });
        const out = categorizeThreads([parent, child]);
        expect(ids(out.current)).toEqual(['parent', 'child']);
        expect(out.archive).toEqual([]);
    });

    it('leaves an orphan child as its own family root when the parent is paginated out', () => {
        const child = withCta('child', { parentId: 'missing-parent' });
        const sibling = running('sibling');
        const out = categorizeThreads([child, sibling]);
        expect(ids(out.current)).toEqual(['child', 'sibling']);
    });

    it('lifts the whole chain into Current when running anywhere pulls the family up', () => {
        const grandparent = running('grandparent');
        const child = inSaved('child', { parentId: 'grandparent' });
        const grandchild = withCta('grandchild', { parentId: 'child' });
        const out = categorizeThreads([grandparent, child, grandchild]);
        expect(ids(out.current).sort()).toEqual(['child', 'grandchild', 'grandparent']);
        expect(out.saved).toEqual([]);
    });

    it('keeps unrelated families in their own sections', () => {
        const p1 = running('p1');
        const c1 = running('c1', { parentId: 'p1' });
        const p2 = inArchive('p2');
        const out = categorizeThreads([p1, c1, p2]);
        expect(ids(out.current).sort()).toEqual(['c1', 'p1']);
        expect(ids(out.archive)).toEqual(['p2']);
    });

    it('excludes composing and discarded threads as before', () => {
        const parent = running('parent');
        const child = withCta('child', { parentId: 'parent' });
        child.meta.state = 'composing';
        const out = categorizeThreads([parent, child]);
        expect(ids(out.current)).toEqual(['parent']);
    });
});

describe('categorizeThreads — On hold', () => {
    it('lists a held root and the sub-threads it reached in On hold, even while one runs', () => {
        const root = makeThread('root', { heldBy: 'root' });
        const child = running('child', { parentId: 'root', heldBy: 'root' });
        const out = categorizeThreads([root, child]);
        expect(ids(out.onHold)).toEqual(['root', 'child']);
        expect(out.current).toEqual([]);
    });

    it('roots a family at the held thread, so its live parent stays in Current', () => {
        const parent = running('parent');
        const held = withCta('held', { parentId: 'parent', heldBy: 'held' });
        const out = categorizeThreads([parent, held]);
        expect(ids(out.current)).toEqual(['parent']);
        expect(ids(out.onHold)).toEqual(['held']);
    });

    it('keeps a saved held root in Saved, since a pin beats a hold', () => {
        const root = makeThread('root', { saved: true, heldBy: 'root' });
        const out = categorizeThreads([root]);
        expect(ids(out.saved)).toEqual(['root']);
        expect(out.onHold).toEqual([]);
    });
});

describe('ThreadDrawer section order', () => {
    it('renders Saved, then On hold, Current and Archive', () => {
        expect(THREAD_DRAWER_SECTION_ORDER).toEqual(['saved', 'on_hold', 'current', 'archive']);
    });
});

describe('computeFamilyKeys — family-aware effective sort keys', () => {
    it('parent inherits the freshest descendant updatedAt for recency sort', () => {
        const parent = makeThread('parent', { updatedAt: '2026-04-01T00:00:00Z' });
        const child = makeThread('child', {
            parentId: 'parent',
            updatedAt: '2026-05-15T00:00:00Z',
        });
        const keys = computeFamilyKeys([parent, child]);
        expect(keys.get('parent')?.recentKey).toBe('2026-05-15T00:00:00Z');
        expect(keys.get('child')?.recentKey).toBe('2026-05-15T00:00:00Z');
    });

    it('isolated family with stale parent + fresh child sorts above an unrelated stale family', () => {
        const aParent = inArchive('a-parent', { updatedAt: '2026-04-01T00:00:00Z' });
        const aChild = inArchive('a-child', {
            parentId: 'a-parent',
            updatedAt: '2026-05-15T00:00:00Z',
        });
        const bParent = inArchive('b-parent', { updatedAt: '2026-04-10T00:00:00Z' });
        const threads = [aParent, aChild, bParent];
        const keys = computeFamilyKeys(threads);
        const sorted = [...threads].sort((x, y) =>
            keys.get(y.meta.id)!.recentKey.localeCompare(keys.get(x.meta.id)!.recentKey),
        );
        expect(ids(sorted)).toEqual(['a-parent', 'a-child', 'b-parent']);
    });

    it('cycle members converge on a shared root so the family is one record', () => {
        const a = makeThread('a', { parentId: 'b' });
        const b = makeThread('b', { parentId: 'a' });
        const keys = computeFamilyKeys([a, b]);
        expect(keys.get('a')).toBe(keys.get('b'));
    });
});
