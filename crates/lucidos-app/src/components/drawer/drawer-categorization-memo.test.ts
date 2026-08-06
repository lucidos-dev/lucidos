/**
 * Equivalence guard for `computeDrawerCategorization` — the memoized drawer
 * categorization pass extracted from `ThreadList` (dev-ws input-lag fix,
 * 2026-06-24). The component now calls this one function inside a `useMemo`
 * keyed on its real inputs instead of inlining the seven-step pipeline on every
 * render. This test pins the load-bearing invariant: the composite MUST produce
 * a result deep-equal to the from-scratch sequence it replaced, for every input
 * — same section buckets, same family routing, same decorations, same status
 * map. (The individual steps' own behavior is covered by categorize-threads /
 * filter-by-top-thread / lifted-family / sort-drawer-sections / nest-by-parent.)
 */

import { describe, it, expect } from 'vitest';
import {
    computeDrawerCategorization,
    computeFamilyGraph,
    filterByTopThread,
    computeFamilySections,
    categorizeThreads,
    computeFamilyDecorations,
    computeFamilyKeys,
    sortDrawerSections,
    type DrawerCategorization,
} from './family-graph';
import { threadPassesChannelFilter } from '../../store/threadFilter';
import { ALL_CHANNELS, CODING_AGENT_CHANNEL, type ThreadChannel } from '../../store/store';
import type { ThreadState, ThreadMeta, ThreadStatus, ThreadComposeState } from '../../store/thread-events';
import type { ArchiveState } from '../../generated/thread-lifecycle';

type ThreadOpts = {
    parentId?: string;
    channel?: ThreadChannel;
    section?: ArchiveState;
    status?: ThreadStatus;
    saved?: boolean;
    repoId?: string;
    triggerId?: string;
    state?: ThreadComposeState;
    createdAt?: string;
};

function makeThread(id: string, opts: ThreadOpts = {}): ThreadState {
    const meta: ThreadMeta = {
        id,
        title: id,
        channel: opts.channel ?? 'chat',
        initiator: 'user',
        saved: opts.saved ?? false,
        createdAt: opts.createdAt ?? '2026-04-12T00:00:00Z',
        updatedAt: opts.createdAt ?? '2026-04-12T00:00:00Z',
        status: opts.status ?? 'idle',
        messageCount: 1,
        section: opts.section ?? 'inbox',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0,
        attentionDescendantCount: 0,
        codingAgentHasDiff: false,
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        lastRevivedAt: '',
        parentThreadId: opts.parentId,
        repoId: opts.repoId,
        triggerId: opts.triggerId,
        state: opts.state ?? 'active',
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

/** The seven-step pipeline exactly as `ThreadList` inlined it before the
 *  extraction — the oracle the composite must match. */
function fromScratch(
    threads: ThreadState[],
    channelFilter: ReadonlySet<ThreadChannel>,
    triggerSelection: ReadonlySet<string>,
    repoSelection: ReadonlySet<string>,
    appSelection: ReadonlySet<string>,
): DrawerCategorization {
    const subSelectionActive =
        triggerSelection.size > 0 || repoSelection.size > 0 || appSelection.size > 0;
    const familyGraph = computeFamilyGraph(threads);
    const allThreads = filterByTopThread(threads, familyGraph, t =>
        threadPassesChannelFilter(t, channelFilter, triggerSelection, repoSelection, appSelection),
        subSelectionActive,
    );
    const familySections = computeFamilySections(allThreads, familyGraph);
    const categorized = categorizeThreads(allThreads, familyGraph, familySections);
    const decorations = computeFamilyDecorations(allThreads, familyGraph, familySections);
    const familyKeys = computeFamilyKeys(allThreads, familyGraph);
    sortDrawerSections(categorized, familyKeys);
    return { categorized, familyGraph, decorations };
}

/** Order-sensitive, reference-free snapshot for deep comparison. */
function snapshot(r: DrawerCategorization) {
    return {
        current: r.categorized.current.map(t => t.meta.id),
        saved: r.categorized.saved.map(t => t.meta.id),
        archive: r.categorized.archive.map(t => t.meta.id),
        statusMap: [...r.categorized.statusMap.entries()].sort(),
        rootByThread: [...r.familyGraph.rootByThread.entries()].sort(),
        routedByThread: [...r.decorations.routedByThread.entries()].sort(),
        liftedRoots: [...r.decorations.liftedRoots].sort(),
        archivedSubThreads: [...r.decorations.archivedSubThreads].sort(),
    };
}

describe('computeDrawerCategorization equals the inlined from-scratch pipeline', () => {
    // A representative set exercising every routing path: a family whose archived
    // root lifts to Current via a running coding-agent child (lifted root); a
    // saved family; a trigger thread; an archived coding-agent thread carrying a
    // repoId (for the sub-selection any-member path); and an excluded composing
    // draft.
    const threads: ThreadState[] = [
        makeThread('root1', { channel: 'chat', section: 'archived', createdAt: '2026-04-10T00:00:00Z' }),
        makeThread('child1a', { parentId: 'root1', channel: CODING_AGENT_CHANNEL, status: 'running', section: 'inbox' }),
        makeThread('root2', { channel: 'chat', saved: true, createdAt: '2026-04-11T00:00:00Z' }),
        makeThread('child2a', { parentId: 'root2', channel: CODING_AGENT_CHANNEL, section: 'archived' }),
        makeThread('t3', { channel: 'trigger', section: 'inbox', triggerId: 'trig-x' }),
        makeThread('t4', { channel: CODING_AGENT_CHANNEL, section: 'archived', repoId: 'repo-x', createdAt: '2026-04-09T00:00:00Z' }),
        makeThread('draft', { state: 'composing' }),
    ];

    const allChannels: ReadonlySet<ThreadChannel> = new Set(ALL_CHANNELS);
    const noSel: ReadonlySet<string> = new Set();

    const cases: Array<{ name: string; filter: ReadonlySet<ThreadChannel>; trig: ReadonlySet<string>; repo: ReadonlySet<string>; app: ReadonlySet<string> }> = [
        { name: 'no filter, no sub-selection', filter: allChannels, trig: noSel, repo: noSel, app: noSel },
        { name: 'channel filter excludes trigger', filter: new Set([...ALL_CHANNELS].filter(c => c !== 'trigger') as ThreadChannel[]), trig: noSel, repo: noSel, app: noSel },
        { name: 'repo sub-selection (any-member)', filter: allChannels, trig: noSel, repo: new Set(['repo-x']), app: noSel },
        { name: 'trigger sub-selection (any-member)', filter: allChannels, trig: new Set(['trig-x']), repo: noSel, app: noSel },
        { name: 'empty channel filter (show nothing)', filter: new Set(), trig: noSel, repo: noSel, app: noSel },
    ];

    for (const c of cases) {
        it(c.name, () => {
            const composite = computeDrawerCategorization(threads, c.filter, c.trig, c.repo, c.app);
            const oracle = fromScratch(threads, c.filter, c.trig, c.repo, c.app);
            expect(snapshot(composite)).toEqual(snapshot(oracle));
        });
    }
});
