/**
 * The drawer's channel/trigger/repo filter applies at top-thread scope.
 * Before this, the filter ran per-thread: a chat top-thread spawning a coding-agent
 * sub-thread had its family row claim "1/1 sub-thread done" via
 * meta.totalChildrenCount while the filtered list silently dropped the
 * sub-thread — render and count diverged. Top-thread scope keeps the
 * conversation tree intact.
 */

import { describe, it, expect } from 'vitest';
import { computeFamilyGraph, filterByTopThread, threadPassesChannelFilter } from './ThreadDrawer';
import { ALL_CHANNELS, CODING_AGENT_CHANNEL, type ThreadChannel } from '../../store/store';
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

const ALL = new Set<ThreadChannel>(ALL_CHANNELS);
const NO_TRIGGERS: ReadonlySet<string> = new Set();
const NO_REPOS: ReadonlySet<string> = new Set();
const NO_APPS: ReadonlySet<string> = new Set();

describe('filterByTopThread', () => {
    it('keeps the whole family when the top-thread passes the channel filter', () => {
        const parent = makeThread('p', { channel: 'chat' });
        const child = makeThread('c', { channel: CODING_AGENT_CHANNEL, parentId: 'p' });
        const all = [parent, child];
        const graph = computeFamilyGraph(all);
        const filter = new Set<ThreadChannel>(['chat']);
        const result = filterByTopThread(all, graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, NO_REPOS, NO_APPS),
        );
        expect(result.map(t => t.meta.id).sort()).toEqual(['c', 'p']);
    });

    it('hides the whole family when the top-thread fails the channel filter', () => {
        // Symmetric trade-off: filtering to Coding Agent won't surface a coding-agent sub-thread
        // buried under a chat top-thread. Finding that specific sub-thread is
        // search's job, not the channel filter's.
        const parent = makeThread('p', { channel: 'chat' });
        const child = makeThread('c', { channel: CODING_AGENT_CHANNEL, parentId: 'p' });
        const graph = computeFamilyGraph([parent, child]);
        const filter = new Set<ThreadChannel>([CODING_AGENT_CHANNEL]);
        const result = filterByTopThread([parent, child], graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, NO_REPOS, NO_APPS),
        );
        expect(result).toEqual([]);
    });

    it('treats an orphan child (parent missing from input) as its own top-thread', () => {
        // Pagination can leave a child loaded after its parent has been
        // evicted. The orphan has no top-thread to inherit from, so the
        // filter applies directly to it — same effect as if it were a top.
        const orphan = makeThread('o', { channel: CODING_AGENT_CHANNEL, parentId: 'missing' });
        const graph = computeFamilyGraph([orphan]);
        const filter = new Set<ThreadChannel>([CODING_AGENT_CHANNEL]);
        const result = filterByTopThread([orphan], graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, NO_REPOS, NO_APPS),
        );
        expect(result.map(t => t.meta.id)).toEqual(['o']);
    });

    it('cascades trigger/repo sub-selection through the top-thread to the family', () => {
        // Repo selection applies to the top-thread's repoId. A coding-agent top-thread
        // in repo X spawning a sub-thread in repo Y still pulls the whole
        // family in when filtering to repo X — top-thread is the gate.
        const parent = makeThread('p', { channel: CODING_AGENT_CHANNEL, repoId: 'X' });
        const child = makeThread('c', { channel: CODING_AGENT_CHANNEL, repoId: 'Y', parentId: 'p' });
        const graph = computeFamilyGraph([parent, child]);
        const repos = new Set(['X']);
        const result = filterByTopThread([parent, child], graph, t =>
            threadPassesChannelFilter(t, ALL, NO_TRIGGERS, repos, NO_APPS),
        );
        expect(result.map(t => t.meta.id).sort()).toEqual(['c', 'p']);
    });

    it('surfaces a coding-agent sub-thread matching a repo sub-selection even when its chat top-thread is channel-filtered out', () => {
        // The reported bug: a (deleted) repo's coding-agent threads are sub-threads of
        // chat parents. Filtering to Coding Agent + that repo dropped the whole
        // family because the chat top-thread fails the coding-agent channel
        // check. With a sub-selection active (matchAnyMember=true), a family is
        // included if ANY member matches — the coding-agent thread surfaces under its
        // chat parent for context.
        const parent = makeThread('p', { channel: 'chat' });
        const child = makeThread('c', { channel: CODING_AGENT_CHANNEL, repoId: 'X', parentId: 'p' });
        const graph = computeFamilyGraph([parent, child]);
        const filter = new Set<ThreadChannel>([CODING_AGENT_CHANNEL]);
        const repos = new Set(['X']);
        const result = filterByTopThread([parent, child], graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, repos, NO_APPS),
            true,
        );
        expect(result.map(t => t.meta.id).sort()).toEqual(['c', 'p']);
    });

    it('surfaces an app coding-agent sub-thread matching an app sub-selection under a chat parent', () => {
        const parent = makeThread('p', { channel: 'chat' });
        const child = makeThread('c', {
            channel: CODING_AGENT_CHANNEL,
            codingAgentKind: 'app',
            codingAgentFolder: '/ws/data/apps/habit-tracker',
            parentId: 'p',
        });
        const graph = computeFamilyGraph([parent, child]);
        const filter = new Set<ThreadChannel>([CODING_AGENT_CHANNEL]);
        const apps = new Set(['habit-tracker']);
        const result = filterByTopThread([parent, child], graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, NO_REPOS, apps),
            true,
        );
        expect(result.map(t => t.meta.id).sort()).toEqual(['c', 'p']);
    });

    it('does not pull in an unrelated family when no member matches the sub-selection', () => {
        // matchAnyMember must not become "show everything": a family whose only
        // coding-agent thread is in repo Y stays hidden when filtering to repo X.
        const parent = makeThread('p', { channel: 'chat' });
        const child = makeThread('c', { channel: CODING_AGENT_CHANNEL, repoId: 'Y', parentId: 'p' });
        const graph = computeFamilyGraph([parent, child]);
        const filter = new Set<ThreadChannel>([CODING_AGENT_CHANNEL]);
        const repos = new Set(['X']);
        const result = filterByTopThread([parent, child], graph, t =>
            threadPassesChannelFilter(t, filter, NO_TRIGGERS, repos, NO_APPS),
            true,
        );
        expect(result).toEqual([]);
    });

    it('still hides a coding-agent sub-thread under a chat parent for a channel-only filter (no sub-selection)', () => {
        // Regression guard: matchAnyMember=false preserves the deliberate
        // top-thread trade-off — channel-only filtering does NOT surface coding-agent
        // sub-threads buried under a chat top-thread (that's search's job).
        const parent = makeThread('p', { channel: 'chat' });
        const child = makeThread('c', { channel: CODING_AGENT_CHANNEL, parentId: 'p' });
        const graph = computeFamilyGraph([parent, child]);
        const filter = new Set<ThreadChannel>([CODING_AGENT_CHANNEL]);
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

    it('rejects a coding-agent thread when the channel filter excludes Coding Agent', () => {
        const t = makeThread('t', { channel: CODING_AGENT_CHANNEL });
        expect(threadPassesChannelFilter(t, new Set(['chat']), NO_TRIGGERS, NO_REPOS, NO_APPS)).toBe(false);
    });

    it('honors trigger sub-selection within the trigger channel', () => {
        const matching = makeThread('m', { channel: 'trigger', triggerId: 'cron-1' });
        const other = makeThread('o', { channel: 'trigger', triggerId: 'cron-2' });
        const triggers = new Set(['cron-1']);
        expect(threadPassesChannelFilter(matching, ALL, triggers, NO_REPOS, NO_APPS)).toBe(true);
        expect(threadPassesChannelFilter(other, ALL, triggers, NO_REPOS, NO_APPS)).toBe(false);
    });

    it('honors repo sub-selection within the coding-agent channel', () => {
        const matching = makeThread('m', { channel: CODING_AGENT_CHANNEL, repoId: 'X' });
        const other = makeThread('o', { channel: CODING_AGENT_CHANNEL, repoId: 'Y' });
        const repos = new Set(['X']);
        expect(threadPassesChannelFilter(matching, ALL, NO_TRIGGERS, repos, NO_APPS)).toBe(true);
        expect(threadPassesChannelFilter(other, ALL, NO_TRIGGERS, repos, NO_APPS)).toBe(false);
    });

    it('honors app sub-selection within the coding-agent channel', () => {
        const matching = makeThread('m', {
            channel: CODING_AGENT_CHANNEL,
            codingAgentKind: 'app',
            codingAgentFolder: '/ws/data/apps/habit-tracker',
        });
        const other = makeThread('o', {
            channel: CODING_AGENT_CHANNEL,
            codingAgentKind: 'app',
            codingAgentFolder: '/ws/data/apps/demo-director',
        });
        const apps = new Set(['habit-tracker']);
        expect(threadPassesChannelFilter(matching, ALL, NO_TRIGGERS, NO_REPOS, apps)).toBe(true);
        expect(threadPassesChannelFilter(other, ALL, NO_TRIGGERS, NO_REPOS, apps)).toBe(false);
    });

    it('unions repo and app sub-selections within the coding-agent channel', () => {
        const repoThread = makeThread('r', { channel: CODING_AGENT_CHANNEL, repoId: 'X' });
        const appThread = makeThread('a', {
            channel: CODING_AGENT_CHANNEL,
            codingAgentKind: 'app',
            codingAgentFolder: '/ws/data/apps/habit-tracker',
        });
        const unrelated = makeThread('u', { channel: CODING_AGENT_CHANNEL, repoId: 'Z' });
        const repos = new Set(['X']);
        const apps = new Set(['habit-tracker']);
        expect(threadPassesChannelFilter(repoThread, ALL, NO_TRIGGERS, repos, apps)).toBe(true);
        expect(threadPassesChannelFilter(appThread, ALL, NO_TRIGGERS, repos, apps)).toBe(true);
        expect(threadPassesChannelFilter(unrelated, ALL, NO_TRIGGERS, repos, apps)).toBe(false);
    });

    it('passes all coding-agent threads when both repo and app selections are empty', () => {
        const appThread = makeThread('a', {
            channel: CODING_AGENT_CHANNEL,
            codingAgentKind: 'app',
            codingAgentFolder: '/ws/data/apps/habit-tracker',
        });
        const repoThread = makeThread('r', { channel: CODING_AGENT_CHANNEL, repoId: 'X' });
        expect(threadPassesChannelFilter(appThread, ALL, NO_TRIGGERS, NO_REPOS, NO_APPS)).toBe(true);
        expect(threadPassesChannelFilter(repoThread, ALL, NO_TRIGGERS, NO_REPOS, NO_APPS)).toBe(true);
    });
});
