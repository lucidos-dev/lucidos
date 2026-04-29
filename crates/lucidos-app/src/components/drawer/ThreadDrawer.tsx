import type { ComponentChildren } from 'preact';
import { useRef, useEffect, useCallback } from 'preact/hooks';
import { signal } from '@preact/signals';
import { threadDrawerOpen, threadDrawerWidth, threadMap, focusedThreadId, focusedDraftId, threadChannelFilter, excludedTriggerIds, threadsLoaded, splitRatio, ThreadChannel, ALL_CHANNELS, effectiveThreadStatus, threadSearchQuery, threadSearchResults, threadHasMore, threadLoadingMore, drafts, type DraftMeta } from '../../store/store';
import { navigateToPane } from '../../store/actions/pane';
import { focusThread } from '../../store/actions/threads';
import { focusDraft } from '../../store/actions/drafts';
import { loadOlderThreads, ensureThreadInMap } from '../../store/actions/thread-loading';
import { ThreadStatusIcon, resolveVisualStatus } from '../shared/ThreadStatusIcon';
import { CloseIcon } from '../shared/icons';
import { PinButton } from '../shared/PinButton';
import { byRecent, byReviewOrder } from '../../store/thread-events';
import type { ThreadState, ThreadStatus } from '../../store/thread-events';
import { displaySection } from '../../generated/thread-lifecycle';
import type { StoredSection } from '../../generated/thread-lifecycle';
import { formatChannel } from '../../utils/formatChannel';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { useFlipTransitions } from '../../hooks/useFlipAnimation';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { isMobile } from '../../utils/viewport';
import type { ThreadSearchResult } from '../../api/threads';

const VALID_CHANNELS: ReadonlySet<string> = new Set<string>(ALL_CHANNELS);

/** Currently keyboard-highlighted thread ID in the drawer. */
const highlightedThreadId = signal<string | null>(null);

/** Ordered list of navigable thread IDs, set by ThreadList or SearchResults. */
const navigableIds = signal<string[]>([]);

const DRAFT_NAV_PREFIX = '__draft_';

export function selectHighlighted() {
    const id = highlightedThreadId.value;
    if (!id) return;
    if (id.startsWith(DRAFT_NAV_PREFIX)) {
        focusDraft(id.slice(DRAFT_NAV_PREFIX.length));
        return;
    }
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

    // Reset highlight when switching between search and list mode
    useEffect(() => {
        highlightedThreadId.value = null;
    }, [isSearching]);

    return (
        <>
            <div class={`thread-drawer${visible ? '' : ' thread-drawer-collapsed'}`}
                 style={visible ? { width: `${threadDrawerWidth.value}px` } : undefined}
                 onKeyDown={handleKeyDown}
                 tabIndex={-1}>
                <div class="thread-drawer-header">
                    <span class="thread-drawer-header-title">Threads</span>
                    <div class="thread-drawer-header-actions">
                        <button class="icon-btn header-icon thread-drawer-close-btn" onClick={() => { threadDrawerOpen.value = false; }} aria-label="Close thread drawer" data-tooltip="Close">
                            <CloseIcon />
                        </button>
                    </div>
                </div>
                <div class="thread-drawer-list">

                    {visible && (isSearching ? <SearchResults /> : <ThreadList />)}
                </div>
            </div>
        </>
    );
}

export type ThreadSections = {
    drafts: ThreadState[];
    review: ThreadState[];
    running: ThreadState[];
    waiting: ThreadState[];
    pinned: ThreadState[];
    history: ThreadState[];
    statusMap: Map<string, ThreadStatus>;
};

/** Group threads into drawer sections. Threads with unsent drafts go to the
 *  Drafts section ONLY (never duplicated into review/history/etc.) — except
 *  when the draft thread is currently focused on desktop, in which case it
 *  appears in its natural section so the user can still see live updates. */
export function categorizeThreads(
    threads: ThreadState[],
    draftMap: ReadonlyMap<string, DraftMeta>,
    focused: string | null,
    mobile: boolean,
): ThreadSections {
    const out: ThreadSections = {
        drafts: [], review: [], running: [], waiting: [], pinned: [], history: [],
        statusMap: new Map(),
    };
    for (const t of threads) {
        const status = effectiveThreadStatus(t);
        out.statusMap.set(t.meta.id, status);
        if (draftMap.has(t.meta.id) && (mobile || t.meta.id !== focused)) {
            out.drafts.push(t);
            continue;
        }
        const display = displaySection(
            t.meta.section as StoredSection,
            status,
            t.meta.pinned,
            t.meta.activeChildrenCount > 0,
        );
        switch (display) {
            case 'running': out.running.push(t); break;
            case 'waiting': out.waiting.push(t); break;
            case 'review': out.review.push(t); break;
            case 'pinned': out.pinned.push(t); break;
            case 'history': out.history.push(t); break;
        }
    }
    return out;
}

interface ComposeDraftEntry {
    id: string;
    meta: DraftMeta;
}

/** Compose drafts visible as separate rows in the Drafts section. Excludes
 *  thread-attached drafts (rendered via ThreadRow) and the focused compose
 *  draft on desktop (already visible in the textarea). */
function visibleComposeDraftEntries(
    draftMap: ReadonlyMap<string, DraftMeta>,
    threads: ReadonlyMap<string, unknown>,
    focusedThread: string | null,
    activeDraft: string,
    mobile: boolean,
): ComposeDraftEntry[] {
    const entries: ComposeDraftEntry[] = [];
    for (const [id, meta] of draftMap) {
        if (threads.has(id)) continue;
        if (focusedThread === null && id === activeDraft && !mobile) continue;
        entries.push({ id, meta });
    }
    entries.sort((a, b) => b.meta.updatedAt.localeCompare(a.meta.updatedAt));
    return entries;
}

function ThreadList() {
    const containerRef = useRef<HTMLDivElement>(null);
    const portalRef = useRef<HTMLDivElement>(null);
    const sentinelRef = useRef<HTMLDivElement>(null);
    const hydrated = threadsLoaded.value;

    const focused = focusedThreadId.value;
    const mobile = isMobile();
    const draftMap = drafts.value;
    const composeDraftEntries = visibleComposeDraftEntries(
        draftMap,
        threadMap.value,
        focused,
        focusedDraftId.value,
        mobile,
    );

    let categorized: ThreadSections;
    if (hydrated) {
        const filter = threadChannelFilter.value;
        const excludedTriggers = excludedTriggerIds.value;
        const allThreads = Array.from(threadMap.value.values()).filter(t => {
            const channel = t.meta.channel;
            if (!filter.has(channel as ThreadChannel) && VALID_CHANNELS.has(channel)) return false;
            // Trigger threads without a known triggerId have no checkbox; hiding them would orphan them.
            if (channel === 'trigger' && t.meta.triggerId && excludedTriggers.has(t.meta.triggerId)) {
                return false;
            }
            return true;
        });
        categorized = categorizeThreads(allThreads, draftMap, focused, mobile);

        const byRevived = (a: ThreadState, b: ThreadState) =>
            (b.meta.lastRevivedAt || b.meta.createdAt).localeCompare(a.meta.lastRevivedAt || a.meta.createdAt);
        categorized.review.sort(byReviewOrder);
        categorized.drafts.sort(byRecent);
        categorized.running.sort(byRevived);
        categorized.waiting.sort(byRevived);
        categorized.pinned.sort(byRecent);
        categorized.history.sort(byRecent);
    } else {
        categorized = {
            drafts: [], review: [], running: [], waiting: [], pinned: [], history: [],
            statusMap: new Map(),
        };
    }
    const { drafts: draftThreads, review, running, waiting, pinned, history, statusMap } = categorized;

    // Single source of truth for section order
    const sections = [
        { name: 'drafts', threads: draftThreads },
        { name: 'review', threads: review },
        { name: 'running', threads: running },
        { name: 'waiting', threads: waiting },
        { name: 'pinned', threads: pinned },
        { name: 'history', threads: history },
    ];

    const sectionDefs = sections
        .filter(s => s.threads.length > 0 || (s.name === 'drafts' && composeDraftEntries.length > 0))
        .map(s => ({
            name: s.name,
            ids: [
                `__section_${s.name}`,
                ...(s.name === 'drafts' ? composeDraftEntries.map(e => `${DRAFT_NAV_PREFIX}${e.id}`) : []),
                ...s.threads.map(t => t.meta.id),
            ],
        }));

    // Build flat navigable ID list from visible (non-collapsed) sections
    const collapsed = collapsedSections.value;
    const flatIds: string[] = [];
    for (const s of sections) {
        if (collapsed.has(s.name)) continue;
        if (s.name === 'drafts') {
            for (const e of composeDraftEntries) flatIds.push(`${DRAFT_NAV_PREFIX}${e.id}`);
        }
        flatIds.push(...s.threads.map(t => t.meta.id));
    }
    const flatKey = flatIds.join(',');
    useEffect(() => { navigableIds.value = flatIds; }, [flatKey]);

    // String resetKey lets the FLIP hook compare by value (Sets would compare by reference).
    const filterResetKey = `${[...threadChannelFilter.value].sort().join(',')}|${[...excludedTriggerIds.value].sort().join(',')}`;
    useFlipTransitions(containerRef, portalRef, sectionDefs, filterResetKey);

    // Scroll focused thread into view (e.g. when opened via thread link)
    useEffect(() => {
        if (!focused) return;
        const id = requestAnimationFrame(() => {
            containerRef.current?.querySelector(`[data-thread-nav="${focused}"]`)
                ?.scrollIntoView({ block: 'nearest' });
        });
        return () => cancelAnimationFrame(id);
    }, [focused]);

    // Reset pagination when filter changes — different filter = different cursor space.
    const currentFilter = threadChannelFilter.value;
    const prevFilterRef = useRef(currentFilter);
    useEffect(() => {
        if (prevFilterRef.current !== currentFilter) {
            prevFilterRef.current = currentFilter;
            threadHasMore.value = true;
        }
    }, [currentFilter]);

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
                    const isDrafts = s.name === 'drafts';
                    const visible = isDrafts
                        ? (s.threads.length > 0 || composeDraftEntries.length > 0)
                        : s.threads.length > 0;
                    if (!visible) return null;
                    const title = s.name.charAt(0).toUpperCase() + s.name.slice(1);
                    const count = isDrafts ? s.threads.length + composeDraftEntries.length : s.threads.length;
                    return (
                        <DrawerSection key={s.name} title={title} count={count}>
                            {isDrafts && composeDraftEntries.map(e => (
                                <ComposeDraftRow key={e.id} draftId={e.id} meta={e.meta} />
                            ))}
                            {s.threads.map(t => <ThreadRow key={t.meta.id} threadId={t.meta.id} status={statusMap.get(t.meta.id)!} />)}
                        </DrawerSection>
                    );
                })}
                {sections.every(s => s.threads.length === 0) && composeDraftEntries.length === 0 && (
                    <div class="empty">No threads</div>
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

function DrawerSection({ title, children, count }: { title: string; children: ComponentChildren; count: number }) {
    const key = title.toLowerCase();
    const collapsed = collapsedSections.value.has(key);

    const toggle = () => {
        const next = new Set(collapsedSections.value);
        collapsed ? next.delete(key) : next.add(key);
        collapsedSections.value = next;
        saveCollapsed(next);
    };

    return (
        <div class="drawer-section">
            <div class={`list-section-title list-section-title-collapsible${collapsed ? ' collapsed' : ''}`}
                 data-flip-id={`__section_${key}`}
                 onClick={toggle}
                 role="button"
                 aria-expanded={!collapsed}>
                {title}
                <span class="section-count">{count}</span>
            </div>
            {!collapsed && children}
        </div>
    );
}

function ComposeDraftRow({ draftId, meta }: { draftId: string; meta: DraftMeta }) {
    const navId = `${DRAFT_NAV_PREFIX}${draftId}`;
    const isFocused = focusedThreadId.value === null && focusedDraftId.value === draftId;
    const isHighlighted = highlightedThreadId.value === navId;
    const classes = ['list-row', 'thread-row', 'compose-draft-row'];
    if (isFocused) classes.push('thread-row-focused');
    if (isHighlighted) classes.push('thread-row-highlighted');

    return (
        <div data-flip-id={navId}>
            <div class={classes.join(' ')}
                 data-thread-nav={navId}
                 data-draft-id={draftId}
                 onClick={() => {
                     focusDraft(draftId);
                     if (isMobile()) navigateToPane('thread');
                 }}>
                <ThreadStatusIcon status="idle" />
                <div class="thread-row-left">
                    <span class="thread-row-title-row">
                        <span class="thread-row-title">{meta.title}</span>
                        <span class="draft-indicator" data-tooltip="Has unsent draft">Draft</span>
                    </span>
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
    pinned: boolean;
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
                    <span class="thread-row-title">{data.title || 'Untitled Thread'}</span>
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
                <span class="thread-row-tag-line">
                    <span class={`label message-channel-tag${data.channel === 'error_unknown_channel' ? ' channel-error' : ''}`}>{formatChannel(data.channel)}</span>
                    <PinButton threadId={data.id} pinned={data.pinned} stopPropagation />
                </span>
            </div>
        </div>
    );
}

function ThreadRow({ threadId, status }: { threadId: string; status: ThreadStatus }) {
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
                title: meta.title,
                channel: meta.channel,
                status,
                timestamp: formatMessageTimestamp(meta.updatedAt),
                exchangeCount,
                totalChildren: meta.totalChildrenCount,
                activeChildren: meta.activeChildrenCount,
                ccHasChanges: meta.ccHasChanges,
                pinned: meta.pinned,
                needsReview: meta.section === 'unread' && status !== 'running',
                hasDraft: drafts.value.has(meta.id),
                onClick: () => focusThread(meta.id),
            }} />
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
        return <div class="empty" style={{ color: 'var(--error)' }}>Search failed</div>;
    }
    if (loadable.status !== 'loaded') {
        if (!showLoading) return null;
        return <div class="loading-spinner" />;
    }

    if (loadable.data.length === 0) {
        return <div class="empty">No threads found</div>;
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
            pinned: liveThread?.meta.pinned ?? false,
            needsReview: section === 'unread' && status !== 'running',
            hasDraft: drafts.value.has(result.thread_id),
            onClick: () => { ensureThreadInMap(result); focusThread(result.thread_id); },
        }} />
    );
}
