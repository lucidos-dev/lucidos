// @vitest-environment jsdom
/**
 * Show in Folders: `planFoldersReveal` decides what hides a thread's
 * row, and `revealThreadInFolders` undoes exactly that. It always lands on the
 * Folders grouping. The thread filter is the one thing it never undoes on its
 * own: a filtered-out thread gets a toast.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { planFoldersReveal, revealThreadInFolders, setFamilyCollapsed, setSectionCollapsed } from './ThreadDrawer';
import { computeFamilyGraph, collapsedAncestorIds } from './family-graph';
import {
    ALL_CHANNELS, drawerGrouping, selectedOngoingGroup, setDrawerGrouping, setSelectedOngoingGroup,
    setThreadChannelFilter, threadChannelFilter, threadDrawerOpen, threadMap, threadSearchQuery, toasts,
} from '../../store/store';
import { appliedThreadFilter, type ThreadFilterSelection } from '../../store/appliedThreadFilter';
import type { ThreadState, ThreadMeta } from '../../store/thread-events';
import type { ArchiveState, EventChannel } from '../../generated/thread-lifecycle';

type ThreadOpts = { parentId?: string; section?: ArchiveState; channel?: EventChannel; state?: ThreadMeta['state'] };

function makeThread(id: string, opts: ThreadOpts = {}): ThreadState {
    const meta: ThreadMeta = {
        id,
        title: id,
        channel: opts.channel ?? 'chat',
        initiator: 'user',
        saved: false,
        createdAt: '2026-09-01T00:00:00Z',
        updatedAt: '2026-09-01T00:00:00Z',
        status: 'idle',
        summaryVersion: 0,
        messageCount: 1,
        section: opts.section ?? 'inbox',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentChangeState: { kind: 'none' },
        codingAgentIsExternalRepo: false,
        lastRevivedAt: '',
        parentThreadId: opts.parentId,
        state: opts.state ?? 'active',
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

const ALL: ThreadFilterSelection = {
    channels: new Set(ALL_CHANNELS), triggerIds: new Set(), repoIds: new Set(), appIds: new Set(),
};
const CHAT_ONLY: ThreadFilterSelection = { ...ALL, channels: new Set(['chat']) };

describe('collapsedAncestorIds', () => {
    it('names every collapsed ancestor, nearest first, and nothing else', () => {
        const threads = [
            makeThread('root'), makeThread('mid', { parentId: 'root' }), makeThread('leaf', { parentId: 'mid' }),
            makeThread('other'),
        ];
        const graph = computeFamilyGraph(threads);
        expect(collapsedAncestorIds('leaf', new Set(['root', 'mid', 'other']), graph)).toEqual(['mid', 'root']);
        expect(collapsedAncestorIds('leaf', new Set(['other']), graph)).toEqual([]);
    });

    it('terminates on a parent cycle', () => {
        const graph = computeFamilyGraph([makeThread('a', { parentId: 'b' }), makeThread('b', { parentId: 'a' })]);
        expect(collapsedAncestorIds('a', new Set(['a', 'b']), graph)).toEqual(['b']);
    });
});

describe('planFoldersReveal', () => {
    it('reveals a top-level thread in its own section with nothing to expand', () => {
        const threads = [makeThread('t')];
        expect(planFoldersReveal('t', threads, ALL, new Set()))
            .toEqual({ kind: 'reveal', section: 'current', collapsedAncestors: [], archivedAncestorsToReveal: [] });
    });

    it('expands the collapsed ancestors of a nested sub-thread', () => {
        const threads = [
            makeThread('root'), makeThread('mid', { parentId: 'root' }), makeThread('leaf', { parentId: 'mid' }),
        ];
        expect(planFoldersReveal('leaf', threads, ALL, new Set(['root', 'mid'])))
            .toEqual({
                kind: 'reveal', section: 'current',
                collapsedAncestors: ['mid', 'root'], archivedAncestorsToReveal: [],
            });
    });

    it('names the section the family routes to, not the thread own section', () => {
        // An archived sub-thread under a live parent renders in Current.
        const threads = [makeThread('root'), makeThread('child', { parentId: 'root', section: 'archived' })];
        expect(planFoldersReveal('child', threads, ALL, new Set()))
            .toMatchObject({ kind: 'reveal', section: 'current' });
    });

    it('names the hidden-archived ancestor chain of a thread the family tree hides', () => {
        // child is a fully-archived branch, hidden by default under the live
        // root — landing on it (e.g. via search) must reveal it there too.
        const threads = [makeThread('root'), makeThread('child', { parentId: 'root', section: 'archived' })];
        expect(planFoldersReveal('child', threads, ALL, new Set()))
            .toEqual({
                kind: 'reveal', section: 'current',
                collapsedAncestors: [], archivedAncestorsToReveal: ['root'],
            });
    });

    it('names every hidden step up a fully-archived multi-level chain', () => {
        const threads = [
            makeThread('root'),
            makeThread('mid', { parentId: 'root', section: 'archived' }),
            makeThread('leaf', { parentId: 'mid', section: 'archived' }),
        ];
        expect(planFoldersReveal('leaf', threads, ALL, new Set()))
            .toEqual({
                kind: 'reveal', section: 'current',
                collapsedAncestors: [], archivedAncestorsToReveal: ['mid', 'root'],
            });
    });

    it('reports a thread the filter hides, without planning any expansion', () => {
        const threads = [makeThread('t', { channel: 'trigger' })];
        expect(planFoldersReveal('t', threads, CHAT_ONLY, new Set())).toEqual({ kind: 'hidden-by-filter' });
    });

    it('reports a thread no section lists', () => {
        expect(planFoldersReveal('missing', [makeThread('t')], ALL, new Set())).toEqual({ kind: 'not-listed' });
    });
});

describe('revealThreadInFolders', () => {
    beforeEach(() => {
        localStorage.clear();
        toasts.value = [];
        threadDrawerOpen.value = false;
        threadSearchQuery.value = '';
        setDrawerGrouping('folders');
        setSelectedOngoingGroup('blocked');
        setThreadChannelFilter(new Set(ALL_CHANNELS));
        setSectionCollapsed('current', false);
        for (const id of ['root', 'mid', 'other']) setFamilyCollapsed(id, false);
    });

    it('opens Folders, undoing the search, section and family collapse', () => {
        threadMap.value = new Map([
            ['root', makeThread('root')],
            ['mid', makeThread('mid', { parentId: 'root' })],
            ['leaf', makeThread('leaf', { parentId: 'mid' })],
            ['other', makeThread('other')],
        ]);
        threadSearchQuery.value = 'something';
        setDrawerGrouping('ongoing');
        setSectionCollapsed('current', true);
        setFamilyCollapsed('root', true);
        setFamilyCollapsed('mid', true);
        setFamilyCollapsed('other', true);

        revealThreadInFolders('leaf');

        expect(threadDrawerOpen.value).toBe(true);
        expect(threadSearchQuery.value).toBe('');
        expect(drawerGrouping.value).toBe('folders');
        expect(JSON.parse(localStorage.getItem('lucidos-drawer-collapsed') ?? '[]')).not.toContain('current');
        // Only the leaf's own ancestors open. An unrelated family stays collapsed.
        expect(JSON.parse(localStorage.getItem('lucidos-drawer-collapsed-families') ?? '[]')).toEqual(['other']);
        expect(toasts.value).toHaveLength(0);
    });

    it('opens Folders for a thread an ongoing group lists, leaving the selected group alone', () => {
        const change = makeThread('change', { channel: 'claude_code' });
        change.meta.codingAgentChangeState = { kind: 'proposed', requires_restart: false };
        threadMap.value = new Map([['change', change]]);
        setDrawerGrouping('ongoing');
        setSelectedOngoingGroup('review');

        revealThreadInFolders('change');

        expect(threadDrawerOpen.value).toBe(true);
        expect(drawerGrouping.value).toBe('folders');
        expect(selectedOngoingGroup.value).toBe('review');
        expect(toasts.value).toHaveLength(0);
    });

    it('forgets the list scroll position, so the grouping switch restores nothing over the reveal', () => {
        threadMap.value = new Map([['t', makeThread('t')]]);
        localStorage.setItem('lucidos-scroll-thread-drawer', '0');
        setDrawerGrouping('ongoing');

        revealThreadInFolders('t');

        expect(localStorage.getItem('lucidos-scroll-thread-drawer')).toBeNull();
    });

    it('reveals a fully-archived branch hidden under its live parent', () => {
        threadMap.value = new Map([
            ['root', makeThread('root')],
            ['child', makeThread('child', { parentId: 'root', section: 'archived' })],
        ]);

        revealThreadInFolders('child');

        expect(JSON.parse(localStorage.getItem('lucidos-drawer-revealed-archived') ?? '[]')).toEqual(['root']);
        expect(toasts.value).toHaveLength(0);
    });

    it('leaves the filter alone for a filtered-out thread and offers to clear it', () => {
        threadMap.value = new Map([['t', makeThread('t', { channel: 'trigger' })]]);
        setThreadChannelFilter(new Set(['chat']));
        setDrawerGrouping('ongoing');

        revealThreadInFolders('t');

        expect([...threadChannelFilter.value]).toEqual(['chat']);
        expect(drawerGrouping.value).toBe('ongoing');
        expect(toasts.value).toHaveLength(1);
        const action = toasts.value[0].action;
        expect(action?.label).toBe('Clear filter');

        action?.onClick();

        expect(appliedThreadFilter.value.channels.size).toBe(ALL_CHANNELS.length);
        expect(drawerGrouping.value).toBe('folders');
    });
});
