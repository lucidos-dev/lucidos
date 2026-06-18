/**
 * Tests for categorizeThreads + the family-aware effective sort keys in
 * ThreadDrawer. The contract: a family (a thread plus all transitive
 * descendants reachable via parentThreadId) renders as a unit — same section,
 * sorted together — so nestByParent can render the child directly below its
 * parent. Section priority: current > saved > archive.
 */

import { describe, it, expect } from 'vitest';
import { categorizeThreads, computeFamilyKeys, THREAD_DRAWER_SECTION_ORDER } from './ThreadDrawer';
import type { ThreadState, ThreadMeta, ThreadStatus } from '../../store/thread-events';
import type { ArchiveState } from '../../generated/thread-lifecycle';

type ThreadOpts = {
    parentId?: string;
    section?: ArchiveState;
    status?: ThreadStatus;
    saved?: boolean;
    activeChildrenCount?: number;
    codingAgentProposed?: boolean;
    updatedAt?: string;
    lastRevivedAt?: string;
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
        lastRevivedAt: opts.lastRevivedAt ?? '',
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

// Convenience builders. A running thread and a CTA (codingAgentProposed) thread
// both land in Current — the former is the system's turn, the latter the
// user's — distinguished at the row level (status icon / attention filter), not
// by a separate section. Current itself sorts purely by creation time.
const running = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, status: 'running' });
const withCta = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, codingAgentProposed: true, section: 'inbox' });
const inSaved = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, saved: true });
const inArchive = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, section: 'archived' });

function ids(list: ThreadState[]): string[] {
    return list.map(t => t.meta.id);
}

describe('categorizeThreads — family-aware section routing', () => {
    it('keeps a family together in Current when any descendant is running, even with a sibling CTA', () => {
        // Running and CTA both resolve to Current, so the family renders as one
        // unit there. The CTA still surfaces on the child row + the attention
        // sort; it no longer pulls the family into a separate section.
        const parent = running('parent');
        const child = withCta('child', { parentId: 'parent' });
        const out = categorizeThreads([parent, child]);
        expect(ids(out.current)).toEqual(['parent', 'child']);
    });

    it('keeps a family in Current when idle no-CTA siblings sit under a running child', () => {
        // A CC parent with one running child and two idle children (changes
        // applied, codingAgentProposed=false) renders as one family unit in
        // Current — no member splits off into another section.
        const parent = makeThread('parent', { activeChildrenCount: 1 });
        const runningChild = running('running-child', { parentId: 'parent' });
        const doneA = makeThread('done-a', { parentId: 'parent' });
        const doneB = makeThread('done-b', { parentId: 'parent' });
        const out = categorizeThreads([parent, runningChild, doneA, doneB]);
        expect(ids(out.current).sort()).toEqual(['done-a', 'done-b', 'parent', 'running-child']);
    });

    it('pins family to Saved when the root is saved, regardless of a CTA descendant', () => {
        // Save is an explicit user pin — "keep this visible to me" — and
        // overrides automatic categorization for the whole family.
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
        // parent isn't in the input list (filtered out, different page, etc.).
        // Child has no anchor — it's its own root and lands in Current.
        const child = withCta('child', { parentId: 'missing-parent' });
        const sibling = running('sibling');
        const out = categorizeThreads([child, sibling]);
        expect(ids(out.current)).toEqual(['child', 'sibling']);
    });

    it('lifts the whole chain into Current when running anywhere pulls the family up', () => {
        // Grandparent running, child saved (non-root), grandchild has a CTA →
        // family in Current. Current outranks a saved NON-root member; only a
        // saved ROOT pins the family to Saved.
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
        // Child is filtered out entirely → parent stays in Current alone.
        expect(ids(out.current)).toEqual(['parent']);
    });
});

describe('ThreadDrawer section order', () => {
    it('renders Saved before Current and Archive', () => {
        expect(THREAD_DRAWER_SECTION_ORDER).toEqual(['saved', 'current', 'archive']);
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
        // Every family member maps to the same family-derived record.
        expect(keys.get('child')?.recentKey).toBe('2026-05-15T00:00:00Z');
    });

    it('isolated family with stale parent + fresh child sorts above an unrelated stale family', () => {
        // Two families in the same archive section. Family A's child has fresh
        // activity; family B has nothing recent. Sorted by recentKey desc,
        // family A (lifted by its child) must come first.
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
        // a-parent and a-child tie on the family recent key, so their relative
        // order is the input order. The key assertion: both precede b-parent.
        expect(ids(sorted)).toEqual(['a-parent', 'a-child', 'b-parent']);
    });

    it('cycle members converge on a shared root so the family is one record', () => {
        // Defensive: data corruption could create a parentThreadId cycle. The
        // implementation canonicalizes on the lex-min visited id so both
        // members map to the *same* FamilyKeys record.
        const a = makeThread('a', { parentId: 'b' });
        const b = makeThread('b', { parentId: 'a' });
        const keys = computeFamilyKeys([a, b]);
        expect(keys.get('a')).toBe(keys.get('b'));
    });
});
