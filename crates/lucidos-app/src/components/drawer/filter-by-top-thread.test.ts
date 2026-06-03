/**
 * The drawer's channel/trigger/repo filter applies at top-thread scope.
 * Before this, the filter ran per-thread: a chat top-thread spawning a CC
 * sub-thread had its family row claim "1/1 sub-thread done" via
 * meta.totalChildrenCount while the filtered list silently dropped the
 * sub-thread — render and count diverged. Top-thread scope keeps the
 * conversation tree intact.
 */

import { describe, it, expect } from 'vitest';
import { computeFamilyGraph, filterByTopThread, threadPassesChannelFilter } from './ThreadDrawer';
import { ALL_CHANNELS, type ThreadChannel } from '../../store/store';
import type { ThreadState, ThreadMeta } from '../../store/thread-events';

function makeThread(
    id: string,
    opts: {
        parentId?: string;
        channel?: ThreadChannel;
        triggerId?: string;
        repoId?: string;
        codingAgentKind?: 'lucidos' | 'app' | 'external';
        codingAgentFolder?: string;
    } = {},
): ThreadState {
    const meta: ThreadMeta = {
        id,
        title: id,
        channel: opts.channel ?? 'chat',
        initiator: 'user',
        saved: false,
        createdAt: '2026-04-12T00:00:00Z',
        updatedAt: '2026-04-12T00:00:00Z',
        status: 'idle',
        messageCount: 1,
        section: 'inbox',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentHasDiff: false,
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        lastRevivedAt: '',
        parentThreadId: opts.parentId,
        triggerId: opts.triggerId,
        repoId: opts.repoId,
        codingAgentKind: opts.codingAgentKind,
        codingAgentFolder: opts.codingAgentFolder,
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

const ALL = new Set<ThreadChannel>(ALL_CHANNELS);
const NO_TRIGGERS: ReadonlySet<string> = new Set();
const NO_REPOS: ReadonlySet<string> = new Set();
const NO_APPS: ReadonlySet<string> = new Set();

describe('filterByTopThread', () => {
    it('keeps the whole family when the top-thread passes the channel filter', () => {
        const parent = makeThread('p', { channel: 'chat' });
        const child = makeThread('c', { channel: 'claude_code', parentId: 'p' });
        const all = [parent, child];
        const graph = computeFamilyGraph(all);
        const filter = new Set<ThreadChannel>(['chat']);
        const result = filterByTopThread(all, graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, NO_REPOS, NO_APPS),
        );
        expect(result.map(t => t.meta.id).sort()).toEqual(['c', 'p']);
    });

    it('hides the whole family when the top-thread fails the channel filter', () => {
        // Symmetric trade-off: filtering to CC won't surface a CC sub-thread
        // buried under a chat top-thread. Finding that specific sub-thread is
        // search's job, not the channel filter's.
        const parent = makeThread('p', { channel: 'chat' });
        const child = makeThread('c', { channel: 'claude_code', parentId: 'p' });
        const graph = computeFamilyGraph([parent, child]);
        const filter = new Set<ThreadChannel>(['claude_code']);
        const result = filterByTopThread([parent, child], graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, NO_REPOS, NO_APPS),
        );
        expect(result).toEqual([]);
    });

    it('treats an orphan child (parent missing from input) as its own top-thread', () => {
        // Pagination can leave a child loaded after its parent has been
        // evicted. The orphan has no top-thread to inherit from, so the
        // filter applies directly to it — same effect as if it were a top.
        const orphan = makeThread('o', { channel: 'claude_code', parentId: 'missing' });
        const graph = computeFamilyGraph([orphan]);
        const filter = new Set<ThreadChannel>(['claude_code']);
        const result = filterByTopThread([orphan], graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, NO_REPOS, NO_APPS),
        );
        expect(result.map(t => t.meta.id)).toEqual(['o']);
    });

    it('cascades trigger/repo sub-selection through the top-thread to the family', () => {
        // Repo selection applies to the top-thread's repoId. A CC top-thread
        // in repo X spawning a sub-thread in repo Y still pulls the whole
        // family in when filtering to repo X — top-thread is the gate.
        const parent = makeThread('p', { channel: 'claude_code', repoId: 'X' });
        const child = makeThread('c', { channel: 'claude_code', repoId: 'Y', parentId: 'p' });
        const graph = computeFamilyGraph([parent, child]);
        const repos = new Set(['X']);
        const result = filterByTopThread([parent, child], graph, t =>
            threadPassesChannelFilter(t, ALL, NO_TRIGGERS, repos, NO_APPS),
        );
        expect(result.map(t => t.meta.id).sort()).toEqual(['c', 'p']);
    });

    it('surfaces a CC sub-thread matching a repo sub-selection even when its chat top-thread is channel-filtered out', () => {
        // The reported bug: a (deleted) repo's CC threads are sub-threads of
        // chat parents. Filtering to claude_code + that repo dropped the whole
        // family because the chat top-thread fails the claude_code channel
        // check. With a sub-selection active (matchAnyMember=true), a family is
        // included if ANY member matches — the CC thread surfaces under its
        // chat parent for context.
        const parent = makeThread('p', { channel: 'chat' });
        const child = makeThread('c', { channel: 'claude_code', repoId: 'X', parentId: 'p' });
        const graph = computeFamilyGraph([parent, child]);
        const filter = new Set<ThreadChannel>(['claude_code']);
        const repos = new Set(['X']);
        const result = filterByTopThread([parent, child], graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, repos, NO_APPS),
            true,
        );
        expect(result.map(t => t.meta.id).sort()).toEqual(['c', 'p']);
    });

    it('surfaces a CC app sub-thread matching an app sub-selection under a chat parent', () => {
        const parent = makeThread('p', { channel: 'chat' });
        const child = makeThread('c', {
            channel: 'claude_code',
            codingAgentKind: 'app',
            codingAgentFolder: '/ws/data/apps/momentum',
            parentId: 'p',
        });
        const graph = computeFamilyGraph([parent, child]);
        const filter = new Set<ThreadChannel>(['claude_code']);
        const apps = new Set(['momentum']);
        const result = filterByTopThread([parent, child], graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, NO_REPOS, apps),
            true,
        );
        expect(result.map(t => t.meta.id).sort()).toEqual(['c', 'p']);
    });

    it('does not pull in an unrelated family when no member matches the sub-selection', () => {
        // matchAnyMember must not become "show everything": a family whose only
        // CC thread is in repo Y stays hidden when filtering to repo X.
        const parent = makeThread('p', { channel: 'chat' });
        const child = makeThread('c', { channel: 'claude_code', repoId: 'Y', parentId: 'p' });
        const graph = computeFamilyGraph([parent, child]);
        const filter = new Set<ThreadChannel>(['claude_code']);
        const repos = new Set(['X']);
        const result = filterByTopThread([parent, child], graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, repos, NO_APPS),
            true,
        );
        expect(result).toEqual([]);
    });

    it('still hides a CC sub-thread under a chat parent for a channel-only filter (no sub-selection)', () => {
        // Regression guard: matchAnyMember=false preserves the deliberate
        // top-thread trade-off — channel-only filtering does NOT surface CC
        // sub-threads buried under a chat top-thread (that's search's job).
        const parent = makeThread('p', { channel: 'chat' });
        const child = makeThread('c', { channel: 'claude_code', parentId: 'p' });
        const graph = computeFamilyGraph([parent, child]);
        const filter = new Set<ThreadChannel>(['claude_code']);
        const result = filterByTopThread([parent, child], graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, NO_REPOS, NO_APPS),
            false,
        );
        expect(result).toEqual([]);
    });
});

describe('threadPassesChannelFilter', () => {
    it('passes a chat thread when chat is in the channel filter', () => {
        const t = makeThread('t', { channel: 'chat' });
        expect(threadPassesChannelFilter(t, new Set(['chat']), NO_TRIGGERS, NO_REPOS, NO_APPS)).toBe(true);
    });

    it('rejects a CC thread when the channel filter excludes claude_code', () => {
        const t = makeThread('t', { channel: 'claude_code' });
        expect(threadPassesChannelFilter(t, new Set(['chat']), NO_TRIGGERS, NO_REPOS, NO_APPS)).toBe(false);
    });

    it('honors trigger sub-selection within the trigger channel', () => {
        const matching = makeThread('m', { channel: 'trigger', triggerId: 'cron-1' });
        const other = makeThread('o', { channel: 'trigger', triggerId: 'cron-2' });
        const triggers = new Set(['cron-1']);
        expect(threadPassesChannelFilter(matching, ALL, triggers, NO_REPOS, NO_APPS)).toBe(true);
        expect(threadPassesChannelFilter(other, ALL, triggers, NO_REPOS, NO_APPS)).toBe(false);
    });

    it('honors repo sub-selection within the claude_code channel', () => {
        const matching = makeThread('m', { channel: 'claude_code', repoId: 'X' });
        const other = makeThread('o', { channel: 'claude_code', repoId: 'Y' });
        const repos = new Set(['X']);
        expect(threadPassesChannelFilter(matching, ALL, NO_TRIGGERS, repos, NO_APPS)).toBe(true);
        expect(threadPassesChannelFilter(other, ALL, NO_TRIGGERS, repos, NO_APPS)).toBe(false);
    });

    it('honors app sub-selection within the claude_code channel', () => {
        const matching = makeThread('m', {
            channel: 'claude_code',
            codingAgentKind: 'app',
            codingAgentFolder: '/ws/data/apps/momentum',
        });
        const other = makeThread('o', {
            channel: 'claude_code',
            codingAgentKind: 'app',
            codingAgentFolder: '/ws/data/apps/habit-tracker',
        });
        const apps = new Set(['momentum']);
        expect(threadPassesChannelFilter(matching, ALL, NO_TRIGGERS, NO_REPOS, apps)).toBe(true);
        expect(threadPassesChannelFilter(other, ALL, NO_TRIGGERS, NO_REPOS, apps)).toBe(false);
    });

    it('unions repo and app sub-selections within claude_code', () => {
        const repoThread = makeThread('r', { channel: 'claude_code', repoId: 'X' });
        const appThread = makeThread('a', {
            channel: 'claude_code',
            codingAgentKind: 'app',
            codingAgentFolder: '/ws/data/apps/momentum',
        });
        const unrelated = makeThread('u', { channel: 'claude_code', repoId: 'Z' });
        const repos = new Set(['X']);
        const apps = new Set(['momentum']);
        expect(threadPassesChannelFilter(repoThread, ALL, NO_TRIGGERS, repos, apps)).toBe(true);
        expect(threadPassesChannelFilter(appThread, ALL, NO_TRIGGERS, repos, apps)).toBe(true);
        expect(threadPassesChannelFilter(unrelated, ALL, NO_TRIGGERS, repos, apps)).toBe(false);
    });

    it('passes all CC threads when both repo and app selections are empty', () => {
        const appThread = makeThread('a', {
            channel: 'claude_code',
            codingAgentKind: 'app',
            codingAgentFolder: '/ws/data/apps/momentum',
        });
        const repoThread = makeThread('r', { channel: 'claude_code', repoId: 'X' });
        expect(threadPassesChannelFilter(appThread, ALL, NO_TRIGGERS, NO_REPOS, NO_APPS)).toBe(true);
        expect(threadPassesChannelFilter(repoThread, ALL, NO_TRIGGERS, NO_REPOS, NO_APPS)).toBe(true);
    });
});
