import { Fragment } from 'preact';
import type { ComponentChildren } from 'preact';
import { useRef, useEffect, useCallback } from 'preact/hooks';
import { memo } from 'preact/compat';
import { signal } from '@preact/signals';
import { threadDrawerOpen, threadDrawerWidth, threadMap, focusedThreadId, threadChannelFilter, selectedTriggerIds, selectedRepoIds, selectedAppIds, threadsLoaded, splitRatio, effectiveThreadStatus, getThreadDisplaySection, threadSearchQuery, threadSearchResults, threadHasMore, threadLoadingMore, draftsViewActive } from '../../store/store';
import { threadPassesChannelFilter } from '../../store/threadFilter';
import { navigateToPane } from '../../store/actions/pane';
import { focusThread } from '../../store/actions/threads';
import { loadOlderThreads, reloadAfterFilterChange, ensureThreadInMap } from '../../store/actions/thread-loading';
import { threadFilterActive } from '../../store/threadFilterActive';
import { ThreadStatusIcon, resolveVisualStatus } from '../shared/ThreadStatusIcon';
import { CopyThreadRefButton } from '../shared/CopyThreadRefButton';
import type { ThreadState, ThreadStatus } from '../../store/thread-events';
import { getDraft } from '../../store/composeDrafts';
import type { DisplaySection } from '../../generated/thread-lifecycle';

type DrawerSectionName = DisplaySection | 'new';
import { formatChannel } from '../../utils/formatChannel';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { useFlipTransitions } from '../../hooks/useFlipAnimation';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useScrollMemory } from '../../hooks/useScrollMemory';
import { isMobile } from '../../utils/viewport';
import type { ThreadSearchResult } from '../../api/threads';

// `threadPassesChannelFilter` lives in `store/threadFilter.ts` (shared with the
// infinite-scroll cursor in `thread-loading.ts`); re-exported here so existing
// importers (and tests) keep their path.
export { threadPassesChannelFilter };

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
    const isSearching = threadSearchQuery.value.trim().length > 0;
    const isDraftsMode = !isSearching && draftsViewActive.value;

    const listRef = useRef<HTMLDivElement>(null);
    // Don't restore while in an alternate view — saved offset is for the full list.
    useScrollMemory(listRef, 'lucidos-scroll-thread-drawer', { paused: isSearching || isDraftsMode });

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
    }, [isSearching, isDraftsMode]);

    return (
        <div class={`thread-drawer${visible ? '' : ' thread-drawer-collapsed'}`}
             style={visible ? { width: `${threadDrawerWidth.value}px` } : undefined}
             onKeyDown={handleKeyDown}
             tabIndex={-1}>
            <div class="thread-drawer-list" ref={listRef}>
                {visible && (
                    isSearching ? <SearchResults />
                    : isDraftsMode ? <DraftsList />
                    : <ThreadList />
                )}
            </div>
        </div>
    );
}


// Section priority for family-aware routing. Lower = higher priority. Active
// outranks review: any running descendant keeps the family under Active even
// when a sibling has a real CTA (codingAgentProposed / waiting_for_user_answer
// / failed) — the CTA still renders inline on the child row. The reverse
// (review beating active) drags a still-working family out of Active the
// moment an idle child's displaySection falls through to 'review'.

import { categorizeThreads, composingThreads, computeFamilyDecorations, computeFamilyGraph, computeFamilyKeys, computeFamilySections, depthStyle, draftThreads, filterByTopThread, hasCollapsedAncestor, nestByParent, threadHasUnsentDraft } from './family-graph';
import type { FamilyDecorations, FamilyGraph, NestedThread, ThreadSections } from './family-graph';
export * from './family-graph';
function ThreadList() {
    const containerRef = useRef<HTMLDivElement>(null);
    const portalRef = useRef<HTMLDivElement>(null);
    const sentinelRef = useRef<HTMLDivElement>(null);
    const hydrated = threadsLoaded.value;

    const filter = threadChannelFilter.value;
    // Empty channel filter means "show nothing" — including composing threads.
    // Otherwise the New section would be the only thing visible.
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
        // matching: the repo/app/trigger is a property of a specific CC thread,
        // not its (often chat/trigger) top-thread, so a CC thread in the
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
        const byFamilyRevived = (a: ThreadState, b: ThreadState) =>
            familyKeys.get(b.meta.id)!.revivedKey.localeCompare(familyKeys.get(a.meta.id)!.revivedKey);
        const byFamilyRecent = (a: ThreadState, b: ThreadState) =>
            familyKeys.get(b.meta.id)!.recentKey.localeCompare(familyKeys.get(a.meta.id)!.recentKey);
        const byFamilyReview = (a: ThreadState, b: ThreadState) => {
            const ka = familyKeys.get(a.meta.id)!;
            const kb = familyKeys.get(b.meta.id)!;
            if (ka.reviewTier !== kb.reviewTier) return ka.reviewTier - kb.reviewTier;
            return kb.recentKey.localeCompare(ka.recentKey);
        };
        categorized.review.sort(byFamilyReview);
        categorized.active.sort(byFamilyRevived);
        categorized.saved.sort(byFamilyRecent);
        categorized.archive.sort(byFamilyRecent);
    } else {
        categorized = {
            review: [], active: [], saved: [], archive: [],
            statusMap: new Map(),
            savedAttentionCount: 0,
        };
    }
    const { review, active, saved, archive, statusMap, savedAttentionCount } = categorized;

    // Drop descendants of collapsed families. The parent itself stays visible
    // (FamilyToggleRow lets the user re-expand).
    const collapsedFamiliesSet = collapsedFamilies.value;
    const filterCollapsed = (nested: NestedThread[]) =>
        nested.filter(n => !hasCollapsedAncestor(n.thread.meta.id, collapsedFamiliesSet, familyGraph));
    const sections: { name: DrawerSectionName; threads: NestedThread[] }[] = [
        { name: 'new', threads: nestByParent(composingList) },
        { name: 'review', threads: filterCollapsed(nestByParent(review)) },
        { name: 'active', threads: filterCollapsed(nestByParent(active)) },
        { name: 'saved', threads: filterCollapsed(nestByParent(saved)) },
        { name: 'archive', threads: filterCollapsed(nestByParent(archive)) },
    ];

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

    // When the channel / trigger / repo / app filter changes, re-arm pagination
    // AND eagerly fetch the first page of matching threads. A different filter
    // is a different cursor space, and the matches may be entirely outside the
    // loaded window (e.g. a repo whose threads are all archived) — relying on
    // the IntersectionObserver sentinel alone strands the user on "No threads"
    // because it's suppressed while Archive is collapsed and only re-fires when
    // threadHasMore flips. `reloadAfterFilterChange` makes population
    // deterministic. The prevRef guard suppresses the mount run (no fetch on
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
            void reloadAfterFilterChange();
        }
    }, [filter, currentTriggers, currentRepos, currentApps]);

    // Infinite scroll: observe a sentinel at the bottom of the list.
    // loadOlderThreads self-guards against concurrent calls.
    const hasMore = threadHasMore.value;

    useEffect(() => {
        const sentinel = sentinelRef.current;
        if (!sentinel || !hydrated || !hasMore) return;

        const observer = new IntersectionObserver(
            (entries) => {
                if (entries[0]?.isIntersecting && shouldLoadOlderOnIntersection(collapsedSections.value, threadFilterActive.value)) {
                    void loadOlderThreads();
                }
            },
            { root: sentinel.closest('.thread-drawer-list'), threshold: 0 },
        );
        observer.observe(sentinel);
        return () => observer.disconnect();
    }, [hydrated, hasMore]);

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
                    const attentionBadge = s.name === 'saved' ? savedAttentionCount : 0;
                    const isNew = s.name === 'new';
                    return (
                        <DrawerSection key={s.name} sectionKey={s.name} title={title} attentionBadge={attentionBadge}>
                            {s.threads.map(n => {
                                if (isNew) {
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
                                const totalChildren = n.thread.meta.totalChildrenCount;
                                const activeChildren = n.thread.meta.activeChildrenCount;
                                const isCollapsed = collapsedFamiliesSet.has(id);
                                return (
                                    <Fragment key={id}>
                                        <ThreadRow
                                            threadId={id}
                                            status={statusMap.get(id)!}
                                            depth={n.depth}
                                            isLiftedParent={isLiftedParent}
                                            isResponsibleChild={isResponsibleChild}
                                        />
                                        {totalChildren > 0 && (
                                            <FamilyToggleRow
                                                parentId={id}
                                                depth={n.depth + 1}
                                                totalChildren={totalChildren}
                                                activeChildren={activeChildren}
                                                isCollapsed={isCollapsed}
                                            />
                                        )}
                                    </Fragment>
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

// Skip pagination when Archive is collapsed: collapsing shrinks the list, pops the
// sentinel into view, and would silently bloat the count badge on every toggle.
// Archive is the bottom section and the one that absorbs paginated older threads.
//
// Exception: when a filter is active, yield to it. A trigger/repo/app filter
// whose matches are all archived can ONLY land in the Archive section, so
// blocking pagination there would strand the user on "No threads" — the whole
// point of selecting the facet. The badge-bloat case the guard protects against
// is the unfiltered view, where `filterActive` is false and the block stands.
export function shouldLoadOlderOnIntersection(collapsed: ReadonlySet<string>, filterActive: boolean): boolean {
    return filterActive || !collapsed.has('archive');
}

function DrawerSection({ sectionKey, title, children, attentionBadge = 0 }: { sectionKey: string; title: string; children: ComponentChildren; attentionBadge?: number }) {
    const collapsed = collapsedSections.value.has(sectionKey);

    const toggle = () => {
        const next = new Set(collapsedSections.value);
        if (collapsed) next.delete(sectionKey);
        else next.add(sectionKey);
        collapsedSections.value = next;
        saveStringSet(COLLAPSED_KEY, next);
    };

    return (
        <div class="drawer-section">
            <div class={`list-section-title list-section-title-collapsible${collapsed ? ' collapsed' : ''}`}
                 data-flip-id={`__section_${sectionKey}`}
                 onClick={toggle}
                 role="button"
                 aria-expanded={!collapsed}>
                {title}
                {attentionBadge > 0 && (
                    <span class="section-attention-badge" data-tooltip="Saved threads that need attention">
                        {attentionBadge}
                    </span>
                )}
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
    const modeLabel = getDraft(thread.meta.id).mode === 'claude_code' ? 'Claude Code' : 'Lucidos';

    return (
        <div data-flip-id={thread.meta.id} style={depthStyle(depth)} class={depth > 0 ? 'thread-row-wrap is-nested' : 'thread-row-wrap'}>
            <div class={classes.join(' ')}
                 data-thread-nav={thread.meta.id}
                 onClick={() => {
                     focusThread(thread.meta.id);
                     onAfterClick?.();
                     if (isMobile()) navigateToPane('thread');
                 }}>
                <ThreadStatusIcon status="idle" />
                <div class="thread-row-left">
                    <span class="thread-row-title-row">
                        <span class="thread-row-title">{threadDisplayTitle(thread)}</span>
                        <span class="draft-indicator" data-tooltip="Has unsent draft">Draft</span>
                    </span>
                </div>
                <div class="thread-row-right">
                    <span class="thread-row-time">{thread.meta.updatedAt ? formatMessageTimestamp(thread.meta.updatedAt) : ''}</span>
                    <span class="label message-channel-tag">{modeLabel}</span>
                </div>
            </div>
        </div>
    );
}

interface ThreadRowContentProps {
    id: string;
    title: string;
    channel: string;
    status: ThreadStatus;
    timestamp: string;
    exchangeCount: number;
    totalChildren: number;
    activeChildren: number;
    codingAgentProposed: boolean;
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
    onClick: () => void;
}

function ThreadRowContentImpl(props: ThreadRowContentProps) {
    const hasActiveChildren = props.activeChildren > 0;

    const classes = ['list-row', 'thread-row'];
    if (props.isFocused) classes.push('thread-row-focused');
    if (props.isHighlighted) classes.push('thread-row-highlighted');
    if (props.needsReview) classes.push('thread-row-review');
    if (props.isLiftedParent) classes.push('thread-row-lifted-parent');
    if (props.isResponsibleChild) classes.push('thread-row-lifted-child');

    return (
        <div class={classes.join(' ')}
             data-thread-nav={props.id}
             onClick={props.onClick}>
            <ThreadStatusIcon status={resolveVisualStatus(props.status, hasActiveChildren, props.codingAgentProposed)} />
            <div class="thread-row-left">
                <span class="thread-row-title-row">
                    <span class="thread-row-title">{props.title}</span>
                    {props.hasDraft && <span class="draft-indicator" data-tooltip="Has unsent draft">Draft</span>}
                </span>
                {props.exchangeCount > 1 && (
                    <span class="thread-row-meta">{props.exchangeCount} exchanges</span>
                )}
            </div>
            <div class="thread-row-right">
                <span class="thread-row-time">{props.timestamp}</span>
                <span class={`label message-channel-tag${props.channel === 'error_unknown_channel' ? ' channel-error' : ''}`}>{formatChannel(props.channel)}</span>
                <span class="thread-row-actions">
                    <CopyThreadRefButton threadId={props.id} title={props.title} stopPropagation extraClass="thread-row-action" />
                </span>
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
    && prev.title === next.title
    && prev.channel === next.channel
    && prev.status === next.status
    && prev.timestamp === next.timestamp
    && prev.exchangeCount === next.exchangeCount
    && prev.totalChildren === next.totalChildren
    && prev.activeChildren === next.activeChildren
    && prev.codingAgentProposed === next.codingAgentProposed
    && prev.needsReview === next.needsReview
    && prev.hasDraft === next.hasDraft
    && prev.isFocused === next.isFocused
    && prev.isHighlighted === next.isHighlighted
    && prev.isLiftedParent === next.isLiftedParent
    && prev.isResponsibleChild === next.isResponsibleChild
);

/** Disclosure bar rendered under a parent row. On its own row (not inline on
 *  the parent) so same-depth siblings render identically regardless of which
 *  ones happen to have grandchildren. */
function FamilyToggleRow({ parentId, depth, totalChildren, activeChildren, isCollapsed }: {
    parentId: string;
    depth: number;
    totalChildren: number;
    activeChildren: number;
    isCollapsed: boolean;
}) {
    const doneCount = totalChildren - activeChildren;
    // Visible label reads as a fraction ("1/1 sub-threads done"); aria reads as
    // a bare count, so it stays smart-plural ("Show 1 sub-thread").
    const progressLabel = `${doneCount}/${totalChildren} sub-threads done`;
    const a11yCount = `${totalChildren} sub-thread${totalChildren === 1 ? '' : 's'}`;
    return (
        <div class="thread-row-wrap is-nested family-toggle-wrap" style={depthStyle(depth)}>
            <button
                type="button"
                class={`family-toggle${isCollapsed ? ' is-collapsed' : ''}`}
                onClick={() => toggleFamilyCollapse(parentId)}
                aria-label={isCollapsed ? `Show ${a11yCount}` : `Hide ${a11yCount}`}
                aria-expanded={!isCollapsed}>
                <span class="family-toggle-glyph" aria-hidden="true">{isCollapsed ? '▸' : '▾'}</span>
                <span class="family-toggle-progress">{progressLabel}</span>
            </button>
        </div>
    );
}

// Reading composeDrafts here would fan a re-render to every visible ThreadRow
// per keystroke — the lag this signal was added to prevent.

export function ThreadRow({ threadId, status, depth = 0, isLiftedParent, isResponsibleChild, onAfterClick }: {
    threadId: string;
    status: ThreadStatus;
    depth?: number;
    isLiftedParent?: boolean;
    isResponsibleChild?: boolean;
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
    const isFocused = focusedThreadId.value === meta.id;
    const isHighlighted = highlightedThreadId.value === meta.id;
    const hasDraft = threadHasUnsentDraft(thread);

    const wrapClasses = ['thread-row-wrap'];
    if (depth > 0) wrapClasses.push('is-nested');

    return (
        <div data-flip-id={meta.id} style={depthStyle(depth)} class={wrapClasses.join(' ')}>
            <ThreadRowContent
                id={meta.id}
                title={threadDisplayTitle(thread)}
                channel={meta.channel}
                status={status}
                timestamp={formatMessageTimestamp(meta.updatedAt)}
                exchangeCount={meta.messageCount}
                totalChildren={meta.totalChildrenCount}
                activeChildren={meta.activeChildrenCount}
                codingAgentProposed={meta.codingAgentProposed}
                needsReview={meta.section === 'inbox' && status !== 'running'}
                hasDraft={hasDraft}
                isFocused={isFocused}
                isHighlighted={isHighlighted}
                isLiftedParent={isLiftedParent}
                isResponsibleChild={isResponsibleChild}
                onClick={() => { focusThread(meta.id); onAfterClick?.(); }}
            />
        </div>
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
    const section = liveThread?.meta.section ?? result.section;
    const isFocused = focusedThreadId.value === result.thread_id;
    const isHighlighted = highlightedThreadId.value === result.thread_id;

    return (
        <ThreadRowContent
            id={result.thread_id}
            title={result.title}
            channel={result.channel}
            status={status}
            timestamp={formatMessageTimestamp(result.last_activity)}
            exchangeCount={result.message_count}
            totalChildren={liveThread?.meta.totalChildrenCount ?? 0}
            activeChildren={liveThread?.meta.activeChildrenCount ?? 0}
            codingAgentProposed={liveThread?.meta.codingAgentProposed ?? false}
            needsReview={section === 'inbox' && status !== 'running'}
            hasDraft={threadHasUnsentDraft(liveThread)}
            isFocused={isFocused}
            isHighlighted={isHighlighted}
            onClick={() => { void ensureThreadInMap(result); focusThread(result.thread_id); }}
        />
    );
}
