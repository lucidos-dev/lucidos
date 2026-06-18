import type { ComponentChildren } from 'preact';
import { useRef, useEffect, useCallback } from 'preact/hooks';
import { memo } from 'preact/compat';
import { signal } from '@preact/signals';
import { threadDrawerOpen, threadDrawerWidth, threadMap, focusedThreadId, threadChannelFilter, selectedTriggerIds, selectedRepoIds, selectedAppIds, threadsLoaded, splitRatio, effectiveThreadStatus, getThreadDisplaySection, threadSearchQuery, threadSearchResults, threadHasMore, threadLoadingMore, archiveThreadCount, draftsViewActive, attentionViewActive, selectedCodingAgent, selectedScope, repositories } from '../../store/store';
import { composeDraftContextName } from '../../store/composeDestination';
import { threadPassesChannelFilter } from '../../store/threadFilter';
import { navigateToPane, focusPane } from '../../store/actions/pane';
import { focusThread } from '../../store/actions/threads';
import { loadOlderThreads, reloadAfterFilterChange, ensureThreadInMap } from '../../store/actions/thread-loading';
import { ThreadStatusIcon, resolveVisualStatus, type VisualStatus } from '../shared/ThreadStatusIcon';
import { CopyThreadRefButton } from '../shared/CopyThreadRefButton';
import type { ThreadState, ThreadStatus } from '../../store/thread-events';
import { getDraft } from '../../store/composeDrafts';
import type { DisplaySection } from '../../generated/thread-lifecycle';
import { formatThreadChannelLabel } from '../../utils/formatChannel';
import { threadRowTooltip, threadContextName, type ThreadContextFields } from './threadRowInfo';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { useFlipTransitions } from '../../hooks/useFlipAnimation';
import { useDelayedLoading, useLingeringFlag } from '../../hooks/useDelayedLoading';
import { PANE_TRANSITION_MS } from '../layout/splitHelpers';
import { useScrollMemory } from '../../hooks/useScrollMemory';
import { isMobile } from '../../utils/viewport';
import type { ThreadSearchResult } from '../../api/threads';

// `threadPassesChannelFilter` lives in `store/threadFilter.ts` (shared with the
// infinite-scroll cursor in `thread-loading.ts`); re-exported here so existing
// importers (and tests) keep their path.
export { threadPassesChannelFilter };

export const THREAD_DRAWER_SECTION_ORDER: readonly DisplaySection[] = ['saved', 'current', 'archive'];

/** True when an event target lies within a thread row (`[data-thread-nav]`).
 *  Tolerates non-Element / null targets — a missing `closest` reads as
 *  "not a thread row". */
export function isThreadRowTarget(target: EventTarget | null): boolean {
    return !!(target as Element | null)?.closest?.('[data-thread-nav]');
}

/** Pointer-down handler for the drawer pane. Focuses the drawer pane on a click
 *  on its chrome (section headers, empty space, scrollbar). A click on a thread
 *  row is exempt: that click focuses a *thread*, which focuses the thread pane
 *  (the prompt input's `onFocus` → `focusPane('thread')`). Pre-focusing the
 *  drawer here would glide the header focus wash to the drawer and then straight
 *  back to the thread on the same click. */
export function handleDrawerPointerDown(target: EventTarget | null): void {
    if (isThreadRowTarget(target)) return;
    focusPane('drawer');
}

/** Currently keyboard-highlighted thread ID in the drawer. */
const highlightedThreadId = signal<string | null>(null);

/** Ordered list of navigable thread IDs, set by ThreadList or SearchResults. */
const navigableIds = signal<string[]>([]);

export function selectHighlighted() {
    const id = highlightedThreadId.value;
    if (!id) return;
    const searchResult = threadSearchResults.value;
    if (threadSearchQuery.value.trim().length > 0 && searchResult.status === 'loaded') {
        const match = searchResult.data.find((r: ThreadSearchResult) => r.thread_id === id);
        if (match) void ensureThreadInMap(match);
    }
    focusThread(id);
}

/** Collapse (`collapse: true`) or expand (`collapse: false`) the highlighted
 *  thread's family. No-op (returns false) when no thread is highlighted, the
 *  highlighted thread has no children (no chevron), or the family is already
 *  in the requested state. The caller uses the return value to decide whether
 *  to consume the keystroke. */
function setHighlightedFamilyCollapse(collapse: boolean): boolean {
    const id = highlightedThreadId.value;
    if (!id) return false;
    const thread = threadMap.value.get(id);
    if (!thread || thread.meta.totalChildrenCount === 0) return false;
    const isCollapsed = collapsedFamilies.value.has(id);
    if (isCollapsed === collapse) return false;
    toggleFamilyCollapse(id);
    return true;
}

export function moveHighlight(delta: number) {
    const ids = navigableIds.value;
    if (ids.length === 0) return;
    const current = highlightedThreadId.value;
    const idx = current ? ids.indexOf(current) : -1;
    let next: number;
    if (idx === -1) {
        next = delta > 0 ? 0 : ids.length - 1;
    } else {
        next = idx + delta;
        if (next < 0) next = 0;
        if (next >= ids.length) next = ids.length - 1;
    }
    highlightedThreadId.value = ids[next];
    // Scroll highlighted row into view
    const el = document.querySelector(`[data-thread-nav="${ids[next]}"]`);
    el?.scrollIntoView({ block: 'nearest' });
}

export function ThreadDrawer({ forceVisible }: { forceVisible?: boolean } = {}) {
    // On mobile, the drawer overlay is disabled — mobile has a dedicated threads
    // pane (pane 0) that keeps dots, header, and content in sync via mobileView.
    // The drawer overlay (threadDrawerOpen) is desktop-only.
    const visible = forceVisible || (threadDrawerOpen.value && splitRatio.value > 0);
    // Keep the list mounted through the width-collapse transition —
    // unmounting at close start blanks the drawer body while it's still
    // sliding shut.
    const renderContent = useLingeringFlag(visible, PANE_TRANSITION_MS + 50);
    const isSearching = threadSearchQuery.value.trim().length > 0;
    const isDraftsMode = !isSearching && draftsViewActive.value;
    const isAttentionMode = !isSearching && !isDraftsMode && attentionViewActive.value;

    const listRef = useRef<HTMLDivElement>(null);
    // Don't restore while in an alternate view — saved offset is for the full list.
    useScrollMemory(listRef, 'lucidos-scroll-thread-drawer', { paused: isSearching || isDraftsMode || isAttentionMode });

    const handleKeyDown = useCallback((e: KeyboardEvent) => {
        if (e.key === 'ArrowDown') {
            e.preventDefault();
            moveHighlight(1);
        } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            moveHighlight(-1);
        } else if (e.key === 'Enter') {
            e.preventDefault();
            selectHighlighted();
        } else if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
            // Only intercept when the highlighted row is a parent — otherwise
            // let the default keystroke through (no row has anything to do
            // with horizontal arrow keys).
            if (setHighlightedFamilyCollapse(e.key === 'ArrowLeft')) {
                e.preventDefault();
            }
        }
    }, []);

    useEffect(() => {
        highlightedThreadId.value = null;
    }, [isSearching, isDraftsMode, isAttentionMode]);

    return (
        <div class={`thread-drawer${visible ? '' : ' thread-drawer-collapsed'}`}
             style={visible ? { width: `${threadDrawerWidth.value}px` } : undefined}
             onKeyDown={handleKeyDown}
             onPointerDown={(e) => handleDrawerPointerDown(e.target)}
             tabIndex={-1}>
            <div class="thread-drawer-list" ref={listRef}>
                {renderContent && (
                    isSearching ? <SearchResults />
                    : isDraftsMode ? <DraftsList />
                    : isAttentionMode ? <AttentionList />
                    : <ThreadList />
                )}
            </div>
        </div>
    );
}


import { attentionThreads, categorizeThreads, composingThreads, computeFamilyDecorations, computeFamilyGraph, computeFamilyKeys, computeFamilySections, depthStyle, draftThreads, filterByTopThread, hasCollapsedAncestor, nestByParent, threadHasUnsentDraft } from './family-graph';
import { byCreated } from '../../store/thread-events';
import type { FamilyDecorations, FamilyGraph, NestedThread, ThreadSections } from './family-graph';
export * from './family-graph';
function ThreadList() {
    const containerRef = useRef<HTMLDivElement>(null);
    const portalRef = useRef<HTMLDivElement>(null);
    const sentinelRef = useRef<HTMLDivElement>(null);
    const hydrated = threadsLoaded.value;

    const filter = threadChannelFilter.value;
    // Empty channel filter means "show nothing" — including composing threads.
    // Otherwise the composing drafts would be the only thing visible.
    const composingList = filter.size === 0 ? [] : composingThreads(threadMap.value);

    let categorized: ThreadSections;
    let familyGraph: FamilyGraph = { byId: new Map(), rootByThread: new Map() };
    let decorations: FamilyDecorations = { routedByThread: new Map(), liftedRoots: new Set() };
    if (hydrated) {
        const triggerSelection = selectedTriggerIds.value;
        const repoSelection = selectedRepoIds.value;
        const appSelection = selectedAppIds.value;
        // Filter at top-thread scope so a family is shown iff its top-thread
        // passes — a per-thread filter would drop sub-threads while the
        // parent row still advertised them via meta.totalChildrenCount.
        // Share the same graph across routing, sort keys, decorations, and
        // the collapse filter; rebuilding it for the filtered subset would
        // double the per-render parent-walk cost.
        //
        // When a trigger/repo/app sub-selection is active, switch to any-member
        // matching: the repo/app/trigger is a property of a specific coding-agent thread,
        // not its (often chat/trigger) top-thread, so a coding-agent thread in the
        // selected repo/app must surface with its family even when the root's
        // channel is filtered out.
        const subSelectionActive =
            triggerSelection.size > 0 || repoSelection.size > 0 || appSelection.size > 0;
        const unfilteredArr = Array.from(threadMap.value.values());
        familyGraph = computeFamilyGraph(unfilteredArr);
        const allThreads = filterByTopThread(unfilteredArr, familyGraph, t =>
            threadPassesChannelFilter(t, filter, triggerSelection, repoSelection, appSelection),
            subSelectionActive,
        );
        const familySections = computeFamilySections(allThreads, familyGraph);
        categorized = categorizeThreads(allThreads, familyGraph, familySections);
        decorations = computeFamilyDecorations(allThreads, familyGraph, familySections);
        const familyKeys = computeFamilyKeys(allThreads, familyGraph);
        const byFamilyRecent = (a: ThreadState, b: ThreadState) =>
            familyKeys.get(b.meta.id)!.recentKey.localeCompare(familyKeys.get(a.meta.id)!.recentKey);
        // Current sorts by creation time (newest first) — a stable order that
        // doesn't reshuffle as agents churn or threads gain a CTA. Attention and
        // drafts are surfaced by the header filter icons, not by bubbling. Saved
        // / Archive still sort by the family's freshest user action.
        categorized.current.sort(byCreated);
        categorized.saved.sort(byFamilyRecent);
        categorized.archive.sort(byFamilyRecent);
    } else {
        categorized = {
            current: [], saved: [], archive: [],
            statusMap: new Map(),
        };
    }
    const { current, saved, archive, statusMap } = categorized;

    // Drop descendants of collapsed families. The parent itself stays visible
    // (its disclosure chevron lets the user re-expand).
    const collapsedFamiliesSet = collapsedFamilies.value;
    const filterCollapsed = (nested: NestedThread[]) =>
        nested.filter(n => !hasCollapsedAncestor(n.thread.meta.id, collapsedFamiliesSet, familyGraph));
    // `count` is the section's full thread total (parents + sub-threads, before
    // family-collapse filtering) — the number shown in the collapsed-section
    // badge. `threads` is the post-collapse render list. Archive is special:
    // its badge reads the server-sourced `archiveThreadCount` (the true count of
    // archived threads matching the active filter — see `refreshArchivedCount`)
    // so it stays stable as rows page in and never drifts when the user
    // collapses/expands the section.
    const archiveCount = archiveThreadCount.value;
    // Composing drafts ride at the top of Current (most-recent-first, already
    // sorted by `composingThreads`), ahead of the family-sorted current rows.
    const sectionByName: Record<DisplaySection, { name: DisplaySection; count: number; threads: NestedThread[] }> = {
        saved: { name: 'saved', count: saved.length, threads: filterCollapsed(nestByParent(saved)) },
        current: {
            name: 'current',
            count: composingList.length + current.length,
            threads: [...nestByParent(composingList), ...filterCollapsed(nestByParent(current))],
        },
        archive: { name: 'archive', count: archiveCount, threads: filterCollapsed(nestByParent(archive)) },
    };
    const sections = THREAD_DRAWER_SECTION_ORDER.map(name => sectionByName[name]);

    const sectionDefs = sections
        .filter(s => s.threads.length > 0)
        .map(s => ({
            name: s.name,
            ids: [
                `__section_${s.name}`,
                ...s.threads.map(n => n.thread.meta.id),
            ],
        }));

    // Build flat navigable ID list from visible (non-collapsed) sections
    const collapsed = collapsedSections.value;
    const flatIds: string[] = [];
    for (const s of sections) {
        if (collapsed.has(s.name)) continue;
        flatIds.push(...s.threads.map(n => n.thread.meta.id));
    }
    const flatKey = flatIds.join(',');
    useEffect(() => { navigableIds.value = flatIds; }, [flatKey]);

    useFlipTransitions(containerRef, portalRef, sectionDefs, filter);

    // Infinite scroll, part 1 — the fill loop. Keep loading until the sentinel
    // is pushed back out of view (or there's nothing more). This loop is
    // load-bearing: an IntersectionObserver fires only on enter/exit
    // *transitions*, so when a page doesn't refill the viewport (a sparse
    // filtered result, or a page that adds few visible rows) the sentinel stays
    // intersecting with no new event and pagination stalls — which is why it
    // previously only advanced when a collapse/expand re-layout forced a fresh
    // intersection. `fillingRef` makes the loop re-entrancy-safe so a scroll
    // event firing the observer mid-fill can't double-load.
    const fillingRef = useRef(false);
    const loadWhileSentinelVisible = useCallback(async () => {
        if (fillingRef.current) return;
        const sentinel = sentinelRef.current;
        const root = sentinel?.closest('.thread-drawer-list');
        if (!sentinel || !root) return;
        fillingRef.current = true;
        try {
            // Runaway backstop. Filling a viewport needs ~2-3 pages; this cap is
            // far above that. It guards the pathological case where loaded rows
            // render NOWHERE visible (e.g. filter matches that are all
            // collapsed-family descendants, dropped by `filterCollapsed`): then
            // the map grows every page (so the size guard never trips) and the
            // sentinel never moves, so without a cap one intersection would page
            // through the whole matching set synchronously. Subsequent scrolls
            // resume pagination normally.
            const MAX_PAGES_PER_FILL = 40;
            let pages = 0;
            while (
                pages < MAX_PAGES_PER_FILL &&
                threadHasMore.value &&
                archivePaginationAllowed(collapsedSections.value) &&
                sentinelInView(sentinel.getBoundingClientRect(), root.getBoundingClientRect())
            ) {
                const before = threadMap.value.size;
                await loadOlderThreads();
                pages++;
                // Let the re-render commit so the next iteration measures the new
                // layout (whether the freshly-loaded rows pushed the sentinel out).
                await new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
                // Defensive stop: loadOlderThreads already flips threadHasMore off
                // when a page adds nothing, but bail if the map didn't grow so a
                // stuck cursor can never spin this loop.
                if (threadMap.value.size === before) break;
            }
        } finally {
            fillingRef.current = false;
        }
    }, []);

    // When the channel / trigger / repo / app filter changes, re-arm pagination
    // AND eagerly fetch the first page of matching threads, then fill the
    // viewport. A different filter is a different cursor space, and the matches
    // may be entirely outside the loaded window (e.g. a repo whose threads are
    // all archived) — relying on the IntersectionObserver alone strands the user
    // because it only re-fires on a scroll transition. `reloadAfterFilterChange`
    // makes population deterministic; `loadWhileSentinelVisible` then tops the
    // viewport off. The prevRef guard suppresses the mount run (no fetch on
    // first render; the initial window load + the observer cover that case).
    const currentTriggers = selectedTriggerIds.value;
    const currentRepos = selectedRepoIds.value;
    const currentApps = selectedAppIds.value;
    const prevFilterRef = useRef(filter);
    const prevTriggersRef = useRef(currentTriggers);
    const prevReposRef = useRef(currentRepos);
    const prevAppsRef = useRef(currentApps);
    useEffect(() => {
        if (prevFilterRef.current !== filter
            || prevTriggersRef.current !== currentTriggers
            || prevReposRef.current !== currentRepos
            || prevAppsRef.current !== currentApps) {
            prevFilterRef.current = filter;
            prevTriggersRef.current = currentTriggers;
            prevReposRef.current = currentRepos;
            prevAppsRef.current = currentApps;
            void reloadAfterFilterChange().then(() => void loadWhileSentinelVisible());
        }
    }, [filter, currentTriggers, currentRepos, currentApps]);

    const hasMore = threadHasMore.value;

    useEffect(() => {
        const sentinel = sentinelRef.current;
        if (!sentinel || !hydrated || !hasMore) return;

        const observer = new IntersectionObserver(
            (entries) => {
                if (entries[0]?.isIntersecting) void loadWhileSentinelVisible();
            },
            { root: sentinel.closest('.thread-drawer-list'), threshold: 0 },
        );
        observer.observe(sentinel);
        // Kick once: a short list whose sentinel is already visible produces no
        // scroll transition, so without this the first viewport never fills.
        void loadWhileSentinelVisible();
        return () => observer.disconnect();
    }, [hydrated, hasMore, loadWhileSentinelVisible]);

    if (!hydrated) {
        return (
            <>
                <div ref={containerRef} />
                <div ref={portalRef} class="flip-portal" />
            </>
        );
    }

    const loadingMore = threadLoadingMore.value;

    return (
        <>
            <div ref={containerRef}>
                {sections.map(s => {
                    if (s.threads.length === 0) return null;
                    const title = s.name.charAt(0).toUpperCase() + s.name.slice(1);
                    return (
                        <DrawerSection key={s.name} sectionKey={s.name} title={title} count={s.count}>
                            {s.threads.map(n => {
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
                                        enableFamilyToggle
                                    />
                                );
                            })}
                        </DrawerSection>
                    );
                })}
                {sections.every(s => s.threads.length === 0) && (
                    <div class="empty-state">No threads</div>
                )}
                {hasMore && (
                    <div ref={sentinelRef} class="thread-drawer-load-more">
                        {loadingMore && <span class="thread-drawer-loading">Loading...</span>}
                    </div>
                )}
            </div>
            <div ref={portalRef} class="flip-portal" />
        </>
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

/** Shared collapsed state so ThreadList can read it for navigableIds. */
const collapsedSections = signal(loadStringSet(COLLAPSED_KEY));

/** Per-family collapse state keyed by parent thread id. Mirrors the
 *  `collapsedSections` precedent: localStorage-backed, per-device, not
 *  event-sourced. */
const collapsedFamilies = signal(loadStringSet(COLLAPSED_FAMILIES_KEY));

export function toggleFamilyCollapse(threadId: string) {
    const next = new Set(collapsedFamilies.value);
    if (next.has(threadId)) next.delete(threadId);
    else next.add(threadId);
    collapsedFamilies.value = next;
    saveStringSet(COLLAPSED_FAMILIES_KEY, next);
}

// Skip pagination when Archive is collapsed: collapsing shrinks the list, pops
// the sentinel into view, and would otherwise let the fill loop pull the ENTIRE
// archive into memory while every row is hidden — pointless fetch + render churn
// the user can't see. Archive is the bottom section and the one that absorbs
// paginated older threads.
//
// Unlike the prior version, there is NO filter-active bypass: the collapsed
// badge now reads the server-sourced `archiveThreadCount` (see
// `refreshArchivedCount`), so a filter whose matches are all archived shows its
// true count even while collapsed — no need to eager-load hidden rows to count
// them. When the user expands the section the fill loop loads the matches, and
// `reloadAfterFilterChange` already fetches the first page on every filter
// change, so selecting a facet is never stranded on "No threads".
export function archivePaginationAllowed(collapsed: ReadonlySet<string>): boolean {
    return !collapsed.has('archive');
}

// Whether the infinite-scroll sentinel overlaps the scroll container's
// viewport (vertical axis). The fill loop keeps loading while this is true so a
// page that doesn't push the sentinel below the fold doesn't stall pagination.
// Pure rect math, extracted for testability (no IntersectionObserver / DOM).
export function sentinelInView(sentinel: { top: number; bottom: number }, root: { top: number; bottom: number }): boolean {
    return sentinel.top < root.bottom && sentinel.bottom > root.top;
}

function DrawerSection({ sectionKey, title, count, children }: { sectionKey: string; title: string; count: number; children: ComponentChildren }) {
    const collapsed = collapsedSections.value.has(sectionKey);

    const toggle = () => {
        const next = new Set(collapsedSections.value);
        if (collapsed) next.delete(sectionKey);
        else next.add(sectionKey);
        collapsedSections.value = next;
        saveStringSet(COLLAPSED_KEY, next);
    };

    return (
        <div class={`drawer-section${collapsed ? ' drawer-section-collapsed' : ''}`}>
            <div class={`list-section-title list-section-title-collapsible${collapsed ? ' collapsed' : ''}`}
                 data-flip-id={`__section_${sectionKey}`}
                 onClick={toggle}
                 role="button"
                 aria-expanded={!collapsed}>
                {title}
                {/* Thread count rides in a badge only while the section is
                    collapsed — expanded sections show the rows themselves. */}
                {collapsed && <span class="collapse-count-badge">{count}</span>}
            </div>
            {!collapsed && children}
        </div>
    );
}

export function ComposingThreadRow({ thread, depth = 0, onAfterClick }: { thread: ThreadState; depth?: number; onAfterClick?: () => void }) {
    const isFocused = focusedThreadId.value === thread.meta.id;
    const isHighlighted = highlightedThreadId.value === thread.meta.id;
    const classes = ['list-row', 'thread-row', 'compose-draft-row'];
    if (isFocused) classes.push('thread-row-focused');
    if (isHighlighted) classes.push('thread-row-highlighted');
    // A coding draft hasn't bound a backend yet — it spawns with the device's
    // current `selectedCodingAgent` at send time (see sendCompose), so the tag
    // reflects that pick (Codex vs Claude Code) rather than always "Claude Code".
    // A plain chat draft is a Lucidos thread, which carries no channel tag —
    // `formatThreadChannelLabel('chat')` returns '' and the chip is skipped.
    const draftMode = getDraft(thread.meta.id).mode;
    const modeLabel = draftMode === 'claude_code'
        ? formatThreadChannelLabel('claude_code', selectedCodingAgent.value)
        : formatThreadChannelLabel('chat');
    // The repo/app chip mirrors started threads. A coding draft hasn't bound its
    // meta yet, so it reads the device-global `selectedScope` (the same value
    // `sendCompose` would bind) rather than `meta.repoName`/`codingAgentKind`.
    const reposLoadable = repositories.value;
    const contextName = composeDraftContextName(
        draftMode,
        selectedScope.value,
        reposLoadable.status === 'loaded' ? reposLoadable.data : [],
    );

    return (
        <div data-flip-id={thread.meta.id} style={depthStyle(depth)} class={depth > 0 ? 'thread-row-wrap is-nested' : 'thread-row-wrap'}>
            {/* Dot lives on the wrapper (outside the row's nested clip-path) so it
                holds a fixed left column at every depth — matching ThreadRowContent. */}
            <ThreadStatusIcon status="idle" />
            <div class={classes.join(' ')}
                 data-thread-nav={thread.meta.id}
                 onClick={() => {
                     focusThread(thread.meta.id);
                     onAfterClick?.();
                     if (isMobile()) navigateToPane('thread');
                 }}>
                <div class="thread-row-left">
                    <span class="thread-row-title-row">
                        <span class="thread-row-title">{threadDisplayTitle(thread)}</span>
                        <span class="draft-indicator" data-tooltip="Has unsent draft">Draft</span>
                    </span>
                </div>
                <div class="thread-row-right">
                    {modeLabel && <span class="label message-channel-tag">{modeLabel}</span>}
                    {contextName && <span class="label thread-row-context">{contextName}</span>}
                </div>
            </div>
        </div>
    );
}

interface ThreadRowContentProps {
    id: string;
    /** Nesting depth — 0 for top-level / search rows, ≥1 for sub-threads.
     *  Drives the wrapper's `is-nested` class + `--thread-depth` indent var. */
    depth?: number;
    /** FLIP-animation key, set only by the nested ThreadList. Search / drafts
     *  render flat lists that don't animate, so they omit it (no `data-flip-id`). */
    flipId?: string;
    title: string;
    channel: string;
    /** Coding-agent backend for `claude_code`-channel threads — drives the
     *  "Codex" vs "Claude Code" channel tag. Absent for non-coding-agent/legacy rows. */
    codingAgent?: 'claude-code' | 'codex';
    /** Precomputed status dot, derived by the caller via `resolveVisualStatus`
     *  from the snapshot `status` (the same `effectiveThreadStatus` the panel
     *  header uses) — so every drawer row's dot is built in one pass and stays
     *  in lockstep with the list, instead of being re-read live per row. */
    visualStatus: VisualStatus;
    /** Structured hover/long-press tooltip (Status / You / Agent / Context /
     *  Exchanges / Started), pre-serialized to a JSON `TooltipRow[]` so the memo
     *  equality check can compare it as a plain string. Empty string → no
     *  tooltip (e.g. an unhydrated search hit). */
    tooltip: string;
    /** Repo / app / trigger name chip shown next to the channel tag. Undefined
     *  for plain chat — a Lucidos thread carries no channel tag and no context
     *  name; the bare row (no chips) is itself the signal it's a regular chat. */
    contextName?: string;
    totalChildren: number;
    needsReview: boolean;
    hasDraft: boolean;
    isFocused: boolean;
    isHighlighted: boolean;
    /** Set when this row is the root of a lifted family — its own natural
     *  section is lower-priority than where the family was routed. Drives the
     *  demoted-parent styling so the row reads as "I'm here under protest". */
    isLiftedParent?: boolean;
    /** Set when this row is a non-root descendant whose natural section
     *  matches the routed section, inside a lifted family — i.e. the row that
     *  earned the lift. Drives the bright accent rail. */
    isResponsibleChild?: boolean;
    /** True when this row anchors a family with sub-threads AND we're in the
     *  nested ThreadList (not search / drafts, which render flat). Enables the
     *  bottom-left disclosure chevron + collapsed-state count. */
    collapsible?: boolean;
    /** Whether this family is currently collapsed (children hidden). */
    isCollapsed?: boolean;
    /** Toggle this family's collapse state. Excluded from the memo equality
     *  check (like `onClick`) — the closure changes per parent render but
     *  always closes over the same thread id. */
    onToggleFamily?: () => void;
    onClick: () => void;
}

function ThreadRowContentImpl(props: ThreadRowContentProps) {
    const depth = props.depth ?? 0;
    const hasFamily = !!props.collapsible && props.totalChildren > 0;

    const classes = ['list-row', 'thread-row'];
    if (props.isFocused) classes.push('thread-row-focused');
    if (props.isHighlighted) classes.push('thread-row-highlighted');
    if (props.needsReview) classes.push('thread-row-review');
    if (props.isLiftedParent) classes.push('thread-row-lifted-parent');
    if (props.isResponsibleChild) classes.push('thread-row-lifted-child');
    // Lets the CSS reserve bottom room in the title column for the absolutely-
    // positioned disclosure badge, so a long (multi-line) title can't grow down
    // into it (see `.thread-row-has-family .thread-row-left` in drawer.css).
    if (hasFamily) classes.push('thread-row-has-family');

    const wrapClasses = ['thread-row-wrap'];
    if (depth > 0) wrapClasses.push('is-nested');

    // aria stays a smart-plural bare count; the visible sub-thread count rides
    // in a badge shown only while the family is collapsed (expanded families
    // show their children inline, so the number would be redundant).
    const a11yCount = `${props.totalChildren} sub-thread${props.totalChildren === 1 ? '' : 's'}`;
    // The disclosure control carries its OWN tooltip + aria-label so hovering the
    // badge/chevron shows what the control does — not the row's general thread
    // tooltip. The global tooltip system walks up to the nearest `data-tooltip`
    // ancestor (useTooltip `findTarget`), so without this the badge/chevron
    // inherited the row's `data-tooltip` and showed the thread blurb. Collapsed
    // names the hidden count ("Show N sub-threads"); expanded is just "Hide
    // sub-threads" — the children are listed inline, so repeating the count
    // would be redundant (same reason the count badge is collapsed-only).
    const disclosureLabel = props.isCollapsed ? `Show ${a11yCount}` : 'Hide sub-threads';

    // Plain chat / Lucidos threads have no channel tag (empty label) — render the
    // chip only when there's a label, so the row doesn't paint an empty bordered
    // chip for a regular Lucidos thread.
    const channelLabel = formatThreadChannelLabel(props.channel, props.codingAgent);

    // The status dot is the wrapper's child, not the row's, so it stays in a
    // fixed left column at every depth — anchored to the un-indented wrapper
    // instead of tracking the title's per-depth indent (see drawer.css).
    return (
        <div class={wrapClasses.join(' ')}
             style={depthStyle(depth)}
             {...(props.flipId ? { 'data-flip-id': props.flipId } : {})}>
            <ThreadStatusIcon status={props.visualStatus} />
            <div class={classes.join(' ')}
                 data-thread-nav={props.id}
                 {...(props.tooltip ? { 'data-tooltip-rows': props.tooltip, 'data-tooltip-title': props.title, 'data-tooltip-longpress': '' } : {})}
                 onClick={props.onClick}>
                {hasFamily && (
                    <button
                        type="button"
                        class="family-disclosure"
                        onClick={(e) => { e.stopPropagation(); props.onToggleFamily?.(); }}
                        data-tooltip={disclosureLabel}
                        data-tooltip-longpress=""
                        aria-label={disclosureLabel}
                        aria-expanded={!props.isCollapsed}>
                        {/* Collapsed → the count badge alone signals hidden
                            sub-threads (no chevron); clicking it expands. Expanded →
                            the chevron is the affordance to collapse back. It points
                            UP (▴): the control sits at the bottom of the parent, above
                            the expanded sub-threads, so up reads as "pull these back
                            up / hide them". A down chevron here would read as the
                            opposite — "expand / more below". */}
                        {props.isCollapsed
                            ? <span class="collapse-count-badge">{props.totalChildren}</span>
                            : <span class="family-disclosure-glyph" aria-hidden="true">▴</span>}
                    </button>
                )}
                <div class="thread-row-left">
                    <span class="thread-row-title-row">
                        <span class="thread-row-title">{props.title}</span>
                        {props.hasDraft && <span class="draft-indicator" data-tooltip="Has unsent draft">Draft</span>}
                    </span>
                </div>
                <div class="thread-row-right">
                    {channelLabel && <span class={`label message-channel-tag${props.channel === 'error_unknown_channel' ? ' channel-error' : ''}`}>{channelLabel}</span>}
                    {props.contextName && <span class="label thread-row-context">{props.contextName}</span>}
                    <span class="thread-row-actions">
                        <CopyThreadRefButton threadId={props.id} title={props.title} stopPropagation extraClass="thread-row-action" />
                    </span>
                </div>
            </div>
        </div>
    );
}

/** Skip the render when nothing the row paints has changed. Drawer flushes
 *  fire on every SSE event in the workspace — without memo, all 100+ visible
 *  rows re-execute their render function and reconcile their VDOM even when
 *  only one thread's state moved. `onClick` is intentionally excluded from
 *  the equality check: the closure changes per parent render, but it always
 *  closes over the same threadId, so a "stale" reference still does the
 *  right thing. */
const ThreadRowContent = memo(ThreadRowContentImpl, (prev, next) =>
    prev.id === next.id
    && prev.depth === next.depth
    && prev.flipId === next.flipId
    && prev.title === next.title
    && prev.channel === next.channel
    && prev.codingAgent === next.codingAgent
    && prev.visualStatus === next.visualStatus
    && prev.tooltip === next.tooltip
    && prev.contextName === next.contextName
    && prev.totalChildren === next.totalChildren
    && prev.needsReview === next.needsReview
    && prev.hasDraft === next.hasDraft
    && prev.isFocused === next.isFocused
    && prev.isHighlighted === next.isHighlighted
    && prev.isLiftedParent === next.isLiftedParent
    && prev.isResponsibleChild === next.isResponsibleChild
    && prev.collapsible === next.collapsible
    && prev.isCollapsed === next.isCollapsed
);

// Reading composeDrafts here would fan a re-render to every visible ThreadRow
// per keystroke — the lag this signal was added to prevent.

export function ThreadRow({ threadId, status, depth = 0, isLiftedParent, isResponsibleChild, enableFamilyToggle, onAfterClick }: {
    threadId: string;
    status: ThreadStatus;
    depth?: number;
    isLiftedParent?: boolean;
    isResponsibleChild?: boolean;
    /** Render the bottom-left disclosure chevron when this thread has
     *  sub-threads. Only the nested ThreadList sets it — search / drafts render
     *  flat lists where collapsing nothing visible would be a no-op. */
    enableFamilyToggle?: boolean;
    onAfterClick?: () => void;
}) {
    // Signal reads stay here so each row's subscription set is narrow: the
    // row re-renders on threadMap / focusedThreadId / highlightedThreadId /
    // draftPresentThreadIds, but the memo on ThreadRowContent below short-
    // circuits when none of the primitives it derives actually changed —
    // turning a per-flush N-row VDOM storm into one render per moved row.
    const thread = threadMap.value.get(threadId);
    if (!thread) return null;
    const { meta } = thread;
    // Status dot from the snapshot `status` prop (the list builder derived it
    // from the same `effectiveThreadStatus` the panel uses), NOT a live re-read
    // of `meta.status`. ThreadRow re-renders on `focusedThreadId` changes, which
    // do NOT flush `threadMap`; a live `meta.status` read here made the focused
    // row repaint its dot to a value that diverged from the rest of the list
    // (which still shows the snapshot) until the next flush — the "focused
    // thread's running dot disappears" bug. Feeding the snapshot `status` through
    // the shared `resolveVisualStatus` formula keeps every row's dot in lockstep.
    const visualStatus = resolveVisualStatus(
        status,
        meta.activeChildrenCount > 0,
        meta.codingAgentProposed,
    );
    const isFocused = focusedThreadId.value === meta.id;
    const isHighlighted = highlightedThreadId.value === meta.id;
    const hasDraft = threadHasUnsentDraft(thread);
    const hasFamily = !!enableFamilyToggle && meta.totalChildrenCount > 0;
    const isCollapsed = hasFamily && collapsedFamilies.value.has(meta.id);

    return (
        <ThreadRowContent
            id={meta.id}
            depth={depth}
            flipId={meta.id}
            title={threadDisplayTitle(thread)}
            channel={meta.channel}
            codingAgent={meta.codingAgent}
            visualStatus={visualStatus}
            tooltip={JSON.stringify(threadRowTooltip(meta, status))}
            contextName={threadContextName(meta)}
            totalChildren={meta.totalChildrenCount}
            needsReview={meta.section === 'inbox' && status !== 'running'}
            hasDraft={hasDraft}
            isFocused={isFocused}
            isHighlighted={isHighlighted}
            isLiftedParent={isLiftedParent}
            isResponsibleChild={isResponsibleChild}
            collapsible={hasFamily}
            isCollapsed={isCollapsed}
            onToggleFamily={() => toggleFamilyCollapse(meta.id)}
            onClick={() => { focusThread(meta.id); onAfterClick?.(); }}
        />
    );
}

/** Single-section view of every thread carrying an unsent draft. Bypasses the
 *  channel/trigger/repo filters and the four lifecycle sections — when the user
 *  toggles the drafts icon they want every draft, not just the ones the active
 *  filter would surface. Drafts come from threads already in `threadMap`; older
 *  draft-bearing threads outside the pagination window are not loaded on
 *  demand, which is acceptable because drafts are by definition recently
 *  touched and ride at the top of the window. */
function DraftsList() {
    const hydrated = threadsLoaded.value;
    const drafts = hydrated ? draftThreads(threadMap.value) : [];

    const ids = drafts.map(t => t.meta.id);
    const idsKey = ids.join(',');
    useEffect(() => { navigableIds.value = ids; }, [idsKey]);

    if (!hydrated) return null;
    if (drafts.length === 0) {
        return <div class="empty-state">No drafts</div>;
    }
    return (
        <div>
            <div class="list-section-title">Drafts</div>
            {drafts.map(t => t.meta.state === 'composing'
                ? <ComposingThreadRow key={t.meta.id} thread={t} />
                : <ThreadRow key={t.meta.id} threadId={t.meta.id} status={effectiveThreadStatus(t)} />)}
        </div>
    );
}

/** Single-section view of every Current/Saved thread needing the user's
 *  attention — awaiting an answer/permission, a failed turn, or a change ready to
 *  apply (see `threadNeedsAttention`). Mirrors `DraftsList`: bypasses the
 *  channel/trigger/repo filters and the four lifecycle sections so the user sees
 *  everything that needs them in one place, flat and most-recent-first. Same
 *  pagination caveat as drafts — attention threads ride at the top of the loaded
 *  window. */
function AttentionList() {
    const hydrated = threadsLoaded.value;
    const threads = hydrated ? attentionThreads(threadMap.value) : [];

    const ids = threads.map(t => t.meta.id);
    const idsKey = ids.join(',');
    useEffect(() => { navigableIds.value = ids; }, [idsKey]);

    if (!hydrated) return null;
    if (threads.length === 0) {
        return <div class="empty-state">Nothing needs attention</div>;
    }
    return (
        <div>
            <div class="list-section-title">Needs attention</div>
            {threads.map(t => <ThreadRow key={t.meta.id} threadId={t.meta.id} status={effectiveThreadStatus(t)} />)}
        </div>
    );
}

function SearchResults() {
    const loadable = threadSearchResults.value;
    const showLoading = useDelayedLoading(loadable);

    const resultIds = loadable.status === 'loaded' ? loadable.data.map((r: ThreadSearchResult) => r.thread_id) : [];
    const resultKey = resultIds.join(',');
    useEffect(() => { navigableIds.value = resultIds; }, [resultKey]);

    if (loadable.status === 'failed') {
        return <div class="empty-state error-text">Search failed</div>;
    }
    if (loadable.status !== 'loaded') {
        if (!showLoading) return null;
        return <div class="loading-spinner" />;
    }

    if (loadable.data.length === 0) {
        return <div class="empty-state">No threads found</div>;
    }

    return (
        <div>
            <div class="list-section-title">Results</div>
            {loadable.data.map((r: ThreadSearchResult) => (
                <SearchResultRow key={r.thread_id} result={r} />
            ))}
        </div>
    );
}

function SearchResultRow({ result }: { result: ThreadSearchResult }) {
    const liveThread = threadMap.value.get(result.thread_id);
    // Prefer live status from threadMap (SSE-updated), fall back to API result
    const status: ThreadStatus = liveThread ? effectiveThreadStatus(liveThread) : (result.status as ThreadStatus);
    // Same `resolveVisualStatus` formula as the live rows, fed the `status`
    // snapshot above. A search hit not yet hydrated into threadMap has no
    // child/proposal info, so those default to false.
    const visualStatus = resolveVisualStatus(
        status,
        (liveThread?.meta.activeChildrenCount ?? 0) > 0,
        liveThread?.meta.codingAgentProposed ?? false,
    );
    const section = liveThread?.meta.section ?? result.section;
    const isFocused = focusedThreadId.value === result.thread_id;
    const isHighlighted = highlightedThreadId.value === result.thread_id;
    // Context name works whether or not the hit is hydrated into threadMap —
    // ThreadMeta is structurally a ThreadContextFields; the result's snake-case
    // fields map onto the same shape. The richer tooltip needs the full meta, so
    // it's only built for hydrated rows (otherwise empty → no tooltip).
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
            channel={result.channel}
            codingAgent={liveThread?.meta.codingAgent ?? result.coding_agent ?? undefined}
            visualStatus={visualStatus}
            tooltip={liveThread ? JSON.stringify(threadRowTooltip(liveThread.meta, status)) : ''}
            contextName={threadContextName(ctxFields)}
            totalChildren={liveThread?.meta.totalChildrenCount ?? 0}
            needsReview={section === 'inbox' && status !== 'running'}
            hasDraft={threadHasUnsentDraft(liveThread)}
            isFocused={isFocused}
            isHighlighted={isHighlighted}
            onClick={() => { void ensureThreadInMap(result); focusThread(result.thread_id); }}
        />
    );
}
