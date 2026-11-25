import { LucidosGlyph } from '../shared/LucidosMark';
import type { ComponentChildren, RefObject } from 'preact';
import { useRef, useEffect, useLayoutEffect, useCallback, useMemo } from 'preact/hooks';
import { memo } from 'preact/compat';
import { batch, computed, effect, signal, useSignal } from '@preact/signals';
import { threadMap, focusedThreadId, threadsLoaded, effectiveThreadStatus, getThreadDisplaySection, threadSearchQuery, threadSearchResults, threadHasMore, threadLoadingMore, archiveThreadCount, drawerGrouping, setDrawerGrouping, selectedOngoingGroup, setSelectedOngoingGroup, ongoingFilterChoice, setOngoingFilterChoice, isSplitOngoingGroup, type OngoingFilter, type OngoingPart, type SplitOngoingGroup, ONGOING_GROUPS, type OngoingGroup, repositories, focusedPane, scaledDurationMs, showToast, ALL_CHANNELS, clearThreadFilter } from '../../store/store';
import { drawnDrawerShown, drawnDrawerWidth } from '../layout/drawnLayout';
import { appliedThreadFilter, type ThreadFilterSelection } from '../../store/appliedThreadFilter';
import { resolveScope, resolveCodingAgent } from '../../store/composeSelections';
import { composeDraftContextName } from '../../store/composeDestination';
import { threadPassesChannelFilter } from '../../store/threadFilter';
import { threadFilterPanelOpen, closeThreadFilterPanel } from '../../store/threadFilterPanel';
import { ThreadFilterCover } from './ThreadFilterCover';
import { LeavingViewSnapshot, type LeavingDrawing } from './LeavingViewDrawing';
import { focusPane, showThreadList } from '../../store/actions/pane';
import { focusThread } from '../../store/actions/threads';
import { archiveAllPhase, handleArchiveAll } from '../../store/actions/archive-all';
import { landingGroupFor, openListedThread, openOngoingThread } from '../../store/actions/event-navigation';
import { preferencesSettled } from '../../store/actions/preferences';
import { loadOlderThreads, reloadAfterFilterChange, filterChangedSinceLoad, ensureThreadInMap, loadThreadEvents } from '../../store/actions/thread-loading';
import { ThreadStatusIcon } from '../shared/ThreadStatusIcon';
import { ThreadIcon } from '../shared/ThreadIcon';
import { visualStatusFor, type VisualStatus } from '../shared/threadVisualStatus';
import { PinThreadButton } from '../shared/PinThreadButton';
import { ThreadOverflowMenu } from '../shared/ThreadOverflowMenu';
import { DraftOverflowMenu } from '../shared/DraftOverflowMenu';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { SectionHeaderContent } from '../shared/SectionHeader';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { applyNavFocus } from '../shared/focusMarker';
import type { ThreadState, ThreadStatus } from '../../store/thread-events';
import { sameCodingAgents } from '../../store/thread-events/thread-meta';
import { getDraft } from '../../store/composeDrafts';
import type { DisplaySection } from '../../generated/thread-lifecycle';
import { formatChannel, formatThreadChannelLabel } from '../../utils/formatChannel';
import type { CodingAgent } from '../../api/types';
import { threadContextName, type ThreadContextFields } from './threadContextName';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { useFlipTransitions, type FlipSection } from '../../hooks/useFlipAnimation';
import { useDelayedFlag, useDelayedLoading, useLingeringFlag } from '../../hooks/useDelayedLoading';
import { PANE_TRANSITION_MS } from '../layout/splitHelpers';
import { readSavedScroll, useScrollMemory } from '../../hooks/useScrollMemory';
import { forgetSavedScroll } from '../../store/savedScroll';
import { useRowActionsGesture } from '../shared/useRowActionsGesture';
import { RowWindowContext, WindowedRows, replanDrawerRowWindow, revealDrawerRow, rowWindowIsMounted, useRowWindowController } from './WindowedRows';
import { getRemPx } from '../../utils/dom';
import { clearStackedMark, placeStackedMark } from './agentMarkLift';
import { isMobile } from '../../utils/viewport';
import { scrollBehavior } from '../../utils/motion';
import type { ThreadSearchResult } from '../../api/threads';
import { FolderIcon, PinIcon, CurrentIcon, HoldIcon, ArchiveIcon, DraftsIcon, BlockedIcon, ReviewIcon, InFlightIcon, SearchIcon, ChevronRightIcon, ClaudeIcon, CodexIcon, LucidosMarkIcon } from '../shared/icons';
import { CategoryIcon } from '../shared/CategoryIcon';
import type { ComponentType, VNode } from 'preact';
import { customFolderSectionKey, knownFolderOf, splitByCustomFolder, threadFolders, type CustomFolderSectionKey, type ThreadFolder } from '../../store/threadFolders';
import { CustomFolderMenu, NewFolderButton } from './CustomFolderControls';

export { threadPassesChannelFilter };

export const THREAD_DRAWER_SECTION_ORDER: readonly DisplaySection[] = ['saved', 'on_hold', 'current', 'archive'];

function formatCreatedTimestamp(createdAt: string | undefined): string {
    if (!createdAt) return '';
    const date = new Date(createdAt);
    if (Number.isNaN(date.getTime())) return '';
    return formatMessageTimestamp(createdAt);
}

function SectionPinIcon({ size }: { size?: string }) {
    return <PinIcon size={size} className="section-icon-pin" />;
}

export const SECTION_META: Record<DisplaySection, { title: string; Icon: ComponentType<{ size?: string }> }> = {
    saved: { title: 'Pinned', Icon: SectionPinIcon },
    on_hold: { title: 'On hold', Icon: HoldIcon },
    current: { title: 'Current', Icon: CurrentIcon },
    archive: { title: 'Archive', Icon: ArchiveIcon },
};

type DrawerSectionRows = {
    name: DrawerSectionKey;
    folder?: ThreadFolder;
    count: number;
    threads: NestedThread[];
    hasRunning: boolean;
};

function sectionAction(s: DrawerSectionRows): ComponentChildren {
    if (s.folder) return <CustomFolderMenu folder={s.folder} />;
    if (s.name === 'saved') return <NewFolderButton />;
    if (s.name === 'current' && s.count > 0) return <ArchiveAllButton />;
    return undefined;
}

export function isThreadRowTarget(target: EventTarget | null): boolean {
    return !!(target as Element | null)?.closest?.('[data-thread-nav]');
}

export function handleDrawerPointerDown(target: EventTarget | null): void {
    if (isThreadRowTarget(target)) return;
    focusPane('drawer');
}

export type DrawerSectionKey = DisplaySection | CustomFolderSectionKey;

export type DrawerNavNode =
    | { kind: 'section'; sectionKey: DrawerSectionKey }
    | { kind: 'tile'; group: OngoingGroup; selected: boolean }
    | {
          kind: 'thread';
          id: string;
          depth: number;
          parentId: string | null;
          hasChildren: boolean;
          hiddenArchivedCount?: number;
          sectionKey: DrawerSectionKey | null;
          group?: OngoingGroup;
      };

const SECTION_KEY_PREFIX = '__section_';
export function sectionNavKey(sectionKey: string): string {
    return `${SECTION_KEY_PREFIX}${sectionKey}`;
}
function isSectionNavKey(key: string): boolean {
    return key.startsWith(SECTION_KEY_PREFIX);
}
export function ongoingTileNavKey(group: OngoingGroup): string {
    return sectionNavKey(`ongoing-${group}`);
}
export function nodeKey(node: DrawerNavNode): string {
    if (node.kind === 'section') return sectionNavKey(node.sectionKey);
    if (node.kind === 'tile') return ongoingTileNavKey(node.group);
    return node.id;
}

export function navKeyDomId(key: string | null): string | undefined {
    return key ? `drawer-nav-${key}` : undefined;
}

function flatThreadNodes(ids: readonly string[], group?: OngoingGroup): DrawerNavNode[] {
    return ids.map((id): DrawerNavNode => ({
        kind: 'thread', id, depth: 0, parentId: null, hasChildren: false, sectionKey: null, group,
    }));
}

const highlightedKey = signal<string | null>(null);

const highlightedRowKey = computed(() => {
    const key = highlightedKey.value;
    return key && !isSectionNavKey(key) ? key : null;
});

const navNodes = signal<DrawerNavNode[]>([]);

function usePublishedNavNodes(build: () => DrawerNavNode[], key: string): void {
    useEffect(() => {
        navNodes.value = build();
        return () => { navNodes.value = []; };
    }, [key]);
}

export function selectHighlighted() {
    const key = highlightedKey.value;
    if (!key) return;
    const node = navNodes.value.find(n => nodeKey(n) === key);
    if (node?.kind === 'section') toggleSectionCollapse(node.sectionKey);
    else if (node?.kind === 'tile') pressOngoingTile(node.group);
    else if (!isSectionNavKey(key)) openKeyedThread(key);
}

function pressOngoingTile(group: OngoingGroup): void {
    if (isSplitOngoingGroup(group) && selectedOngoingGroup.value === group) stepOngoingFilter();
    else setSelectedOngoingGroup(group);
}

function openKeyedThread(id: string): void {
    const searchResult = threadSearchResults.value;
    if (threadSearchQuery.value.trim().length > 0 && searchResult.status === 'loaded') {
        const match = searchResult.data.find((r: ThreadSearchResult) => r.thread_id === id);
        if (match) void ensureThreadInMap(match);
    }
    openListedThread(id);
}

export function stepThreadInList(
    nodes: readonly DrawerNavNode[],
    currentId: string | null,
    delta: 1 | -1,
): string | null {
    const threads = nodes.filter((n): n is Extract<DrawerNavNode, { kind: 'thread' }> => n.kind === 'thread');
    const current = threads.find((n) => n.id === currentId);
    if (!current) {
        const edge = delta > 0 ? threads[0] : threads[threads.length - 1];
        return edge?.id ?? null;
    }
    const peers = threads.filter((n) => n.sectionKey === current.sectionKey);
    return peers[peers.indexOf(current) + delta]?.id ?? null;
}

export function stepFocusedThreadInList(delta: 1 | -1): void {
    if (navNodes.value.length === 0 && !isMobile()) showThreadList();
    whenNavNodesPublished(() => {
        const id = stepThreadInList(navNodes.value, focusedThreadId.value, delta);
        if (!id) return;
        setHighlight(id);
        openKeyedThread(id);
    });
}

export function openHighlightedThreadActions(): boolean {
    if (focusedPane.value !== 'drawer') return false;
    const key = highlightedKey.value;
    if (!key || isSectionNavKey(key)) return false;
    const trigger = document.querySelector<HTMLElement>(
        `[data-thread-nav="${key}"] button[aria-haspopup="menu"]`,
    );
    if (!trigger) return false;
    trigger.click();
    return true;
}

export function toggleFocusedThreadFamily(): void {
    const id = focusedThreadId.value;
    if (!id) return;
    const thread = threadMap.value.get(id);
    if (!thread || thread.meta.totalChildrenCount <= 0) return;
    toggleFamilyCollapse(id);
}

export interface NavCollapseState {
    sectionCollapsed: (sectionKey: string) => boolean;
    familyCollapsed: (threadId: string) => boolean;
    archivedRevealed: (threadId: string) => boolean;
}

export type NavCollapseAction =
    | { type: 'none' }
    | { type: 'collapseSection'; sectionKey: string }
    | { type: 'expandSection'; sectionKey: string }
    | { type: 'collapseFamily'; threadId: string; focusKey: string }
    | { type: 'expandFamily'; threadId: string }
    | { type: 'hideArchived'; threadId: string }
    | { type: 'revealArchived'; threadId: string }
    | { type: 'focusKey'; key: string };

export function leftAction(node: DrawerNavNode, st: NavCollapseState): NavCollapseAction {
    if (node.kind === 'tile') return adjacentTileAction(node.group, -1);
    if (node.kind === 'section') {
        return st.sectionCollapsed(node.sectionKey)
            ? { type: 'none' }
            : { type: 'collapseSection', sectionKey: node.sectionKey };
    }
    if (node.hasChildren && !st.familyCollapsed(node.id)) {
        return { type: 'collapseFamily', threadId: node.id, focusKey: node.id };
    }
    if (node.hiddenArchivedCount && st.archivedRevealed(node.id)) {
        return { type: 'hideArchived', threadId: node.id };
    }
    if (node.depth > 0 && node.parentId) {
        return { type: 'collapseFamily', threadId: node.parentId, focusKey: node.parentId };
    }
    if (node.sectionKey) {
        return { type: 'collapseSection', sectionKey: node.sectionKey };
    }
    if (node.group) {
        return { type: 'focusKey', key: ongoingTileNavKey(node.group) };
    }
    return { type: 'none' };
}

function adjacentTileAction(group: OngoingGroup, step: -1 | 1): NavCollapseAction {
    const next = ONGOING_GROUPS[ONGOING_GROUPS.indexOf(group) + step];
    return next ? { type: 'focusKey', key: ongoingTileNavKey(next) } : { type: 'none' };
}

export function rightAction(
    node: DrawerNavNode,
    nodes: readonly DrawerNavNode[],
    idx: number,
    st: NavCollapseState,
): NavCollapseAction {
    if (node.kind === 'tile') return adjacentTileAction(node.group, 1);
    if (node.kind === 'section') {
        if (st.sectionCollapsed(node.sectionKey)) {
            return { type: 'expandSection', sectionKey: node.sectionKey };
        }
        const next = nodes[idx + 1];
        return next && next.kind === 'thread'
            ? { type: 'focusKey', key: nodeKey(next) }
            : { type: 'none' };
    }
    if (node.hasChildren && st.familyCollapsed(node.id)) {
        return { type: 'expandFamily', threadId: node.id };
    }
    if (node.hiddenArchivedCount && !st.archivedRevealed(node.id)) {
        return { type: 'revealArchived', threadId: node.id };
    }
    if (node.hasChildren && !st.familyCollapsed(node.id)) {
        const next = nodes[idx + 1];
        if (next && next.kind === 'thread' && next.depth > node.depth) {
            return { type: 'focusKey', key: nodeKey(next) };
        }
    }
    return { type: 'none' };
}

function setHighlight(key: string): void {
    highlightedKey.value = key;
    scrollNavKeyIntoView(key);
}

function scrollNavKeyIntoView(key: string): void {
    if (!isSectionNavKey(key) && rowWindowIsMounted()) {
        revealDrawerRow(key, 'nearest');
        return;
    }
    const sel = isSectionNavKey(key)
        ? `[data-flip-id="${key}"], [data-tile-nav="${key}"]`
        : `[data-thread-nav="${key}"]`;
    document.querySelector(sel)?.scrollIntoView({ block: 'nearest' });
}

function liveCollapseState(): NavCollapseState {
    return {
        sectionCollapsed: (sectionKey) => collapsedSections.value.has(sectionKey),
        familyCollapsed: (threadId) => collapsedFamilies.value.has(threadId),
        archivedRevealed: (threadId) => revealedArchivedFamilies.value.has(threadId),
    };
}

function applyCollapseAction(action: NavCollapseAction): boolean {
    switch (action.type) {
        case 'none':
            return false;
        case 'collapseSection':
            setSectionCollapsed(action.sectionKey, true);
            setHighlight(sectionNavKey(action.sectionKey));
            return true;
        case 'expandSection':
            setSectionCollapsed(action.sectionKey, false);
            return true;
        case 'collapseFamily':
            setFamilyCollapsed(action.threadId, true);
            setHighlight(action.focusKey);
            return true;
        case 'expandFamily':
            setFamilyCollapsed(action.threadId, false);
            return true;
        case 'hideArchived':
            setArchivedRevealed(action.threadId, false);
            return true;
        case 'revealArchived':
            setArchivedRevealed(action.threadId, true);
            return true;
        case 'focusKey':
            setHighlight(action.key);
            return true;
    }
}

function highlightedNodeIndex(): number {
    const key = highlightedKey.value;
    if (!key) return -1;
    return navNodes.value.findIndex((n) => nodeKey(n) === key);
}

export function collapseHighlighted(): boolean {
    const idx = highlightedNodeIndex();
    if (idx < 0) return false;
    return applyCollapseAction(leftAction(navNodes.value[idx], liveCollapseState()));
}

export function expandHighlighted(): boolean {
    const nodes = navNodes.value;
    const idx = highlightedNodeIndex();
    if (idx < 0) return false;
    return applyCollapseAction(rightAction(nodes[idx], nodes, idx, liveCollapseState()));
}

export function moveHighlight(delta: number) {
    const nodes = navNodes.value;
    if (nodes.length === 0) return;
    const idx = highlightedNodeIndex();
    let next: number;
    if (idx === -1) {
        next = delta > 0 ? 0 : nodes.length - 1;
    } else {
        next = idx + delta;
        if (next < 0) next = 0;
        if (next >= nodes.length) next = nodes.length - 1;
    }
    setHighlight(nodeKey(nodes[next]));
}

export function pickInitialHighlight(openThreadId: string | null, navKeys: string[]): string | null {
    if (openThreadId && navKeys.includes(openThreadId)) return openThreadId;
    return navKeys[0] ?? null;
}

function whenNavNodesPublished(then: () => void): void {
    let tries = 0;
    const attempt = () => {
        if (navNodes.value.length === 0 && tries++ < 3) { requestAnimationFrame(attempt); return; }
        then();
    };
    attempt();
}

export function seedDrawerHighlight(): void {
    requestAnimationFrame(() => whenNavNodesPublished(() => {
        highlightedKey.value = pickInitialHighlight(focusedThreadId.value, navNodes.value.map(nodeKey));
    }));
}

export type FoldersReveal =
    | { kind: 'reveal'; section: DrawerSectionKey; collapsedAncestors: string[]; archivedAncestorsToReveal: string[] }
    | { kind: 'hidden-by-filter' }
    | { kind: 'not-listed' };

const NO_THREAD_FILTER: ThreadFilterSelection = {
    channels: new Set(ALL_CHANNELS),
    triggerIds: new Set(),
    repoIds: new Set(),
    appIds: new Set(),
};

function routeInList(threadId: string, threads: ThreadState[], filter: ThreadFilterSelection) {
    const { decorations, familyGraph } = computeDrawerCategorization(
        threads, filter.channels, filter.triggerIds, filter.repoIds, filter.appIds,
    );
    const routed = decorations.routedByThread.get(threadId);
    const section = routed === 'saved' ? savedSectionKey(threadId, familyGraph, threadFolders.value) : routed;
    return { section, familyGraph, decorations };
}

function savedSectionKey(threadId: string, graph: FamilyGraph, folders: readonly ThreadFolder[]): DrawerSectionKey {
    const root = graph.byId.get(graph.rootByThread.get(threadId) ?? threadId);
    const folder = knownFolderOf(root, folders);
    return folder ? customFolderSectionKey(folder.id) : 'saved';
}

function hiddenArchivedRevealChain(
    threadId: string,
    hidden: ReadonlySet<string>,
    graph: FamilyGraph,
): string[] {
    const toReveal: string[] = [];
    const visited = new Set<string>();
    let current: string | null | undefined = threadId;
    while (current && hidden.has(current) && !visited.has(current)) {
        visited.add(current);
        const parentId: string | null | undefined = graph.byId.get(current)?.meta.parentThreadId;
        if (!parentId) break;
        toReveal.push(parentId);
        current = parentId;
    }
    return toReveal;
}

export function planFoldersReveal(
    threadId: string,
    threads: ThreadState[],
    filter: ThreadFilterSelection,
    collapsed: ReadonlySet<string>,
): FoldersReveal {
    const { section, familyGraph, decorations } = routeInList(threadId, threads, filter);
    if (section) {
        return {
            kind: 'reveal',
            section,
            collapsedAncestors: collapsedAncestorIds(threadId, collapsed, familyGraph),
            archivedAncestorsToReveal: hiddenArchivedRevealChain(threadId, decorations.hiddenArchivedThreads, familyGraph),
        };
    }
    return routeInList(threadId, threads, NO_THREAD_FILTER).section
        ? { kind: 'hidden-by-filter' }
        : { kind: 'not-listed' };
}

export function revealThreadInFolders(threadId: string): void {
    showThreadList();
    closeThreadFilterPanel();
    const plan = planFoldersReveal(
        threadId, Array.from(threadMap.value.values()), appliedThreadFilter.value, collapsedFamilies.value,
    );
    if (plan.kind === 'hidden-by-filter') {
        showToast('This thread is hidden by the thread filter.', 'info', {
            action: { label: 'Clear filter', onClick: () => { clearThreadFilter(); revealThreadInFolders(threadId); } },
        });
        return;
    }
    if (plan.kind === 'not-listed') {
        showToast('This thread is not in the thread list.', 'info');
        return;
    }
    forgetSavedScroll(DRAWER_SCROLL_KEY);
    batch(() => {
        clearThreadSearch();
        setDrawerGrouping('folders');
        setSectionCollapsed(plan.section, false);
        for (const id of plan.collapsedAncestors) setFamilyCollapsed(id, false);
        for (const id of plan.archivedAncestorsToReveal) setArchivedRevealed(id, true);
    });
    revealDrawerRow(threadId, 'centre', (row, list) => {
        applyNavFocus(row);
        followRowUntilSettled(row, list);
    });
}

function clearThreadSearch(): void {
    threadSearchQuery.value = '';
    threadSearchResults.value = { status: 'not-loaded' };
}

const DRAWER_SCROLL_KEY = 'lucidos-scroll-thread-drawer';
let activePageIn: object | null = null;
let drawerRestoreSettled = false;
let drawerRestoreArmed = false;

function pendingDrawerRestoreTop(): number | null {
    if (drawerRestoreSettled) return null;
    const saved = readSavedScroll(DRAWER_SCROLL_KEY);
    return saved?.kind === 'offset' ? saved.top : null;
}

function centreRowInList(row: HTMLElement, list: HTMLElement): void {
    const r = row.getBoundingClientRect();
    const l = list.getBoundingClientRect();
    const top = list.scrollTop + (r.top + r.height / 2) - (l.top + l.height / 2);
    list.scrollTo({ top, behavior: scrollBehavior() });
}

function rowContentCentre(row: HTMLElement, list: HTMLElement): number {
    const r = row.getBoundingClientRect();
    return r.top + r.height / 2 - list.getBoundingClientRect().top + list.scrollTop;
}

const SETTLED_FRAMES = 3;
const MAX_FOLLOW_FRAMES = 600;

function followRowUntilSettled(row: HTMLElement, list: HTMLElement): void {
    let lastCentre = Number.NaN;
    let lastWidth = Number.NaN;
    let stillFrames = 0;
    let frames = 0;
    const tick = () => {
        if (!row.isConnected) return;
        const centre = rowContentCentre(row, list);
        const width = list.clientWidth;
        const moved = !(Math.abs(centre - lastCentre) <= 1);
        if (moved) {
            centreRowInList(row, list);
            lastCentre = centre;
        }
        const still = !moved && width === lastWidth && width > 0;
        stillFrames = still ? stillFrames + 1 : 0;
        lastWidth = width;
        if (stillFrames < SETTLED_FRAMES && frames++ < MAX_FOLLOW_FRAMES) requestAnimationFrame(tick);
    };
    tick();
}

export function handleDrawerKeyDown(e: KeyboardEvent): void {
    if (threadFilterPanelOpen.value) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === 'ArrowDown') {
        e.preventDefault();
        moveHighlight(1);
    } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        moveHighlight(-1);
    } else if (e.key === 'Enter') {
        e.preventDefault();
        selectHighlighted();
    } else if (e.key === 'ArrowLeft') {
        if (collapseHighlighted()) e.preventDefault();
    } else if (e.key === 'ArrowRight') {
        if (expandHighlighted()) e.preventDefault();
    }
}

export function ThreadDrawer({ forceVisible }: { forceVisible?: boolean } = {}) {
    const visible = forceVisible || drawnDrawerShown.value;
    const renderContent = useLingeringFlag(visible, scaledDurationMs(PANE_TRANSITION_MS) + 50);
    const isSearching = threadSearchQuery.value.trim().length > 0;
    const grouping = drawerGrouping.value;
    const activeView = isSearching ? 'search' : grouping;

    const treeRef = useRef<HTMLDivElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    const leavingDrawing = useSignal<LeavingDrawing | null>(null);
    const memoryPaused = activeView !== 'folders' || !visible || !preferencesSettled.value;
    useEffect(() => {
        drawerRestoreSettled = false;
        drawerRestoreArmed = true;
    }, [memoryPaused]);
    useScrollMemory(listRef, DRAWER_SCROLL_KEY, {
        paused: memoryPaused,
        restoreIsBusy: () => !threadsLoaded.value || drawerRestoreArmed || activePageIn !== null,
        onRestoreSettled: () => {
            drawerRestoreSettled = true;
            drawerRestoreArmed = false;
            replanDrawerRowWindow();
        },
    });

    useEffect(() => effect(() => {
        const id = navKeyDomId(highlightedKey.value);
        const el = treeRef.current;
        if (!el) return;
        if (id) el.setAttribute('aria-activedescendant', id);
        else el.removeAttribute('aria-activedescendant');
    }), [renderContent]);

    useEffect(() => {
        highlightedKey.value = null;
    }, [activeView]);

    const openWidth = `${drawnDrawerWidth.value}px`;
    return (
        <div class={`thread-drawer${visible ? '' : ' thread-drawer-collapsed'}`}
             style={{
                 ...(visible ? { width: openWidth } : {}),
                 ...(forceVisible ? {} : { '--thread-drawer-open-width': openWidth }),
             }}
             onPointerDown={(e) => handleDrawerPointerDown(e.target)}>
            <div class="thread-drawer-body">
                <div class="thread-drawer-list" ref={listRef} tabIndex={-1}
                     onFocus={(e) => { if (e.target === e.currentTarget) treeRef.current?.focus({ preventScroll: true }); }}>
                    {renderContent && (
                        <div ref={treeRef}
                             class="thread-drawer-tree"
                             onKeyDown={handleDrawerKeyDown}
                             role="tree"
                             aria-label="Threads"
                             tabIndex={visible ? 0 : -1}>
                            <LeavingViewSnapshot grouping={grouping} list={listRef} drawing={leavingDrawing}>
                                {activeView === 'search' ? <SearchResults />
                                    : activeView === 'ongoing' ? <OngoingList />
                                    : <ThreadList frozen={!visible} />}
                            </LeavingViewSnapshot>
                        </div>
                    )}
                </div>
                <ThreadFilterCover paneVisible={visible} leaving={leavingDrawing} />
            </div>
        </div>
    );
}


import { ongoingBreakdown, ongoingFilter, ongoingPartOf, stepOngoingFilter, composingThreads, collapsedAncestorIds, computeDrawerCategorization, depthStyle, filterHiddenArchived, holdRootCount, inboxThreadCount, nestByParent, renderedFamilyRows, ongoingGroupLists, selectedOngoingRows, threadHasUnsentDraft, visibleChildrenCount } from './family-graph';
import type { DrawerCategorization, FamilyGraph, NestedThread } from './family-graph';
import { hasUnseenBlocked } from '../../store/blockedSeen';
export * from './family-graph';
function ThreadList({ frozen }: { frozen: boolean }) {
    const containerRef = useRef<HTMLDivElement>(null);
    const rowWindow = useRowWindowController(containerRef, frozen, pendingDrawerRestoreTop, highlightedRowKey);
    const sentinelRef = useRef<HTMLDivElement>(null);
    const hydrated = threadsLoaded.value;

    const applied = appliedThreadFilter.value;
    const composingList = applied.channels.size === 0 ? [] : composingThreads(threadMap.value);

    const threadMapValue = threadMap.value;
    const { categorized, familyGraph, decorations } = useMemo<DrawerCategorization>(
        () => hydrated
            ? computeDrawerCategorization(
                Array.from(threadMapValue.values()),
                applied.channels,
                applied.triggerIds,
                applied.repoIds,
                applied.appIds,
            )
            : {
                categorized: { current: [], onHold: [], saved: [], archive: [], statusMap: new Map() },
                familyGraph: { byId: new Map(), rootByThread: new Map() },
                decorations: {
                    routedByThread: new Map(), liftedRoots: new Set(), archivedSubThreads: new Set(),
                    hiddenArchivedThreads: new Set(), hiddenDirectChildCount: new Map(),
                },
            },
        [hydrated, threadMapValue, applied],
    );
    const { current, onHold, saved, archive, statusMap } = categorized;

    const revealedArchivedSet = revealedArchivedFamilies.value;
    const visibleCurrent = filterHiddenArchived(current, decorations.hiddenArchivedThreads, revealedArchivedSet, familyGraph);
    const visibleSaved = filterHiddenArchived(saved, decorations.hiddenArchivedThreads, revealedArchivedSet, familyGraph);
    const visibleOnHold = filterHiddenArchived(onHold, decorations.hiddenArchivedThreads, revealedArchivedSet, familyGraph);

    const collapsedFamiliesSet = collapsedFamilies.value;
    const familyRows = (visible: ThreadState[]) =>
        renderedFamilyRows(visible, collapsedFamiliesSet, familyGraph, decorations);
    const archiveCount = archiveThreadCount.value;
    const sectionHasRunning = (threads: ThreadState[]) =>
        threads.some(t => statusMap.get(t.meta.id) === 'running');
    const folders = threadFolders.value;
    const savedSplit = splitByCustomFolder(saved, familyGraph.rootByThread, familyGraph.byId, folders);
    const visibleSavedSplit = splitByCustomFolder(visibleSaved, familyGraph.rootByThread, familyGraph.byId, folders);
    const folderSections: DrawerSectionRows[] = savedSplit.folders.map(({ folder, threads }, i) => ({
        name: customFolderSectionKey(folder.id),
        folder,
        count: inboxThreadCount(threads),
        threads: familyRows(visibleSavedSplit.folders[i].threads),
        hasRunning: sectionHasRunning(threads),
    }));
    const sectionByName: Record<DisplaySection, DrawerSectionRows> = {
        saved: {
            name: 'saved',
            count: inboxThreadCount(savedSplit.pinned),
            threads: familyRows(visibleSavedSplit.pinned),
            hasRunning: sectionHasRunning(savedSplit.pinned),
        },
        on_hold: { name: 'on_hold', count: holdRootCount(onHold), threads: familyRows(visibleOnHold), hasRunning: sectionHasRunning(onHold) },
        current: {
            name: 'current',
            count: composingList.length + inboxThreadCount(current),
            threads: [...nestByParent(composingList), ...familyRows(visibleCurrent)],
            hasRunning: sectionHasRunning(current),
        },
        archive: { name: 'archive', count: archiveCount, threads: familyRows(archive), hasRunning: sectionHasRunning(archive) },
    };
    const sections = [...folderSections, ...THREAD_DRAWER_SECTION_ORDER.map(name => sectionByName[name])];
    const listed = (s: DrawerSectionRows) => s.threads.length > 0 || s.folder !== undefined;

    const collapsed = collapsedSections.value;
    const sectionDefs: FlipSection[] = sections
        .filter(listed)
        .map(s => ({
            name: s.name,
            ids: [sectionNavKey(s.name), ...(collapsed.has(s.name) ? [] : s.threads.map(n => n.thread.meta.id))],
        }));

    const navList: DrawerNavNode[] = [];
    for (const s of sections) {
        if (!listed(s)) continue;
        navList.push({ kind: 'section', sectionKey: s.name });
        if (collapsed.has(s.name)) continue;
        for (const n of s.threads) {
            navList.push({
                kind: 'thread',
                id: n.thread.meta.id,
                depth: n.depth,
                parentId: n.depth > 0 ? (n.thread.meta.parentThreadId ?? null) : null,
                hasChildren: visibleChildrenCount(n.thread, decorations) > 0,
                hiddenArchivedCount: decorations.hiddenDirectChildCount.get(n.thread.meta.id) ?? 0,
                sectionKey: s.name,
            });
        }
    }
    const navKey = navList
        .map(n => n.kind === 'thread'
            ? `${n.id}:${n.depth}:${n.parentId ?? ''}:${n.hasChildren ? 1 : 0}:${n.hiddenArchivedCount ?? 0}`
            : nodeKey(n))
        .join(',');
    usePublishedNavNodes(() => navList, navKey);

    useFlipTransitions(containerRef, sectionDefs, applied, [collapsedFamiliesSet, collapsed, revealedArchivedSet]);

    const fillingRef = useRef(false);
    const loadWhileSentinelVisible = useCallback(async () => {
        if (fillingRef.current) return;
        const sentinel = sentinelRef.current;
        const root = sentinel?.closest('.thread-drawer-list');
        if (!sentinel || !root) return;
        fillingRef.current = true;
        try {
            const MAX_PAGES_PER_FILL = 40;
            let pages = 0;
            while (
                pages < MAX_PAGES_PER_FILL &&
                threadHasMore.value &&
                archivePaginationAllowed(collapsedSections.value) &&
                sentinelInView(sentinel.getBoundingClientRect(), root.getBoundingClientRect())
            ) {
                const landed = await loadOlderThreads();
                pages++;
                await nextFrame();
                if (!landed) break;
            }
        } finally {
            fillingRef.current = false;
        }
    }, []);

    useEffect(() => {
        if (!hydrated || !filterChangedSinceLoad()) return;
        void reloadAfterFilterChange().then(() => void loadWhileSentinelVisible());
    }, [hydrated, applied]);

    const restoreCanAttach = preferencesSettled.value;
    useEffect(() => {
        const list = containerRef.current?.closest<HTMLElement>('.thread-drawer-list');
        if (!hydrated || !restoreCanAttach || !list) return;
        void pageInToSavedScroll(list).then(() => void loadWhileSentinelVisible());
        return () => { activePageIn = null; };
    }, [hydrated, restoreCanAttach]);

    const hasMore = threadHasMore.value;
    const paginationAllowed = archivePaginationAllowed(collapsed);
    const showLoadingMore = useDelayedFlag(threadLoadingMore.value);

    useEffect(() => {
        const sentinel = sentinelRef.current;
        if (!sentinel || !hydrated || !hasMore || !paginationAllowed) return;

        const observer = new IntersectionObserver(
            (entries) => {
                if (entries[0]?.isIntersecting) void loadWhileSentinelVisible();
            },
            { root: sentinel.closest('.thread-drawer-list'), threshold: 0 },
        );
        observer.observe(sentinel);
        void loadWhileSentinelVisible();
        return () => observer.disconnect();
    }, [hydrated, hasMore, paginationAllowed, loadWhileSentinelVisible]);

    const windowedRows = (s: typeof sections[number]) => (
        <WindowedRows
            name={s.name}
            ids={s.threads.map(n => n.thread.meta.id)}
            layoutKeys={s.threads.map(n => {
                const id = n.thread.meta.id;
                return n.thread.meta.state === 'composing'
                    ? null
                    : threadRowLayoutKey(threadRowLayoutProps(n.thread, {
                        depth: n.depth,
                        hiddenArchivedCount: decorations.hiddenDirectChildCount.get(id) ?? 0,
                        isArchivedRevealed: revealedArchivedSet.has(id),
                        enableFamilyToggle: true,
                    }));
            })}
            renderRow={(i) => {
                const n = s.threads[i];
                if (n.thread.meta.state === 'composing') {
                    return <ComposingThreadRow key={n.thread.meta.id} thread={n.thread} depth={n.depth} />;
                }
                const id = n.thread.meta.id;
                const root = familyGraph.rootByThread.get(id);
                const familyIsLifted = root !== undefined && decorations.liftedRoots.has(root);
                const isRoot = root === id;
                const naturalSection = getThreadDisplaySection(n.thread);
                const routedSection = decorations.routedByThread.get(id);
                const isLiftedParent = familyIsLifted && isRoot;
                const isResponsibleChild = familyIsLifted && !isRoot && naturalSection === routedSection;
                return (
                    <ThreadRow
                        key={id}
                        threadId={id}
                        status={statusMap.get(id)!}
                        depth={n.depth}
                        isLiftedParent={isLiftedParent}
                        isResponsibleChild={isResponsibleChild}
                        isArchivedSubThread={decorations.archivedSubThreads.has(id)}
                        hiddenArchivedCount={decorations.hiddenDirectChildCount.get(id) ?? 0}
                        isArchivedRevealed={revealedArchivedSet.has(id)}
                        enableFamilyToggle
                    />
                );
            }}
        />
    );

    return (
        <RowWindowContext.Provider value={rowWindow}>
            {!hydrated ? <div ref={containerRef} /> : (
                <div ref={containerRef} class="thread-drawer-rows">
                    {sections.map(s => {
                        if (!listed(s)) return null;
                        const { title, Icon } = s.folder
                            ? { title: s.folder.name, Icon: FolderIcon }
                            : SECTION_META[s.name as DisplaySection];
                        return (
                            <DrawerSection key={s.name} sectionKey={s.name} title={title} Icon={Icon} count={s.count} hasRunning={s.hasRunning}
                                action={sectionAction(s)}>
                                {s.threads.length > 0 && windowedRows(s)}
                            </DrawerSection>
                        );
                    })}
                    {sections.every(s => !listed(s)) && (
                        <div class="empty-state">No threads</div>
                    )}
                    <LoadingFade showSkeleton={showLoadingMore} skeleton={<ListSkeletonOf count={2} row={() => <ThreadRowContent />} />}>
                        {null}
                    </LoadingFade>
                    {hasMore && paginationAllowed && (
                        <div ref={sentinelRef} class="thread-drawer-load-more" />
                    )}
                </div>
            )}
        </RowWindowContext.Provider>
    );
}

const COLLAPSED_KEY = 'lucidos-drawer-collapsed';
const COLLAPSED_FAMILIES_KEY = 'lucidos-drawer-collapsed-families';

function loadStringSet(key: string): Set<string> {
    try {
        const raw = localStorage.getItem(key);
        return raw ? new Set(JSON.parse(raw)) : new Set();
    } catch { return new Set(); }
}

function saveStringSet(key: string, set: Set<string>) {
    localStorage.setItem(key, JSON.stringify([...set]));
}

const collapsedSections = signal(loadStringSet(COLLAPSED_KEY));

const collapsedFamilies = signal(loadStringSet(COLLAPSED_FAMILIES_KEY));

export function setSectionCollapsed(sectionKey: string, collapse: boolean) {
    if (collapsedSections.value.has(sectionKey) === collapse) return;
    const next = new Set(collapsedSections.value);
    if (collapse) next.add(sectionKey);
    else next.delete(sectionKey);
    collapsedSections.value = next;
    saveStringSet(COLLAPSED_KEY, next);
}

export function toggleSectionCollapse(sectionKey: string) {
    setSectionCollapsed(sectionKey, !collapsedSections.value.has(sectionKey));
}

export function setFamilyCollapsed(threadId: string, collapse: boolean) {
    if (collapsedFamilies.value.has(threadId) === collapse) return;
    const next = new Set(collapsedFamilies.value);
    if (collapse) next.add(threadId);
    else next.delete(threadId);
    collapsedFamilies.value = next;
    saveStringSet(COLLAPSED_FAMILIES_KEY, next);
}

export function toggleFamilyCollapse(threadId: string) {
    setFamilyCollapsed(threadId, !collapsedFamilies.value.has(threadId));
}

const REVEALED_ARCHIVED_KEY = 'lucidos-drawer-revealed-archived';

const revealedArchivedFamilies = signal(loadStringSet(REVEALED_ARCHIVED_KEY));

export function setArchivedRevealed(threadId: string, revealed: boolean) {
    if (revealedArchivedFamilies.value.has(threadId) === revealed) return;
    const next = new Set(revealedArchivedFamilies.value);
    if (revealed) next.add(threadId);
    else next.delete(threadId);
    revealedArchivedFamilies.value = next;
    saveStringSet(REVEALED_ARCHIVED_KEY, next);
}

export function toggleArchivedRevealed(threadId: string) {
    setArchivedRevealed(threadId, !revealedArchivedFamilies.value.has(threadId));
}

async function pageInToSavedScroll(list: HTMLElement): Promise<void> {
    const token = {};
    activePageIn = token;
    try {
        await nextFrame();
        if (activePageIn !== token || !drawerRestoreArmed) return;
        drawerRestoreArmed = false;
        const saved = readSavedScroll(DRAWER_SCROLL_KEY);
        if (saved?.kind !== 'offset') return;
        const target = saved.top;
        while (
            activePageIn === token &&
            !drawerRestoreSettled &&
            list.scrollHeight - list.clientHeight < target &&
            threadHasMore.value &&
            archivePaginationAllowed(collapsedSections.value)
        ) {
            const inFlight = threadLoadingMore.value;
            const landed = await loadOlderThreads();
            if (!landed && !(inFlight && await paginationIdle())) break;
            await nextFrame();
        }
    } finally {
        if (activePageIn === token) activePageIn = null;
    }
}

function nextFrame(): Promise<void> {
    return new Promise(resolve => requestAnimationFrame(() => resolve()));
}

const PAGINATION_IDLE_MAX_FRAMES = 600;

async function paginationIdle(): Promise<boolean> {
    for (let frames = 0; frames < PAGINATION_IDLE_MAX_FRAMES; frames++) {
        if (!threadLoadingMore.value) return true;
        await nextFrame();
    }
    return false;
}

export function archivePaginationAllowed(collapsed: ReadonlySet<string>): boolean {
    return !collapsed.has('archive');
}

export function sentinelInView(sentinel: { top: number; bottom: number }, root: { top: number; bottom: number }): boolean {
    return sentinel.top < root.bottom && sentinel.bottom > root.top;
}

function DrawerSection({ sectionKey, title, Icon, count, hasRunning, action, children }: { sectionKey: string; title: string; Icon?: ComponentType<{ size?: string }>; count: number; hasRunning?: boolean; action?: ComponentChildren; children: ComponentChildren }) {
    const collapsed = collapsedSections.value.has(sectionKey);
    return (
        <div class="drawer-section">
            <DrawerSectionTitle
                navKey={sectionNavKey(sectionKey)} title={title} Icon={Icon}
                count={count} hasRunning={hasRunning} collapsed={collapsed} action={action}
                onToggle={() => toggleSectionCollapse(sectionKey)}
            />
            {!collapsed && children}
        </div>
    );
}

function DrawerSectionTitle({ navKey, title, Icon, count, hasRunning, collapsed, onToggle, action }: {
    navKey: string;
    title: string;
    Icon?: ComponentType<{ size?: string }>;
    count: number;
    hasRunning?: boolean;
    collapsed: boolean;
    onToggle: () => void;
    action?: ComponentChildren;
}) {
    const highlighted = highlightedKey.value === navKey;
    return (
        <div class={`list-section-title list-section-title-collapsible${collapsed ? ' collapsed' : ''}${highlighted ? ' list-section-title-highlighted' : ''}`}
             id={navKeyDomId(navKey)}
             data-flip-id={navKey}
             onClick={onToggle}
             role="treeitem"
             aria-selected={highlighted}
             aria-expanded={!collapsed}>
            <SectionHeaderContent Icon={Icon} title={title} count={count} running={hasRunning} collapsible />
            {action && <span class="list-section-actions drawer-section-actions">{action}</span>}
        </div>
    );
}

function ArchiveAllButton() {
    const phase = archiveAllPhase.value;
    const label = { idle: 'Archive all', checking: 'Checking…', confirming: 'Awaiting confirmation…', archiving: 'Archiving…' }[phase];
    return (
        <button type="button" class="drawer-archive-all"
                disabled={phase !== 'idle'} aria-busy={phase === 'checking' || phase === 'archiving'}
                onClick={(e) => { e.stopPropagation(); void handleArchiveAll(); }}>
            {label}
        </button>
    );
}

function RowActions({ children }: { children: ComponentChildren }) {
    return <span class="thread-row-actions-line"><span class="thread-row-actions">{children}</span></span>;
}

type AgentGlyph = 'claude' | 'codex' | 'trigger' | 'lucidos';

const AGENT_GLYPHS: Record<AgentGlyph, () => VNode> = {
    claude: () => <ClaudeIcon />,
    codex: () => <CodexIcon />,
    trigger: () => <CategoryIcon category="triggers" />,
    lucidos: () => <><LucidosMarkIcon /><LucidosGlyph /></>,
};

function isStackedAgents(channel: string | undefined, agents: CodingAgent[] | undefined): agents is CodingAgent[] {
    return channel === 'claude_code' && !!agents && agents.length > 1;
}

function agentGlyph(channel: string, codingAgent?: CodingAgent): AgentGlyph | null {
    if (channel === 'claude_code') return codingAgent === 'codex' ? 'codex' : 'claude';
    if (channel === 'trigger') return 'trigger';
    if (channel === 'chat' || channel === 'lucidos') return 'lucidos';
    return null;
}

function ThreadAgentMark(
    { channel, codingAgent, codingAgents }: { channel: string; codingAgent?: CodingAgent; codingAgents?: CodingAgent[] },
) {
    if (isStackedAgents(channel, codingAgents)) {
        return <ThreadAgentStack channel={channel} agents={codingAgents} />;
    }
    const glyph = agentGlyph(channel, codingAgent);
    if (!glyph) {
        const error = channel === 'error_unknown_channel' ? ' channel-error' : '';
        return <span class={`label message-channel-tag${error}`}>{formatChannel(channel)}</span>;
    }
    const label = formatThreadChannelLabel(channel, codingAgent);
    return (
        <span class="thread-row-agent" data-glyph={glyph} role="img" aria-label={label} data-tooltip={label}>
            {AGENT_GLYPHS[glyph]()}
        </span>
    );
}

function ThreadAgentStack({ channel, agents }: { channel: string; agents: CodingAgent[] }) {
    const names = agents.map(agent => formatThreadChannelLabel(channel, agent));
    return (
        <span class="thread-row-agents" role="group" aria-label={names.join(', then ')}>
            {agents.map(agent => (
                <ThreadAgentMark key={agent} channel={channel} codingAgent={agent} />
            ))}
        </span>
    );
}

function ContextChip({ name }: { name: string }) {
    const ref = useRef<HTMLSpanElement>(null);
    useLayoutEffect(() => {
        const el = ref.current;
        if (!el) return;
        const cs = getComputedStyle(el);
        const maxW = parseFloat(cs.maxWidth);
        el.style.width = Number.isFinite(maxW) && maxW > 0 ? `${maxW}px` : '';
        const range = document.createRange();
        range.selectNodeContents(el);
        const rects = range.getClientRects();
        const naturalLines = rects.length;
        if (naturalLines <= 1) { el.style.width = ''; return; }
        const scaleX = el.offsetWidth > 0 ? el.getBoundingClientRect().width / el.offsetWidth : 1;
        let maxLine = 0;
        for (let i = 0; i < rects.length; i++) maxLine = Math.max(maxLine, rects[i].width / scaleX);
        const chrome = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight)
            + parseFloat(cs.borderLeftWidth) + parseFloat(cs.borderRightWidth);
        el.style.width = `${(Math.ceil(maxLine) + chrome + 1) / getRemPx()}rem`;
        range.selectNodeContents(el);
        if (range.getClientRects().length > naturalLines) el.style.width = '';
    }, [name]);
    return <span ref={ref} class="label thread-row-context">{name}</span>;
}

export function ComposingThreadRow({ thread, depth = 0 }: { thread: ThreadState; depth?: number }) {
    const isFocused = focusedThreadId.value === thread.meta.id;
    const isHighlighted = highlightedKey.value === thread.meta.id;
    const classes = ['list-row', 'thread-row', 'compose-draft-row'];
    if (isFocused) classes.push('thread-row-focused');
    if (isHighlighted) classes.push('thread-row-highlighted');
    const draftMode = getDraft(thread.meta.id).mode;
    const draftChannel = draftMode === 'claude_code' ? 'claude_code' : 'chat';
    const draftCodingAgent = draftMode === 'claude_code' ? resolveCodingAgent(thread.meta.id) : undefined;
    const reposLoadable = repositories.value;
    const draftScope = resolveScope(thread.meta.id);
    const contextName = composeDraftContextName(
        draftMode,
        draftScope,
        reposLoadable.status === 'loaded' ? reposLoadable.data : [],
    );
    const createdLabel = formatCreatedTimestamp(thread.meta.createdAt);
    const gesture = useRowActionsGesture({
        enabled: true,
        onTap: () => focusThread(thread.meta.id),
    });
    const draftMenu = (
        <DraftOverflowMenu
            threadId={thread.meta.id}
            stopPropagation
            extraClass="thread-row-action"
            tabIndex={-1}
            hostOpener={gesture.hostOpener}
        />
    );

    return (
        <div data-flip-id={thread.meta.id} style={depthStyle(depth)} class={depth > 0 ? 'thread-row-wrap is-nested' : 'thread-row-wrap'}>
            <div class={classes.join(' ')}
                 id={navKeyDomId(thread.meta.id)}
                 data-thread-nav={thread.meta.id}
                 role="treeitem"
                 aria-selected={isHighlighted}
                 {...gesture.handlers}>
                <div class="thread-row-left">
                    <span class="thread-row-title-row">
                        <span class="thread-row-title-text">
                            <ThreadStatusIcon status="idle" />
                            <span class="thread-row-title">{threadDisplayTitle(thread)}</span>
                            <span class="draft-indicator">Draft</span>
                        </span>
                        {gesture.hostOpener.trigger ? <RowActions>{draftMenu}</RowActions> : draftMenu}
                    </span>
                    <span class="thread-row-meta">
                        {createdLabel && <span class="thread-row-created">{createdLabel}</span>}
                        {contextName && <ContextChip name={contextName} />}
                        <ThreadAgentMark channel={draftChannel} codingAgent={draftCodingAgent} />
                    </span>
                </div>
            </div>
        </div>
    );
}

interface ThreadRowContentProps {
    id: string;
    depth?: number;
    flipId?: string;
    title: string;
    createdLabel: string;
    channel: string;
    codingAgent?: 'claude-code' | 'codex';
    codingAgents?: CodingAgent[];
    visualStatus: VisualStatus;
    contextName?: string;
    totalChildren: number;
    needsReview: boolean;
    hasDraft: boolean;
    isSaved: boolean;
    isHome?: boolean;
    isFocused: boolean;
    isHighlighted: boolean;
    isLiftedParent?: boolean;
    isResponsibleChild?: boolean;
    isArchivedSubThread?: boolean;
    collapsible?: boolean;
    isCollapsed?: boolean;
    onToggleFamily?: () => void;
    hiddenArchivedCount?: number;
    isArchivedRevealed?: boolean;
    onToggleArchivedRevealed?: () => void;
    onShowInFolders?: () => void;
    onClick: () => void;
}

export const THREAD_ROW_PROP_ROLES = {
    id: 'identity',
    flipId: 'identity',
    depth: 'layout',
    title: 'layout',
    createdLabel: 'layout',
    channel: 'layout',
    codingAgent: 'layout',
    codingAgents: 'layout',
    contextName: 'layout',
    totalChildren: 'layout',
    hasDraft: 'layout',
    isHome: 'layout',
    collapsible: 'layout',
    isCollapsed: 'layout',
    hiddenArchivedCount: 'layout',
    isArchivedRevealed: 'layout',
    visualStatus: 'paint',
    needsReview: 'paint',
    isSaved: 'paint',
    isFocused: 'paint',
    isHighlighted: 'paint',
    isLiftedParent: 'paint',
    isResponsibleChild: 'paint',
    isArchivedSubThread: 'paint',
    onToggleFamily: 'handler',
    onToggleArchivedRevealed: 'handler',
    onShowInFolders: 'handler',
    onClick: 'handler',
} as const satisfies Record<keyof ThreadRowContentProps, 'layout' | 'paint' | 'handler' | 'identity'>;

type ThreadRowLayoutName = {
    [K in keyof typeof THREAD_ROW_PROP_ROLES]: (typeof THREAD_ROW_PROP_ROLES)[K] extends 'layout' ? K : never
}[keyof typeof THREAD_ROW_PROP_ROLES];

export type ThreadRowLayoutProps = { [K in ThreadRowLayoutName]-?: ThreadRowContentProps[K] };

const THREAD_ROW_LAYOUT_NAMES = (Object.keys(THREAD_ROW_PROP_ROLES) as (keyof typeof THREAD_ROW_PROP_ROLES)[])
    .filter((k): k is ThreadRowLayoutName => THREAD_ROW_PROP_ROLES[k] === 'layout');

export function threadRowLayoutKey(layout: ThreadRowLayoutProps): string {
    return THREAD_ROW_LAYOUT_NAMES.map(k => String(layout[k])).join('\u0000');
}

export interface ThreadRowLayoutInput {
    depth: number;
    hiddenArchivedCount: number;
    isArchivedRevealed?: boolean;
    enableFamilyToggle?: boolean;
}

export function threadRowLayoutProps(
    thread: ThreadState,
    { depth, hiddenArchivedCount, isArchivedRevealed, enableFamilyToggle }: ThreadRowLayoutInput,
): ThreadRowLayoutProps {
    const { meta } = thread;
    const liveChildrenCount = meta.totalChildrenCount - hiddenArchivedCount;
    const collapsible = !!enableFamilyToggle && liveChildrenCount > 0;
    return {
        depth,
        title: threadDisplayTitle(thread),
        createdLabel: formatCreatedTimestamp(meta.createdAt),
        channel: meta.channel,
        codingAgent: meta.codingAgent,
        codingAgents: meta.codingAgents,
        contextName: threadContextName(meta),
        totalChildren: liveChildrenCount,
        hasDraft: threadHasUnsentDraft(thread),
        isHome: meta.home === true,
        collapsible,
        isCollapsed: collapsible && collapsedFamilies.value.has(meta.id),
        hiddenArchivedCount: enableFamilyToggle ? hiddenArchivedCount : 0,
        isArchivedRevealed,
    };
}

export function familyDisclosureLabel(totalChildren: number, isCollapsed: boolean): string {
    const count = `${totalChildren} sub-thread${totalChildren === 1 ? '' : 's'}`;
    return `${isCollapsed ? 'Show' : 'Hide'} ${count}`;
}

function ThreadRowContentImpl(props: Partial<ThreadRowContentProps>) {
    const sk = useSkeleton();
    const leftRef = useRef<HTMLDivElement>(null);
    const stacked = !sk && isStackedAgents(props.channel, props.codingAgents);
    useLayoutEffect(() => {
        const left = leftRef.current;
        if (!stacked || !left) return;
        placeStackedMark(left);
        let frame = 0;
        const observer = new ResizeObserver(() => {
            cancelAnimationFrame(frame);
            frame = requestAnimationFrame(() => placeStackedMark(left));
        });
        observer.observe(left);
        return () => {
            observer.disconnect();
            cancelAnimationFrame(frame);
            clearStackedMark(left);
        };
    }, [stacked, props.title]);
    const depth = props.depth ?? 0;
    const hasLiveFamily = !!props.collapsible && (props.totalChildren ?? 0) > 0;
    const hasArchivedToggle = (props.hiddenArchivedCount ?? 0) > 0;
    const hasFamily = hasLiveFamily || hasArchivedToggle;

    const classes = ['list-row', 'thread-row'];
    if (props.isFocused) classes.push('thread-row-focused');
    if (props.isHighlighted) classes.push('thread-row-highlighted');
    if (props.needsReview) classes.push('thread-row-review');
    if (props.isLiftedParent) classes.push('thread-row-lifted-parent');
    if (props.isResponsibleChild) classes.push('thread-row-lifted-child');
    const wrapClasses = ['thread-row-wrap'];
    if (depth > 0) wrapClasses.push('is-nested');
    if (props.isArchivedSubThread) wrapClasses.push('is-archived');

    const toggleFamily = (e: Event) => {
        e.stopPropagation();
        props.onToggleFamily?.();
    };

    const toggleArchived = (e: Event) => {
        e.stopPropagation();
        props.onToggleArchivedRevealed?.();
    };

    const hasChips = sk || !!props.channel || !!props.contextName;
    const chips = hasChips && (
        <>
            {!sk && props.contextName && <ContextChip name={props.contextName} />}
            {sk ? (
                <SkBlock w="var(--icon-size-sm)" h="var(--icon-size-sm)" round />
            ) : props.channel ? (
                <ThreadAgentMark channel={props.channel} codingAgent={props.codingAgent} codingAgents={props.codingAgents} />
            ) : null}
        </>
    );

    const gesture = useRowActionsGesture({
        enabled: !sk && !!props.id,
        onTap: props.onClick,
        onPress: () => { if (props.id) void loadThreadEvents(props.id); },
    });

    return (
        <div class={wrapClasses.join(' ')}
             style={depthStyle(depth)}
             {...(props.flipId ? { 'data-flip-id': props.flipId } : {})}>
            <div class={classes.join(' ')}
                 id={props.id ? navKeyDomId(props.id) : undefined}
                 data-thread-nav={props.id}
                 role={props.id ? 'treeitem' : undefined}
                 aria-selected={props.id ? (props.isHighlighted ?? false) : undefined}
                 {...gesture.handlers}>
                <div class="thread-row-left" ref={leftRef}>
                    <span class="thread-row-title-row">
                        <span class="thread-row-title-text">
                            <ThreadStatusIcon status={sk ? null : (props.visualStatus ?? null)} />
                            <ThreadIcon home={props.isHome === true} />
                            <SkText class="thread-row-title" w="11rem">{props.title}</SkText>
                            {props.hasDraft && <span class="draft-indicator" data-tooltip="Has unsent draft">Draft</span>}
                        </span>
                        {!sk && props.id && (
                            <RowActions>
                                {!props.isHome && <PinThreadButton threadId={props.id} saved={props.isSaved ?? false} stopPropagation extraClass="thread-row-action" tabIndex={-1} />}
                                <ThreadOverflowMenu threadId={props.id} title={props.title ?? ''} onShowInFolders={props.onShowInFolders} stopPropagation extraClass="thread-row-action" tabIndex={-1} hostOpener={gesture.hostOpener} />
                            </RowActions>
                        )}
                    </span>
                    {(props.createdLabel || hasChips) && (
                        <span class="thread-row-meta">
                            {(sk || props.createdLabel) && <SkText class="thread-row-created" w="5rem">{props.createdLabel}</SkText>}
                            {chips}
                        </span>
                    )}
                    {hasFamily && (
                        <span class="thread-row-family-line">
                            {hasLiveFamily && (
                                <button
                                    type="button"
                                    class="family-disclosure"
                                    tabIndex={-1}
                                    onClick={toggleFamily}
                                    onKeyDown={(e) => {
                                        if (e.key === 'Enter' || e.key === ' ') {
                                            e.preventDefault();
                                            toggleFamily(e);
                                        }
                                    }}
                                    aria-expanded={!props.isCollapsed}>
                                    <ChevronRightIcon />
                                    {familyDisclosureLabel(props.totalChildren ?? 0, props.isCollapsed ?? false)}
                                </button>
                            )}
                            {hasArchivedToggle && (
                                <label class="archived-reveal" onClick={(e) => e.stopPropagation()}>
                                    <span class="toggle-switch archived-reveal-toggle">
                                        <input
                                            type="checkbox"
                                            tabIndex={-1}
                                            checked={props.isArchivedRevealed ?? false}
                                            onChange={toggleArchived}
                                            aria-label={`${props.isArchivedRevealed ? 'Hide' : 'Show'} ${props.hiddenArchivedCount} archived sub-thread${props.hiddenArchivedCount === 1 ? '' : 's'}`}
                                        />
                                        <span class="toggle-slider"></span>
                                    </span>
                                    <span class="archived-reveal-label">{props.hiddenArchivedCount} archived</span>
                                </label>
                            )}
                        </span>
                    )}
                </div>
            </div>
        </div>
    );
}

const ThreadRowContent = memo(ThreadRowContentImpl, (prev, next) =>
    prev.id === next.id
    && prev.depth === next.depth
    && prev.flipId === next.flipId
    && prev.title === next.title
    && prev.createdLabel === next.createdLabel
    && prev.channel === next.channel
    && prev.codingAgent === next.codingAgent
    && sameCodingAgents(prev.codingAgents, next.codingAgents)
    && prev.visualStatus === next.visualStatus
    && prev.contextName === next.contextName
    && prev.totalChildren === next.totalChildren
    && prev.needsReview === next.needsReview
    && prev.hasDraft === next.hasDraft
    && prev.isSaved === next.isSaved
    && prev.isHome === next.isHome
    && prev.isFocused === next.isFocused
    && prev.isHighlighted === next.isHighlighted
    && prev.isLiftedParent === next.isLiftedParent
    && prev.isResponsibleChild === next.isResponsibleChild
    && prev.isArchivedSubThread === next.isArchivedSubThread
    && prev.collapsible === next.collapsible
    && prev.isCollapsed === next.isCollapsed
    && prev.hiddenArchivedCount === next.hiddenArchivedCount
    && prev.isArchivedRevealed === next.isArchivedRevealed
);

export function ThreadRow({ threadId, status, depth = 0, isLiftedParent, isResponsibleChild, isArchivedSubThread, hiddenArchivedCount = 0, isArchivedRevealed, enableFamilyToggle, offerShowInFolders, onOpen = focusThread }: {
    threadId: string;
    status: ThreadStatus;
    depth?: number;
    isLiftedParent?: boolean;
    isResponsibleChild?: boolean;
    isArchivedSubThread?: boolean;
    hiddenArchivedCount?: number;
    isArchivedRevealed?: boolean;
    enableFamilyToggle?: boolean;
    offerShowInFolders?: boolean;
    onOpen?: (threadId: string) => void;
}) {
    const thread = threadMap.value.get(threadId);
    if (!thread) return null;
    const { meta } = thread;
    const visualStatus = visualStatusFor(status, meta);
    const isFocused = focusedThreadId.value === meta.id;
    const isHighlighted = highlightedKey.value === meta.id;

    return (
        <ThreadRowContent
            {...threadRowLayoutProps(thread, { depth, hiddenArchivedCount, isArchivedRevealed, enableFamilyToggle })}
            id={meta.id}
            flipId={meta.id}
            visualStatus={visualStatus}
            needsReview={meta.section === 'inbox' && status !== 'running'}
            isSaved={meta.saved}
            isFocused={isFocused}
            isHighlighted={isHighlighted}
            isLiftedParent={isLiftedParent}
            isResponsibleChild={isResponsibleChild}
            isArchivedSubThread={isArchivedSubThread}
            onToggleFamily={() => toggleFamilyCollapse(meta.id)}
            onToggleArchivedRevealed={() => toggleArchivedRevealed(meta.id)}
            onShowInFolders={offerShowInFolders && !meta.home ? () => revealThreadInFolders(meta.id) : undefined}
            onClick={() => onOpen(meta.id)}
        />
    );
}

function usePublishedHeight(ref: RefObject<HTMLElement>, name: string): void {
    useLayoutEffect(() => {
        const el = ref.current;
        const list = el?.closest<HTMLElement>('.thread-drawer-list');
        if (!el || !list) return;
        const publish = () => list.style.setProperty(name, `${el.offsetHeight}px`);
        publish();
        const observer = new ResizeObserver(publish);
        observer.observe(el);
        return () => {
            observer.disconnect();
            list.style.removeProperty(name);
        };
    }, []);
}

const ONGOING_TILES_FLIP_ID = sectionNavKey('ongoing-tiles');

function OngoingTiles({ children }: { children?: ComponentChildren }) {
    const ref = useRef<HTMLDivElement>(null);
    usePublishedHeight(ref, '--drawer-ongoing-tiles-height');
    return <div ref={ref} class="drawer-ongoing-tiles" data-flip-id={ONGOING_TILES_FLIP_ID}>{children}</div>;
}

const ONGOING_GROUP_META: Record<OngoingGroup, { title: string; empty: string; Icon: ComponentType<{ size?: string }> }> = {
    drafts: { title: 'Drafts', empty: 'No drafts', Icon: DraftsIcon },
    blocked: { title: 'Blocked', empty: 'Nothing is blocked', Icon: BlockedIcon },
    review: { title: 'Review', empty: 'Nothing to review', Icon: ReviewIcon },
    'in-flight': { title: 'In flight', empty: 'Nothing in flight', Icon: InFlightIcon },
};

function ongoingPartWord(part: OngoingPart, count: number): string {
    return part === 'changes' && count === 1 ? 'change' : part;
}

function OngoingTileBreakdown({ group, rows, filter }: { group: SplitOngoingGroup; rows: readonly NestedThread[]; filter: OngoingFilter }) {
    const parts = ongoingBreakdown(group, rows).filter(({ count }) => count > 0);
    return (
        <span class="ongoing-tile-breakdown">
            {parts.map(({ part, count }) => (
                <span key={part} class={`ongoing-tile-breakdown-part${part === filter ? ' ongoing-tile-breakdown-part-listed' : ''}`}>
                    <span class="ongoing-tile-breakdown-count">{count}</span>
                    <span class="ongoing-tile-breakdown-word">{` ${ongoingPartWord(part, count)}`}</span>
                </span>
            ))}
        </span>
    );
}

function OngoingTile({ group, rows, hydrated, selected }: {
    group: OngoingGroup;
    rows: readonly NestedThread[];
    hydrated: boolean;
    selected: boolean;
}) {
    const { title, Icon } = ONGOING_GROUP_META[group];
    const navKey = ongoingTileNavKey(group);
    const highlighted = highlightedKey.value === navKey;
    const running = group === 'in-flight' && rows.some(n => ongoingPartOf(group, n) === 'running');
    const filter = selected ? ongoingFilter.value : 'all';
    const classes = ['ongoing-tile'];
    if (selected) classes.push('ongoing-tile-selected');
    if (hydrated && rows.length === 0) classes.push('ongoing-tile-empty');
    if (group === 'blocked' && rows.length > 0) {
        classes.push('ongoing-tile-blocked');
        if (hasUnseenBlocked.value) classes.push('ongoing-tile-unseen');
    }
    if (highlighted) classes.push('ongoing-tile-highlighted');
    const label = hydrated ? `${title}, ${rows.length}` : title;
    return (
        <div class={classes.join(' ')}
             id={navKeyDomId(navKey)}
             data-tile-nav={navKey}
             onClick={() => pressOngoingTile(group)}
             role="treeitem"
             aria-selected={highlighted}
             aria-current={selected ? 'true' : undefined}
             aria-label={filter === 'all' ? label : `${label}, showing ${filter}`}>
            <span class="ongoing-tile-top">
                <span class="ongoing-tile-figures">
                    <span class="ongoing-tile-count">{hydrated ? rows.length : ''}</span>
                    {isSplitOngoingGroup(group) && <OngoingTileBreakdown group={group} rows={rows} filter={filter} />}
                </span>
                <span class="ongoing-tile-icon"><Icon size="var(--icon-size-sm)" /></span>
            </span>
            <span class={`ongoing-tile-label${running ? ' running-shimmer' : ''}`}>{title}</span>
        </div>
    );
}

function OngoingList() {
    const containerRef = useRef<HTMLDivElement>(null);
    const hydrated = threadsLoaded.value;
    const groups = ongoingGroupLists.value;
    const selected = selectedOngoingGroup.value;
    const filter = ongoingFilter.value;
    const rows = selectedOngoingRows.value;
    useEffect(() => {
        if (filter !== ongoingFilterChoice.value) setOngoingFilterChoice('all');
    }, [filter]);
    const rowIds = rows.map(n => n.thread.meta.id);
    const emptyId = hydrated && rows.length === 0 ? `ongoing-empty-${selected}` : null;
    const landing = landingGroupFor('ongoing', selected);
    const openRow = landing ? (id: string) => openOngoingThread(id, landing) : focusThread;

    const navList: DrawerNavNode[] = [
        ...ONGOING_GROUPS.map((group): DrawerNavNode => ({ kind: 'tile', group, selected: group === selected })),
        ...flatThreadNodes(rowIds, selected),
    ];
    const navKey = `${selected}:${rowIds.join(',')}`;
    usePublishedNavNodes(() => navList, navKey);

    useFlipTransitions(containerRef, [{ name: 'ongoing', ids: [ONGOING_TILES_FLIP_ID, ...rowIds, ...(emptyId ? [emptyId] : [])] }], 'ongoing', [selected, filter]);

    return (
        <div ref={containerRef} class="thread-drawer-rows ongoing-grouping">
            <OngoingTiles>
                {groups.map(g => (
                    <OngoingTile key={g.group} group={g.group} rows={g.rows} hydrated={hydrated} selected={g.group === selected} />
                ))}
            </OngoingTiles>
            {rows.map(n => n.thread.meta.state === 'composing'
                ? <ComposingThreadRow key={n.thread.meta.id} thread={n.thread} depth={n.depth} />
                : <ThreadRow
                    key={n.thread.meta.id}
                    threadId={n.thread.meta.id}
                    status={effectiveThreadStatus(n.thread)}
                    depth={n.depth}
                    offerShowInFolders
                    onOpen={openRow}
                />)}
            {emptyId && <div key={emptyId} class="empty-state" data-flip-id={emptyId}>{ONGOING_GROUP_META[selected].empty}</div>}
        </div>
    );
}

function SearchResults() {
    const loadable = threadSearchResults.value;
    const showLoading = useDelayedLoading(loadable);

    const resultIds = loadable.status === 'loaded' ? loadable.data.map((r: ThreadSearchResult) => r.thread_id) : [];
    const resultKey = resultIds.join(',');
    usePublishedNavNodes(() => flatThreadNodes(resultIds), resultKey);

    if (loadable.status === 'failed') {
        return <LoadableError noun="search results" error={loadable.error} />;
    }

    return (
        <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf row={() => <ThreadRowContent />} />}>
            {loadable.status === 'loaded' ? (
                loadable.data.length === 0 ? (
                    <div class="empty-state">No threads found</div>
                ) : (
                    <div>
                        <div class="list-section-title">
                            <SectionHeaderContent Icon={SearchIcon} title="Results" />
                        </div>
                        {loadable.data.map((r: ThreadSearchResult) => (
                            <SearchResultRow key={r.thread_id} result={r} />
                        ))}
                    </div>
                )
            ) : null}
        </LoadingFade>
    );
}

function SearchResultRow({ result }: { result: ThreadSearchResult }) {
    const liveThread = threadMap.value.get(result.thread_id);
    const status: ThreadStatus = liveThread ? effectiveThreadStatus(liveThread) : result.status;
    const visualStatus = visualStatusFor(status, liveThread?.meta);
    const section = liveThread?.meta.section ?? result.section;
    const isFocused = focusedThreadId.value === result.thread_id;
    const isHighlighted = highlightedKey.value === result.thread_id;
    const ctxFields: ThreadContextFields = liveThread?.meta ?? {
        channel: result.channel,
        triggerName: result.trigger_name,
        repoName: result.cc_repo_name,
        codingAgentKind: result.coding_agent_kind ?? undefined,
        codingAgentFolder: result.coding_agent_folder,
    };

    return (
        <ThreadRowContent
            id={result.thread_id}
            title={result.title}
            createdLabel={formatCreatedTimestamp(liveThread?.meta.createdAt ?? result.created_at)}
            channel={result.channel}
            codingAgent={liveThread?.meta.codingAgent ?? result.coding_agent ?? undefined}
            codingAgents={liveThread?.meta.codingAgents ?? result.coding_agents}
            visualStatus={visualStatus}
            contextName={threadContextName(ctxFields)}
            totalChildren={liveThread?.meta.totalChildrenCount ?? 0}
            needsReview={section === 'inbox' && status !== 'running'}
            hasDraft={threadHasUnsentDraft(liveThread)}
            isSaved={liveThread?.meta.saved ?? false}
            isFocused={isFocused}
            isHighlighted={isHighlighted}
            onClick={() => { void ensureThreadInMap(result); focusThread(result.thread_id); }}
        />
    );
}
