// @vitest-environment jsdom
/**
 * Show in thread list: `planThreadListReveal` decides what hides a thread's
 * row, and `revealThreadInList` undoes exactly that. The thread filter is the
 * one thing it never undoes on its own: a filtered-out thread gets a toast.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { planThreadListReveal, revealThreadInList, setFamilyCollapsed, setSectionCollapsed } from './ThreadDrawer';
import { computeFamilyGraph, collapsedAncestorIds } from './family-graph';
import {
    ALL_CHANNELS, drawerView, setDrawerView, setThreadChannelFilter, threadChannelFilter,
    threadDrawerOpen, threadMap, threadSearchQuery, toasts,
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
        codingAgentHasDiff: false,
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
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

describe('planThreadListReveal', () => {
    it('reveals a top-level thread in its own section with nothing to expand', () => {
        const threads = [makeThread('t')];
        expect(planThreadListReveal('t', threads, ALL, new Set()))
            .toEqual({ kind: 'reveal', section: 'current', collapsedAncestors: [] });
    });

    it('expands the collapsed ancestors of a nested sub-thread', () => {
        const threads = [
            makeThread('root'), makeThread('mid', { parentId: 'root' }), makeThread('leaf', { parentId: 'mid' }),
        ];
        expect(planThreadListReveal('leaf', threads, ALL, new Set(['root', 'mid'])))
            .toEqual({ kind: 'reveal', section: 'current', collapsedAncestors: ['mid', 'root'] });
    });

    it('names the section the family routes to, not the thread own section', () => {
        // An archived sub-thread under a live parent renders in Current.
        const threads = [makeThread('root'), makeThread('child', { parentId: 'root', section: 'archived' })];
        expect(planThreadListReveal('child', threads, ALL, new Set()))
            .toMatchObject({ kind: 'reveal', section: 'current' });
    });

    it('reports a thread the filter hides, without planning any expansion', () => {
        const threads = [makeThread('t', { channel: 'trigger' })];
        expect(planThreadListReveal('t', threads, CHAT_ONLY, new Set())).toEqual({ kind: 'hidden-by-filter' });
    });

    it('reports a thread no section lists', () => {
        expect(planThreadListReveal('missing', [makeThread('t')], ALL, new Set())).toEqual({ kind: 'not-listed' });
    });
});

describe('revealThreadInList', () => {
    beforeEach(() => {
        localStorage.clear();
        toasts.value = [];
        threadDrawerOpen.value = false;
        threadSearchQuery.value = '';
        setDrawerView('all');
        setThreadChannelFilter(new Set(ALL_CHANNELS));
        setSectionCollapsed('current', false);
        for (const id of ['root', 'mid', 'other']) setFamilyCollapsed(id, false);
    });

    it('opens the list and undoes the search, view, section and family collapse hiding the row', () => {
        threadMap.value = new Map([
            ['root', makeThread('root')],
            ['mid', makeThread('mid', { parentId: 'root' })],
            ['leaf', makeThread('leaf', { parentId: 'mid' })],
            ['other', makeThread('other')],
        ]);
        threadSearchQuery.value = 'something';
        setDrawerView('attention');
        setSectionCollapsed('current', true);
        setFamilyCollapsed('root', true);
        setFamilyCollapsed('mid', true);
        setFamilyCollapsed('other', true);

        revealThreadInList('leaf');

        expect(threadDrawerOpen.value).toBe(true);
        expect(threadSearchQuery.value).toBe('');
        expect(drawerView.value).toBe('all');
        expect(JSON.parse(localStorage.getItem('lucidos-drawer-collapsed') ?? '[]')).not.toContain('current');
        // Only the leaf's own ancestors open. An unrelated family stays collapsed.
        expect(JSON.parse(localStorage.getItem('lucidos-drawer-collapsed-families') ?? '[]')).toEqual(['other']);
        expect(toasts.value).toHaveLength(0);
    });

    it('forgets the list scroll position, so the view switch restores nothing over the reveal', () => {
        threadMap.value = new Map([['t', makeThread('t')]]);
        localStorage.setItem('lucidos-scroll-thread-drawer', '0');
        setDrawerView('attention');

        revealThreadInList('t');

        expect(localStorage.getItem('lucidos-scroll-thread-drawer')).toBeNull();
    });

    it('leaves the filter alone for a filtered-out thread and offers to clear it', () => {
        threadMap.value = new Map([['t', makeThread('t', { channel: 'trigger' })]]);
        setThreadChannelFilter(new Set(['chat']));
        setDrawerView('attention');

        revealThreadInList('t');

        expect([...threadChannelFilter.value]).toEqual(['chat']);
        expect(drawerView.value).toBe('attention');
        expect(toasts.value).toHaveLength(1);
        const action = toasts.value[0].action;
        expect(action?.label).toBe('Clear filter');

        action?.onClick();

        expect(appliedThreadFilter.value.channels.size).toBe(ALL_CHANNELS.length);
        expect(drawerView.value).toBe('all');
    });
});
