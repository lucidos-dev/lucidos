import { computed } from '@preact/signals';
import { effectiveThreadStatus, getThreadDisplaySection, listsInOngoing, threadMap, threadIsBlocked, threadInReview, changeAwaitsApply, ONGOING_GROUPS, selectedOngoingGroup, ongoingFilterChoice, setOngoingFilterChoice, ONGOING_PARTS, isSplitOngoingGroup, type OngoingFilter, type OngoingGroup, type OngoingPart, type SplitOngoingGroup, type ThreadChannel } from '../../store/store';
import { visualStatusFor } from '../shared/threadVisualStatus';
import { byRecent, byCreated, recencyKey, reviewTier, isExcludedFromSections } from '../../store/thread-events';
import type { ThreadState } from '../../store/thread-events';
import type { DisplaySection } from '../../generated/thread-lifecycle';
import type { ThreadStatus } from '../../store/thread-events';
import { draftPresentThreadIds } from '../../store/composeDrafts';
import { threadPassesChannelFilter } from '../../store/threadFilter';


const SECTION_PRIORITY: Record<DisplaySection, number> = {
    current: 0,
    on_hold: 1,
    saved: 2,
    archive: 3,
};

function inLifecycleSections(t: ThreadState): boolean {
    return !isExcludedFromSections(t) && !t.meta.home;
}

const EMPTY_STRING_SET: ReadonlySet<string> = new Set();

function rootAncestorId(start: ThreadState, byId: ReadonlyMap<string, ThreadState>): string {
    let current = start;
    const visited = new Set<string>([current.meta.id]);
    while (current.meta.parentThreadId) {
        if (isHoldRoot(current)) return current.meta.id;
        const parent = byId.get(current.meta.parentThreadId);
        if (!parent || parent.meta.home) return current.meta.id;
        if (visited.has(parent.meta.id)) {
            let min = current.meta.id;
            for (const id of visited) if (id < min) min = id;
            return min;
        }
        visited.add(parent.meta.id);
        current = parent;
    }
    return current.meta.id;
}

export function isFamilyRoot(thread: ThreadState, byId: ReadonlyMap<string, ThreadState>): boolean {
    return rootAncestorId(thread, byId) === thread.meta.id;
}

function isHoldRoot(thread: ThreadState): boolean {
    return thread.meta.hold?.root_thread_id === thread.meta.id;
}

export type FamilyGraph = {
    byId: ReadonlyMap<string, ThreadState>;
    rootByThread: ReadonlyMap<string, string>;
};

export function computeFamilyGraph(threads: ThreadState[]): FamilyGraph {
    const byId = new Map<string, ThreadState>();
    for (const t of threads) byId.set(t.meta.id, t);
    const rootByThread = new Map<string, string>();
    for (const t of threads) {
        if (!inLifecycleSections(t)) continue;
        rootByThread.set(t.meta.id, rootAncestorId(t, byId));
    }
    return { byId, rootByThread };
}

export function filterByTopThread(
    threads: ThreadState[],
    graph: FamilyGraph,
    passesFilter: (top: ThreadState) => boolean,
    matchAnyMember = false,
): ThreadState[] {
    const passingRoots = new Set<string>();
    for (const t of threads) {
        const rootId = graph.rootByThread.get(t.meta.id) ?? t.meta.id;
        if (!matchAnyMember && rootId !== t.meta.id) continue;
        if (passesFilter(t)) passingRoots.add(rootId);
    }
    return threads.filter(t => {
        const rootId = graph.rootByThread.get(t.meta.id) ?? t.meta.id;
        return passingRoots.has(rootId);
    });
}

export type FamilyDecorations = {
    routedByThread: ReadonlyMap<string, DisplaySection>;
    liftedRoots: ReadonlySet<string>;
    archivedSubThreads: ReadonlySet<string>;
    hiddenArchivedThreads: ReadonlySet<string>;
    hiddenDirectChildCount: ReadonlyMap<string, number>;
};

function buildChildrenByParent(threads: ThreadState[]): Map<string, string[]> {
    const childrenByParent = new Map<string, string[]>();
    for (const t of threads) {
        if (!inLifecycleSections(t)) continue;
        const parentId = t.meta.parentThreadId;
        if (!parentId) continue;
        const siblings = childrenByParent.get(parentId);
        if (siblings) siblings.push(t.meta.id);
        else childrenByParent.set(parentId, [t.meta.id]);
    }
    return childrenByParent;
}

export function computeHiddenArchivedThreads(
    threads: ThreadState[],
    graph: FamilyGraph,
    familySections?: FamilySectionMap,
): { hidden: ReadonlySet<string>; hiddenDirectChildCount: ReadonlyMap<string, number> } {
    const familySection = familySections ?? computeFamilySections(threads, graph);
    const childrenByParent = buildChildrenByParent(threads);

    const fullyArchived = new Map<string, boolean>();
    const inProgress = new Set<string>();
    const isFullyArchived = (id: string): boolean => {
        const cached = fullyArchived.get(id);
        if (cached !== undefined) return cached;
        if (inProgress.has(id)) return false;
        inProgress.add(id);
        const thread = graph.byId.get(id);
        const ownArchive = !!thread && getThreadDisplaySection(thread) === 'archive';
        const children = childrenByParent.get(id) ?? [];
        const result = ownArchive && children.every(isFullyArchived);
        inProgress.delete(id);
        fullyArchived.set(id, result);
        return result;
    };

    const hidden = new Set<string>();
    const hiddenDirectChildCount = new Map<string, number>();
    for (const t of threads) {
        if (!inLifecycleSections(t)) continue;
        const id = t.meta.id;
        const root = graph.rootByThread.get(id);
        if (root === undefined || root === id) continue;
        if (familySection.get(root) === 'archive') continue;
        if (!isFullyArchived(id)) continue;
        hidden.add(id);
        const parentId = t.meta.parentThreadId!;
        hiddenDirectChildCount.set(parentId, (hiddenDirectChildCount.get(parentId) ?? 0) + 1);
    }
    return { hidden, hiddenDirectChildCount };
}

export function filterHiddenArchived(
    threads: ThreadState[],
    hidden: ReadonlySet<string>,
    revealed: ReadonlySet<string>,
    graph: FamilyGraph,
): ThreadState[] {
    if (hidden.size === 0) return threads;
    const resolved = new Map<string, boolean>();
    const isVisible = (id: string): boolean => {
        const cached = resolved.get(id);
        if (cached !== undefined) return cached;
        if (!hidden.has(id)) {
            resolved.set(id, true);
            return true;
        }
        resolved.set(id, false);
        const parentId = graph.byId.get(id)?.meta.parentThreadId;
        const result = !!parentId && revealed.has(parentId) && isVisible(parentId);
        resolved.set(id, result);
        return result;
    };
    return threads.filter(t => isVisible(t.meta.id));
}

export function holdRootCount(threads: readonly ThreadState[]): number {
    return threads.filter(isHoldRoot).length;
}

export function inboxThreadCount(threads: readonly ThreadState[]): number {
    return threads.filter(t => t.meta.section !== 'archived').length;
}

export function visibleChildrenCount(
    thread: ThreadState,
    decorations: Pick<FamilyDecorations, 'hiddenDirectChildCount'>,
): number {
    return thread.meta.totalChildrenCount - (decorations.hiddenDirectChildCount.get(thread.meta.id) ?? 0);
}

export function renderedFamilyRows(
    visible: readonly ThreadState[],
    collapsed: ReadonlySet<string>,
    graph: FamilyGraph,
    decorations: Pick<FamilyDecorations, 'hiddenDirectChildCount'>,
): NestedThread[] {
    const honoured = new Set<string>();
    for (const id of collapsed) {
        const thread = graph.byId.get(id);
        if (thread && visibleChildrenCount(thread, decorations) > 0) honoured.add(id);
    }
    return nestByParent(visible).filter(n => !hasCollapsedAncestor(n.thread.meta.id, honoured, graph));
}

export type FamilySectionMap = ReadonlyMap<string, DisplaySection>;

export function computeFamilySections(threads: ThreadState[], graph: FamilyGraph): Map<string, DisplaySection> {
    const familySection = new Map<string, DisplaySection>();
    for (const t of threads) {
        if (!inLifecycleSections(t)) continue;
        const sec = getThreadDisplaySection(t);
        const root = graph.rootByThread.get(t.meta.id);
        if (root === undefined) continue;
        const cur = familySection.get(root);
        if (cur === undefined || SECTION_PRIORITY[sec] < SECTION_PRIORITY[cur]) {
            familySection.set(root, sec);
        }
    }
    for (const root of familySection.keys()) {
        const rootThread = graph.byId.get(root);
        if (rootThread?.meta.saved) familySection.set(root, 'saved');
        else if (rootThread && isHoldRoot(rootThread)) familySection.set(root, 'on_hold');
    }
    return familySection;
}

export function computeFamilyDecorations(
    threads: ThreadState[],
    graph: FamilyGraph,
    familySections?: FamilySectionMap,
): FamilyDecorations {
    const familySection = familySections ?? computeFamilySections(threads, graph);
    const routedByThread = new Map<string, DisplaySection>();
    const liftedRoots = new Set<string>();
    const archivedSubThreads = new Set<string>();
    for (const t of threads) {
        if (!inLifecycleSections(t)) continue;
        const root = graph.rootByThread.get(t.meta.id);
        if (root === undefined) continue;
        const routed = familySection.get(root);
        if (routed === undefined) continue;
        routedByThread.set(t.meta.id, routed);
        if (t.meta.id !== root && routed !== 'archive' && getThreadDisplaySection(t) === 'archive') {
            archivedSubThreads.add(t.meta.id);
        }
        const rootThread = graph.byId.get(root);
        if (rootThread && getThreadDisplaySection(rootThread) !== routed) {
            liftedRoots.add(root);
        }
    }
    const { hidden: hiddenArchivedThreads, hiddenDirectChildCount } =
        computeHiddenArchivedThreads(threads, graph, familySection);
    return { routedByThread, liftedRoots, archivedSubThreads, hiddenArchivedThreads, hiddenDirectChildCount };
}

export function hasCollapsedAncestor(
    threadId: string,
    collapsed: ReadonlySet<string>,
    graph: FamilyGraph,
): boolean {
    for (const id of ancestorIds(threadId, graph)) {
        if (collapsed.has(id)) return true;
    }
    return false;
}

export function collapsedAncestorIds(
    threadId: string,
    collapsed: ReadonlySet<string>,
    graph: FamilyGraph,
): string[] {
    return [...ancestorIds(threadId, graph)].filter(id => collapsed.has(id));
}

function* ancestorIds(threadId: string, graph: FamilyGraph): Generator<string> {
    const visited = new Set<string>([threadId]);
    let parentId = graph.byId.get(threadId)?.meta.parentThreadId;
    while (parentId && !visited.has(parentId)) {
        yield parentId;
        visited.add(parentId);
        parentId = graph.byId.get(parentId)?.meta.parentThreadId;
    }
}

export function categorizeThreads(
    threads: ThreadState[],
    graph?: FamilyGraph,
    familySections?: FamilySectionMap,
): ThreadSections {
    const familyGraph = graph ?? computeFamilyGraph(threads);
    const { rootByThread } = familyGraph;
    const familySection = familySections ?? computeFamilySections(threads, familyGraph);
    const out: ThreadSections = {
        current: [], onHold: [], saved: [], archive: [],
        statusMap: new Map(),
    };

    for (const t of threads) {
        if (!inLifecycleSections(t)) continue;
        const status = effectiveThreadStatus(t);
        out.statusMap.set(t.meta.id, status);
        const display = familySection.get(rootByThread.get(t.meta.id)!)!;
        switch (display) {
            case 'current': out.current.push(t); break;
            case 'on_hold': out.onHold.push(t); break;
            case 'saved': out.saved.push(t); break;
            case 'archive': out.archive.push(t); break;
        }
    }
    return out;
}

export type DrawerCategorization = {
    categorized: ThreadSections;
    familyGraph: FamilyGraph;
    decorations: FamilyDecorations;
};

export function computeDrawerCategorization(
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

export type FamilyKeys = {
    recentKey: string;
};

export function orderedCurrentForReview(
    visible: ThreadState[],
    graph: FamilyGraph,
): ThreadState[] {
    const familySections = computeFamilySections(visible, graph);
    const current = categorizeThreads(visible, graph, familySections).current;
    const { hidden } = computeHiddenArchivedThreads(visible, graph, familySections);
    const visibleCurrent = filterHiddenArchived(current, hidden, EMPTY_STRING_SET, graph);
    visibleCurrent.sort(byCreated);
    return nestByParent(visibleCurrent).map(n => n.thread);
}

export function sortDrawerSections(
    sections: ThreadSections,
    familyKeys: ReadonlyMap<string, FamilyKeys>,
): void {
    const byFamilyRecent = (a: ThreadState, b: ThreadState) =>
        familyKeys.get(b.meta.id)!.recentKey.localeCompare(familyKeys.get(a.meta.id)!.recentKey);
    sections.current.sort(byCreated);
    sections.onHold.sort(byCreated);
    sections.saved.sort(byFamilyRecent);
    sections.archive.sort(byCreated);
}

export function computeFamilyKeys(
    threads: ThreadState[],
    graph?: FamilyGraph,
): Map<string, FamilyKeys> {
    const { rootByThread } = graph ?? computeFamilyGraph(threads);
    const perRoot = new Map<string, FamilyKeys>();
    for (const t of threads) {
        const root = rootByThread.get(t.meta.id);
        if (root === undefined) continue;
        const recent = recencyKey(t);
        const cur = perRoot.get(root);
        if (!cur) {
            perRoot.set(root, { recentKey: recent });
        } else {
            if (recent > cur.recentKey) cur.recentKey = recent;
        }
    }

    const out = new Map<string, FamilyKeys>();
    for (const t of threads) {
        const root = rootByThread.get(t.meta.id);
        if (root === undefined) continue;
        const keys = perRoot.get(root);
        if (keys !== undefined) out.set(t.meta.id, keys);
    }
    return out;
}

export function composingThreads(threads: ReadonlyMap<string, ThreadState>): ThreadState[] {
    const out: ThreadState[] = [];
    for (const t of threads.values()) {
        if (t.meta.state === 'composing' && threadHasUnsentDraft(t)) out.push(t);
    }
    out.sort(byRecent);
    return out;
}

export type NestedThread = { thread: ThreadState; depth: number };

export function depthStyle(depth: number): { [key: string]: string } {
    return { '--thread-depth': String(depth) };
}

export function nestByParent(threads: readonly ThreadState[]): NestedThread[] {
    const idSet = new Set<string>();
    for (const t of threads) idSet.add(t.meta.id);

    const childrenByParent = new Map<string, ThreadState[]>();
    const roots: ThreadState[] = [];
    for (const t of threads) {
        const parentId = t.meta.parentThreadId;
        if (parentId && idSet.has(parentId)) {
            const siblings = childrenByParent.get(parentId);
            if (siblings) siblings.push(t);
            else childrenByParent.set(parentId, [t]);
        } else {
            roots.push(t);
        }
    }

    const out: NestedThread[] = [];
    const visit = (thread: ThreadState, depth: number): void => {
        out.push({ thread, depth });
        const children = childrenByParent.get(thread.meta.id);
        if (children) for (const c of children) visit(c, depth + 1);
    };
    for (const r of roots) visit(r, 0);
    return out;
}

export function draftThreads(threads: ReadonlyMap<string, ThreadState>): ThreadState[] {
    const out: ThreadState[] = [];
    for (const t of threads.values()) {
        if (t.meta.state === 'discarded') continue;
        if (threadHasUnsentDraft(t)) out.push(t);
    }
    out.sort((a, b) => {
        const aNew = a.meta.state === 'composing' ? 0 : 1;
        const bNew = b.meta.state === 'composing' ? 0 : 1;
        if (aNew !== bNew) return aNew - bNew;
        return byRecent(a, b);
    });
    return out;
}

export function blockedThreads(threads: ReadonlyMap<string, ThreadState>): ThreadState[] {
    const out: ThreadState[] = [];
    for (const t of threads.values()) {
        if (threadIsBlocked(t)) out.push(t);
    }
    out.sort((a, b) => {
        const ta = reviewTier(a, effectiveThreadStatus(a));
        const tb = reviewTier(b, effectiveThreadStatus(b));
        if (ta !== tb) return ta - tb;
        return byRecent(a, b);
    });
    return out;
}

export function reviewThreads(threads: ReadonlyMap<string, ThreadState>): ThreadState[] {
    const out: ThreadState[] = [];
    for (const t of threads.values()) {
        if (threadInReview(t)) out.push(t);
    }
    out.sort(byRecent);
    return out;
}

export function threadIsInFlight(thread: ThreadState): boolean {
    if (!listsInOngoing(thread)) return false;
    if (threadIsBlocked(thread)) return false;
    const dot = visualStatusFor(effectiveThreadStatus(thread), { ...thread.meta, readRequested: false });
    return dot === 'running' || dot === 'waiting';
}

export function inFlightThreads(threads: ReadonlyMap<string, ThreadState>): NestedThread[] {
    const out: ThreadState[] = [];
    for (const t of threads.values()) {
        if (threadIsInFlight(t)) out.push(t);
    }
    out.sort(byCreated);
    return nestByParent(out);
}

export function ongoingPartOf(group: SplitOngoingGroup, row: NestedThread): OngoingPart {
    switch (group) {
        case 'review': return changeAwaitsApply(row.thread.meta) ? 'changes' : 'unread';
        case 'in-flight': return effectiveThreadStatus(row.thread) === 'running' ? 'running' : 'waiting';
    }
}

export function ongoingBreakdown(group: SplitOngoingGroup, rows: readonly NestedThread[]): { part: OngoingPart; count: number }[] {
    return ONGOING_PARTS[group].map(part => ({ part, count: rows.filter(n => ongoingPartOf(group, n) === part).length }));
}

export function ongoingFilterSteps(group: OngoingGroup, rows: readonly NestedThread[]): readonly OngoingFilter[] {
    if (!isSplitOngoingGroup(group)) return ['all'];
    const filled = ongoingBreakdown(group, rows).filter(({ count }) => count > 0).map(({ part }) => part);
    return filled.length > 1 ? ['all', ...filled] : ['all'];
}

export function filterOngoingRows(group: OngoingGroup, rows: NestedThread[], filter: OngoingFilter): NestedThread[] {
    if (filter === 'all' || !isSplitOngoingGroup(group)) return rows;
    const kept = rows.filter(n => ongoingPartOf(group, n) === filter);
    return group === 'in-flight' ? nestByParent(kept.map(n => n.thread)) : kept;
}

const flatRows = (threads: ThreadState[]): NestedThread[] => threads.map(thread => ({ thread, depth: 0 }));

export function ongoingGroupRows(threads: ReadonlyMap<string, ThreadState>, group: OngoingGroup): NestedThread[] {
    switch (group) {
        case 'blocked': return flatRows(blockedThreads(threads));
        case 'review': return flatRows(reviewThreads(threads));
        case 'in-flight': return inFlightThreads(threads);
        case 'drafts': return flatRows(draftThreads(threads));
    }
}

export type OngoingGroupList = { group: OngoingGroup; rows: NestedThread[] };

export const ongoingGroupLists = computed<OngoingGroupList[]>(() => ongoingGroups(threadMap.value, ONGOING_GROUPS));

const selectedGroupRows = computed<NestedThread[]>(() =>
    ongoingGroupLists.value.find(g => g.group === selectedOngoingGroup.value)?.rows ?? []);

const selectedFilterSteps = computed(() => ongoingFilterSteps(selectedOngoingGroup.value, selectedGroupRows.value));

export const ongoingFilter = computed<OngoingFilter>(() => {
    const choice = ongoingFilterChoice.value;
    return selectedFilterSteps.value.includes(choice) ? choice : 'all';
});

export function stepOngoingFilter(): void {
    const steps = selectedFilterSteps.value;
    setOngoingFilterChoice(steps[(steps.indexOf(ongoingFilter.value) + 1) % steps.length]);
}

export const selectedOngoingRows = computed<NestedThread[]>(() =>
    filterOngoingRows(selectedOngoingGroup.value, selectedGroupRows.value, ongoingFilter.value));

export function ongoingGroups(
    threads: ReadonlyMap<string, ThreadState>,
    groups: readonly OngoingGroup[],
): OngoingGroupList[] {
    return groups.map(group => ({ group, rows: ongoingGroupRows(threads, group) }));
}

export function ongoingGroupOnSwitch(
    stored: OngoingGroup,
    groups: readonly OngoingGroupList[],
    badged: boolean,
): OngoingGroup {
    if (badged) return 'blocked';
    const filled = groups.filter(g => g.rows.length > 0);
    if (filled.some(g => g.group === stored)) return stored;
    return filled[0]?.group ?? stored;
}

export type ThreadSections = {
    current: ThreadState[];
    onHold: ThreadState[];
    saved: ThreadState[];
    archive: ThreadState[];
    statusMap: Map<string, ThreadStatus>;
};

export function threadHasUnsentDraft(thread: ThreadState | undefined): boolean {
    if (!thread) return false;
    return draftPresentThreadIds.value.has(thread.meta.id);
}
