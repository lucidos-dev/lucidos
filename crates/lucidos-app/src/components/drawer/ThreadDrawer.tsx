import type { ComponentChildren } from 'preact';
import { useRef, useEffect, useCallback } from 'preact/hooks';
import { signal } from '@preact/signals';
import { threadDrawerOpen, threadDrawerWidth, threadMap, focusedThreadId, threadChannelFilter, selectedTriggerIds, selectedRepoIds, threadsLoaded, splitRatio, ThreadChannel, ALL_CHANNELS, effectiveThreadStatus, threadSearchQuery, threadSearchResults, threadHasMore, threadLoadingMore, draftsViewActive } from '../../store/store';
import { navigateToPane } from '../../store/actions/pane';
import { focusThread } from '../../store/actions/threads';
import { loadOlderThreads, ensureThreadInMap } from '../../store/actions/thread-loading';
import { ThreadStatusIcon, resolveVisualStatus } from '../shared/ThreadStatusIcon';
import { CopyThreadRefButton } from '../shared/CopyThreadRefButton';
import { byRecent, byReviewOrder } from '../../store/thread-events';
import type { ThreadState, ThreadStatus } from '../../store/thread-events';
import { draftIsEmpty, getDraft } from '../../store/composeDrafts';
import { displaySection } from '../../generated/thread-lifecycle';
import type { ArchiveState, DisplaySection } from '../../generated/thread-lifecycle';

type DrawerSectionName = DisplaySection | 'new';
import { formatChannel } from '../../utils/formatChannel';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { useFlipTransitions } from '../../hooks/useFlipAnimation';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useScrollMemory } from '../../hooks/useScrollMemory';
import { isMobile } from '../../utils/viewport';
import type { ThreadSearchResult } from '../../api/threads';

const VALID_CHANNELS: ReadonlySet<string> = new Set<string>(ALL_CHANNELS);

/** Currently keyboard-highlighted thread ID in the drawer. */
const highlightedThreadId = signal<string | null>(null);

/** Ordered list of navigable thread IDs, set by ThreadList or SearchResults. */
export const navigableIds = signal<string[]>([]);

export function selectHighlighted() {
    const id = highlightedThreadId.value;
    if (!id) return;
    const searchResult = threadSearchResults.value;
    if (threadSearchQuery.value.trim().length > 0 && searchResult.status === 'loaded') {
        const match = searchResult.data.find((r: ThreadSearchResult) => r.thread_id === id);
        if (match) ensureThreadInMap(match);
    }
    focusThread(id);
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

export type ThreadSections = {
    review: ThreadState[];
    active: ThreadState[];
    saved: ThreadState[];
    archive: ThreadState[];
    statusMap: Map<string, ThreadStatus>;
    /** Saved threads that would be in 'review' if not saved. Powers the
     *  saved-section badge — surfaces what saved-overrides-routing hides. */
    savedReviewCount: number;
};

/** Group threads into drawer sections. Section membership is purely a function
 *  of thread state — see displaySection() in the lifecycle contract. Composing
 *  and discarded threads are excluded from the four lifecycle sections; the
 *  caller surfaces composing threads in the New section instead. */
export function categorizeThreads(threads: ThreadState[]): ThreadSections {
    const out: ThreadSections = {
        review: [], active: [], saved: [], archive: [],
        statusMap: new Map(),
        savedReviewCount: 0,
    };
    for (const t of threads) {
        if (t.meta.state === 'composing' || t.meta.state === 'discarded') continue;
        const status = effectiveThreadStatus(t);
        out.statusMap.set(t.meta.id, status);
        const stored = t.meta.section as ArchiveState;
        const hasActiveChildren = t.meta.activeChildrenCount > 0;
        const hasPendingChanges = t.meta.ccHasChanges;
        const display = displaySection(stored, status, t.meta.saved, hasActiveChildren, hasPendingChanges);
        switch (display) {
            case 'active': out.active.push(t); break;
            case 'review': out.review.push(t); break;
            case 'saved': out.saved.push(t); break;
            case 'archive': out.archive.push(t); break;
        }
        if (display === 'saved'
            && displaySection(stored, status, false, hasActiveChildren, hasPendingChanges) === 'review') {
            out.savedReviewCount++;
        }
    }
    return out;
}

/** New section rows. Empty composing rows are filtered out — POST/DELETE
 *  races, SSE skeletons from a peer's ThreadStarted with no follow-up
 *  compose change, and failed local discards leave server-side rows whose
 *  only UI surface would be a placeholder "Empty draft" title. */
export function composingThreads(threads: ReadonlyMap<string, ThreadState>): ThreadState[] {
    const out: ThreadState[] = [];
    for (const t of threads.values()) {
        if (t.meta.state === 'composing' && threadHasUnsentDraft(t)) out.push(t);
    }
    out.sort((a, b) => b.meta.updatedAt.localeCompare(a.meta.updatedAt));
    return out;
}

/** Drafts view rows: composing threads with content + active threads with
 *  follow-up content. Discarded skipped (stale compose fields lingering on
 *  tombstoned rows must not resurface). Composing (new) ahead of follow-ups,
 *  most recent first within each group. */
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
    if (hydrated) {
        const triggerSelection = selectedTriggerIds.value;
        const repoSelection = selectedRepoIds.value;
        const allThreads = Array.from(threadMap.value.values()).filter(t => {
            const ch = t.meta.channel;
            const channelOk = filter.has(ch as ThreadChannel) || !VALID_CHANNELS.has(ch);
            if (!channelOk) return false;
            if (ch === 'trigger' && triggerSelection.size > 0) {
                return t.meta.triggerId != null && triggerSelection.has(t.meta.triggerId);
            }
            if (ch === 'claude_code' && repoSelection.size > 0) {
                return t.meta.repoId != null && repoSelection.has(t.meta.repoId);
            }
            return true;
        });
        categorized = categorizeThreads(allThreads);

        const byRevived = (a: ThreadState, b: ThreadState) =>
            (b.meta.lastRevivedAt || b.meta.createdAt).localeCompare(a.meta.lastRevivedAt || a.meta.createdAt);
        categorized.review.sort(byReviewOrder);
        categorized.active.sort(byRevived);
        categorized.saved.sort(byRecent);
        categorized.archive.sort(byRecent);
    } else {
        categorized = {
            review: [], active: [], saved: [], archive: [],
            statusMap: new Map(),
            savedReviewCount: 0,
        };
    }
    const { review, active, saved, archive, statusMap, savedReviewCount } = categorized;

    const sections: { name: DrawerSectionName; threads: ThreadState[] }[] = [
        { name: 'new', threads: composingList },
        { name: 'review', threads: review },
        { name: 'active', threads: active },
        { name: 'saved', threads: saved },
        { name: 'archive', threads: archive },
    ];

    const sectionDefs = sections
        .filter(s => s.threads.length > 0)
        .map(s => ({
            name: s.name,
            ids: [
                `__section_${s.name}`,
                ...s.threads.map(t => t.meta.id),
            ],
        }));

    // Build flat navigable ID list from visible (non-collapsed) sections
    const collapsed = collapsedSections.value;
    const flatIds: string[] = [];
    for (const s of sections) {
        if (collapsed.has(s.name)) continue;
        flatIds.push(...s.threads.map(t => t.meta.id));
    }
    const flatKey = flatIds.join(',');
    useEffect(() => { navigableIds.value = flatIds; }, [flatKey]);

    useFlipTransitions(containerRef, portalRef, sectionDefs, filter);

    // Reset pagination when channel, trigger, or repo filter changes —
    // different filter = different cursor space.
    const currentTriggers = selectedTriggerIds.value;
    const currentRepos = selectedRepoIds.value;
    const prevFilterRef = useRef(filter);
    const prevTriggersRef = useRef(currentTriggers);
    const prevReposRef = useRef(currentRepos);
    useEffect(() => {
        if (prevFilterRef.current !== filter
            || prevTriggersRef.current !== currentTriggers
            || prevReposRef.current !== currentRepos) {
            prevFilterRef.current = filter;
            prevTriggersRef.current = currentTriggers;
            prevReposRef.current = currentRepos;
            threadHasMore.value = true;
        }
    }, [filter, currentTriggers, currentRepos]);

    // Infinite scroll: observe a sentinel at the bottom of the list.
    // loadOlderThreads self-guards against concurrent calls.
    const hasMore = threadHasMore.value;

    useEffect(() => {
        const sentinel = sentinelRef.current;
        if (!sentinel || !hydrated || !hasMore) return;

        const observer = new IntersectionObserver(
            (entries) => {
                if (entries[0]?.isIntersecting) {
                    loadOlderThreads();
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
                    const reviewBadge = s.name === 'saved' ? savedReviewCount : 0;
                    const isNew = s.name === 'new';
                    return (
                        <DrawerSection key={s.name} sectionKey={s.name} title={title} reviewBadge={reviewBadge}>
                            {s.threads.map(t => isNew
                                ? <ComposingThreadRow key={t.meta.id} thread={t} />
                                : <ThreadRow key={t.meta.id} threadId={t.meta.id} status={statusMap.get(t.meta.id)!} />)}
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

function loadCollapsed(): Set<string> {
    try {
        const raw = localStorage.getItem(COLLAPSED_KEY);
        return raw ? new Set(JSON.parse(raw)) : new Set();
    } catch { return new Set(); }
}

function saveCollapsed(set: Set<string>) {
    localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...set]));
}

/** Shared collapsed state so ThreadList can read it for navigableIds. */
const collapsedSections = signal(loadCollapsed());

function DrawerSection({ sectionKey, title, children, reviewBadge = 0 }: { sectionKey: string; title: string; children: ComponentChildren; reviewBadge?: number }) {
    const collapsed = collapsedSections.value.has(sectionKey);

    const toggle = () => {
        const next = new Set(collapsedSections.value);
        if (collapsed) next.delete(sectionKey);
        else next.add(sectionKey);
        collapsedSections.value = next;
        saveCollapsed(next);
    };

    return (
        <div class="drawer-section">
            <div class={`list-section-title list-section-title-collapsible${collapsed ? ' collapsed' : ''}`}
                 data-flip-id={`__section_${sectionKey}`}
                 onClick={toggle}
                 role="button"
                 aria-expanded={!collapsed}>
                {title}
                {reviewBadge > 0 && (
                    <span class="section-review-badge" data-tooltip="Saved threads in review">
                        {reviewBadge}
                    </span>
                )}
            </div>
            {!collapsed && children}
        </div>
    );
}

export function ComposingThreadRow({ thread, onAfterClick }: { thread: ThreadState; onAfterClick?: () => void }) {
    const isFocused = focusedThreadId.value === thread.meta.id;
    const isHighlighted = highlightedThreadId.value === thread.meta.id;
    const classes = ['list-row', 'thread-row', 'compose-draft-row'];
    if (isFocused) classes.push('thread-row-focused');
    if (isHighlighted) classes.push('thread-row-highlighted');
    const modeLabel = getDraft(thread.meta.id).mode === 'claude_code' ? 'Claude Code' : 'Lucidos';

    return (
        <div data-flip-id={thread.meta.id}>
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

interface ThreadRowData {
    id: string;
    title: string;
    channel: string;
    status: ThreadStatus;
    timestamp: string;
    exchangeCount: number;
    totalChildren: number;
    activeChildren: number;
    ccHasChanges: boolean;
    needsReview: boolean;
    hasDraft: boolean;
    onClick: () => void;
}

function ThreadRowContent({ data }: { data: ThreadRowData }) {
    const hasChildren = data.totalChildren > 0;
    const hasActiveChildren = data.activeChildren > 0;
    const doneCount = data.totalChildren - data.activeChildren;

    const classes = ['list-row', 'thread-row'];
    if (focusedThreadId.value === data.id) classes.push('thread-row-focused');
    if (highlightedThreadId.value === data.id) classes.push('thread-row-highlighted');
    if (data.needsReview) classes.push('thread-row-review');

    return (
        <div class={classes.join(' ')}
             data-thread-nav={data.id}
             onClick={data.onClick}>
            <ThreadStatusIcon status={resolveVisualStatus(data.status, hasActiveChildren, data.ccHasChanges)} />
            <div class="thread-row-left">
                <span class="thread-row-title-row">
                    <span class="thread-row-title">{data.title}</span>
                    {data.hasDraft && <span class="draft-indicator" data-tooltip="Has unsent draft">Draft</span>}
                </span>
                {(data.exchangeCount > 1 || hasChildren) && (
                    <span class={`thread-row-meta${hasChildren ? ' thread-row-children-progress' : ''}`}>
                        {data.exchangeCount > 1 && `${data.exchangeCount} exchanges`}
                        {data.exchangeCount > 1 && hasChildren && ' · '}
                        {hasChildren && `${doneCount}/${data.totalChildren} done`}
                    </span>
                )}
            </div>
            <div class="thread-row-right">
                <span class="thread-row-time">{data.timestamp}</span>
                <span class={`label message-channel-tag${data.channel === 'error_unknown_channel' ? ' channel-error' : ''}`}>{formatChannel(data.channel)}</span>
                <span class="thread-row-actions">
                    <CopyThreadRefButton threadId={data.id} title={data.title} stopPropagation extraClass="thread-row-action" />
                </span>
            </div>
        </div>
    );
}

function threadHasUnsentDraft(thread: ThreadState | undefined): boolean {
    if (!thread) return false;
    return !draftIsEmpty(getDraft(thread.meta.id));
}

export function ThreadRow({ threadId, status, onAfterClick }: { threadId: string; status: ThreadStatus; onAfterClick?: () => void }) {
    const thread = threadMap.value.get(threadId);
    if (!thread) return null;
    const { meta } = thread;
    // Use API-provided message_count until events are loaded, then count from events
    const exchangeCount = thread.eventsLoaded
        ? [...thread.events.values()].filter(e => e.type === 'MessageReceived' || e.type === 'TriggerStarted').length
        : meta.messageCount;

    return (
        <div data-flip-id={meta.id}>
            <ThreadRowContent data={{
                id: meta.id,
                title: threadDisplayTitle(thread),
                channel: meta.channel,
                status,
                timestamp: formatMessageTimestamp(meta.updatedAt),
                exchangeCount,
                totalChildren: meta.totalChildrenCount,
                activeChildren: meta.activeChildrenCount,
                ccHasChanges: meta.ccHasChanges,
                needsReview: meta.section === 'inbox' && status !== 'running',
                hasDraft: threadHasUnsentDraft(thread),
                onClick: () => { focusThread(meta.id); onAfterClick?.(); },
            }} />
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

    return (
        <ThreadRowContent data={{
            id: result.thread_id,
            title: result.title,
            channel: result.channel,
            status,
            timestamp: formatMessageTimestamp(result.last_activity),
            exchangeCount: result.message_count,
            totalChildren: liveThread?.meta.totalChildrenCount ?? 0,
            activeChildren: liveThread?.meta.activeChildrenCount ?? 0,
            ccHasChanges: liveThread?.meta.ccHasChanges ?? false,
            needsReview: section === 'inbox' && status !== 'running',
            hasDraft: threadHasUnsentDraft(liveThread),
            onClick: () => { ensureThreadInMap(result); focusThread(result.thread_id); },
        }} />
    );
}
