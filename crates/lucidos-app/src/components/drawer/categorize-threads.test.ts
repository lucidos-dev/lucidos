/**
 * Tests for categorizeThreads + the family-aware effective sort keys in
 * ThreadDrawer. The contract: a family (a thread plus all transitive
 * descendants reachable via parentThreadId) renders as a unit — same section,
 * sorted together — so nestByParent can render the child directly below its
 * parent. Section priority: active > review > saved > archive.
 */

import { describe, it, expect } from 'vitest';
import { categorizeThreads, computeFamilyKeys } from './ThreadDrawer';
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

// Convenience: each helper produces a thread whose own getThreadDisplaySection
// resolves to the named section, so test intent reads cleanly at the call site.
const inReview = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, codingAgentProposed: true, section: 'inbox' });
const inActive = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, status: 'running' });
const inSaved = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, saved: true });
const inArchive = (id: string, opts: ThreadOpts = {}) =>
    makeThread(id, { ...opts, section: 'archived' });

function ids(list: ThreadState[]): string[] {
    return list.map(t => t.meta.id);
}

describe('categorizeThreads — family-aware section routing', () => {
    it('keeps a family in active when any descendant is running, even if a sibling has a CTA', () => {
        // Running-anywhere always wins over review — even a real codingAgentProposed
        // CTA. The CTA still renders inline on the child row; the family lands in
        // Active. This is the priority rule: active > review > saved > archive.
        const parent = inActive('parent');
        const child = inReview('child', { parentId: 'parent' });
        const out = categorizeThreads([parent, child]);
        expect(ids(out.active)).toEqual(['parent', 'child']);
        expect(out.review).toEqual([]);
    });

    it('keeps a family in active when an idle no-CTA sibling would otherwise fall through to review', () => {
        // Regression: a CC parent thread with one running child and two idle children
        // (changes already applied, so codingAgentProposed=false) previously had the
        // whole family dragged into Review because the idle children's displaySection
        // falls through to 'review'. Active must beat review so the family stays
        // grouped under Active while work is in progress.
        const parent = makeThread('parent', { activeChildrenCount: 1 });
        const running = inActive('running-child', { parentId: 'parent' });
        const doneA = makeThread('done-a', { parentId: 'parent' });
        const doneB = makeThread('done-b', { parentId: 'parent' });
        const out = categorizeThreads([parent, running, doneA, doneB]);
        expect(ids(out.active).sort()).toEqual(['done-a', 'done-b', 'parent', 'running-child']);
        expect(out.review).toEqual([]);
    });

    it('pins family to Saved when the root is saved, regardless of a review-needing descendant', () => {
        // Save is an explicit user pin — "keep this visible to me" — and
        // overrides automatic categorization for the whole family, matching
        // the leaf-thread rule where isSaved beats running/review/etc.
        const parent = inSaved('parent');
        const child = inReview('child', { parentId: 'parent' });
        const out = categorizeThreads([parent, child]);
        expect(ids(out.saved)).toEqual(['parent', 'child']);
        expect(out.review).toEqual([]);
    });

    it('pins family to Saved when the root is saved, even with a running descendant', () => {
        // The saved-root override beats the active-anywhere lift too — a
        // saved parent stays in Saved even while a child is running.
        const parent = inSaved('parent');
        const runningChild = inActive('running-child', { parentId: 'parent' });
        const reviewChild = inReview('review-child', { parentId: 'parent' });
        const out = categorizeThreads([parent, runningChild, reviewChild]);
        expect(ids(out.saved).sort()).toEqual(['parent', 'review-child', 'running-child']);
        expect(out.active).toEqual([]);
        expect(out.review).toEqual([]);
    });

    it('lifts an archived parent into active when a child is active', () => {
        const parent = inArchive('parent');
        const child = inActive('child', { parentId: 'parent' });
        const out = categorizeThreads([parent, child]);
        expect(ids(out.active)).toEqual(['parent', 'child']);
        expect(out.archive).toEqual([]);
    });

    it('leaves an orphan child in its own section when the parent is paginated out', () => {
        // parent isn't in the input list (filtered out, on a different page, etc.).
        // Child has no anchor — keeps its intrinsic section and renders as a root there.
        const child = inReview('child', { parentId: 'missing-parent' });
        const sibling = inActive('sibling');
        const out = categorizeThreads([child, sibling]);
        expect(ids(out.review)).toEqual(['child']);
        expect(ids(out.active)).toEqual(['sibling']);
    });

    it('lifts the whole chain when running anywhere pulls the family up', () => {
        // Grandparent active (running), child saved, grandchild needs review → family
        // in active. Running anywhere always wins, including over a real review CTA.
        const grandparent = inActive('grandparent');
        const child = inSaved('child', { parentId: 'grandparent' });
        const grandchild = inReview('grandchild', { parentId: 'child' });
        const out = categorizeThreads([grandparent, child, grandchild]);
        expect(ids(out.active).sort()).toEqual(['child', 'grandchild', 'grandparent']);
        expect(out.review).toEqual([]);
        expect(out.saved).toEqual([]);
    });

    it('keeps unrelated families in their own sections', () => {
        const p1 = inActive('p1');
        const c1 = inActive('c1', { parentId: 'p1' });
        const p2 = inArchive('p2');
        const out = categorizeThreads([p1, c1, p2]);
        expect(ids(out.active).sort()).toEqual(['c1', 'p1']);
        expect(ids(out.archive)).toEqual(['p2']);
    });

    it('excludes composing and discarded threads as before', () => {
        const parent = inActive('parent');
        const child = inReview('child', { parentId: 'parent' });
        child.meta.state = 'composing';
        const out = categorizeThreads([parent, child]);
        // Child is filtered out entirely → no review lift, parent stays active.
        expect(out.review).toEqual([]);
        expect(ids(out.active)).toEqual(['parent']);
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

    it('parent inherits the freshest descendant lastRevivedAt for active sort', () => {
        const parent = makeThread('parent', {
            updatedAt: '2026-04-01T00:00:00Z',
            lastRevivedAt: '2026-04-01T00:00:00Z',
        });
        const child = makeThread('child', {
            parentId: 'parent',
            updatedAt: '2026-04-02T00:00:00Z',
            lastRevivedAt: '2026-05-15T00:00:00Z',
        });
        const keys = computeFamilyKeys([parent, child]);
        expect(keys.get('parent')?.revivedKey).toBe('2026-05-15T00:00:00Z');
    });

    it('parent inherits the highest-priority review tier from any descendant', () => {
        // Parent has no review signal (tier 1), child has codingAgentProposed (tier 0).
        const parent = inActive('parent');
        const child = inReview('child', { parentId: 'parent' });
        const keys = computeFamilyKeys([parent, child]);
        expect(keys.get('parent')?.reviewTier).toBe(0);
        expect(keys.get('child')?.reviewTier).toBe(0);
    });

    it('isolated family with stale parent + fresh child sorts above an unrelated stale family', () => {
        // Two families in the same archive section. Family A's child has fresh
        // activity; family B has nothing recent. Sorted by recentKey desc, family A
        // (lifted by its child) must come first.
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
        // order is the input order. The important assertion: both come before b-parent.
        expect(ids(sorted)).toEqual(['a-parent', 'a-child', 'b-parent']);
    });

    it('cycle members converge on a shared root so the family is one record', () => {
        // Defensive: data corruption could create a parentThreadId cycle. The
        // implementation canonicalizes on the lex-min visited id so both
        // members map to the *same* FamilyKeys record — otherwise the family
        // would split across two roots and stop rendering as a unit.
        const a = makeThread('a', { parentId: 'b' });
        const b = makeThread('b', { parentId: 'a' });
        const keys = computeFamilyKeys([a, b]);
        expect(keys.get('a')).toBe(keys.get('b'));
    });
});
