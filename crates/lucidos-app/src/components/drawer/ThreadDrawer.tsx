import type { ComponentChildren } from 'preact';
import { useRef, useEffect, useLayoutEffect, useCallback, useMemo } from 'preact/hooks';
import { memo } from 'preact/compat';
import { signal, useSignalEffect } from '@preact/signals';
import { threadDrawerOpen, threadDrawerWidth, threadMap, focusedThreadId, threadsLoaded, splitRatio, effectiveThreadStatus, getThreadDisplaySection, threadSearchQuery, threadSearchResults, threadHasMore, threadLoadingMore, archiveThreadCount, drawerView, setDrawerView, repositories, focusedPane, scaledDurationMs } from '../../store/store';
import { appliedThreadFilter } from '../../store/appliedThreadFilter';
import { resolveScope, resolveCodingAgent } from '../../store/composeSelections';
import { composeDraftContextName } from '../../store/composeDestination';
import { threadPassesChannelFilter } from '../../store/threadFilter';
import { threadFilterPanelOpen, closeThreadFilterPanel } from '../../store/threadFilterPanel';
import { ThreadFilterPanel } from '../layout/ThreadFilterPanel';
import { focusPane } from '../../store/actions/pane';
import { focusThread } from '../../store/actions/threads';
import { loadOlderThreads, reloadAfterFilterChange, filterChangedSinceLoad, ensureThreadInMap, loadThreadEvents } from '../../store/actions/thread-loading';
import { ThreadStatusIcon, resolveVisualStatus, type VisualStatus } from '../shared/ThreadStatusIcon';
import { PinThreadButton } from '../shared/PinThreadButton';
import { ThreadOverflowMenu } from '../shared/ThreadOverflowMenu';
import { DraftOverflowMenu } from '../shared/DraftOverflowMenu';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import type { ThreadState, ThreadStatus } from '../../store/thread-events';
import { getDraft } from '../../store/composeDrafts';
import type { DisplaySection } from '../../generated/thread-lifecycle';
import { formatThreadChannelLabel } from '../../utils/formatChannel';
import { threadContextName, type ThreadContextFields } from './threadRowInfo';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { useFlipTransitions } from '../../hooks/useFlipAnimation';
import { useDelayedLoading, useLingeringFlag } from '../../hooks/useDelayedLoading';
import { PANE_TRANSITION_MS } from '../layout/splitHelpers';
import { useScrollMemory } from '../../hooks/useScrollMemory';
import { getRemPx } from '../../utils/dom';
import type { ThreadSearchResult } from '../../api/threads';
import { PinIcon, InboxIcon, ArchiveIcon, DraftsIcon, AttentionIcon, RunningIcon } from '../shared/icons';
import type { ComponentType } from 'preact';

// `threadPassesChannelFilter` lives in `store/threadFilter.ts` (shared with the
// infinite-scroll cursor in `thread-loading.ts`); re-exported here so existing
// importers (and tests) keep their path.
export { threadPassesChannelFilter };

export const THREAD_DRAWER_SECTION_ORDER: readonly DisplaySection[] = ['saved', 'current', 'archive'];

function formatCreatedTimestamp(createdAt: string | undefined): string {
    if (!createdAt) return '';
    const date = new Date(createdAt);
    if (Number.isNaN(date.getTime())) return '';
    return formatMessageTimestamp(createdAt);
}

// Per-section header display: label + icon. The `'saved'` section reads "Pinned"
// in the UI — a label-only override. Every USER-FACING surface now says
// "Pinned" / "Pin" / "Unpin" (this header, the chat pin toggle, the unpin
// confirm dialog, tooltips, aria-labels, the disk-usage badge); only the
// INTERNAL identifiers still say "saved" (the section key, `is_saved`, the
// `ThreadSaved`/`ThreadUnsaved` events, the `/threads/save`+`/unsave` routes) —
// a full-stack rename of those is a later decision. The other two sections
// derive their label from the section key.
const SECTION_META: Record<DisplaySection, { title: string; Icon: ComponentType<{ size?: string }> }> = {
    saved: { title: 'Pinned', Icon: PinIcon },
    current: { title: 'Current', Icon: InboxIcon },
    archive: { title: 'Archive', Icon: ArchiveIcon },
};

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
 *  drawer here would move the focused pane to the drawer and then straight back
 *  to the thread on the same click. */
export function handleDrawerPointerDown(target: EventTarget | null): void {
    if (isThreadRowTarget(target)) return;
    focusPane('drawer');
}

/** A keyboard-navigable node in the drawer's list: either a collapsible section
 *  header or a (possibly nested) thread row. The ↑/↓ highlight walks these in
 *  render order; ←/→ collapse/expand them tree-style (see `leftAction` /
 *  `rightAction`). */
export type DrawerNavNode =
    | { kind: 'section'; sectionKey: DisplaySection }
    | {
          kind: 'thread';
          id: string;
          /** Nesting depth — 0 for top-level rows, ≥1 for sub-threads. */
          depth: number;
          /** The visible parent thread id, set only when this row is nested
           *  under a parent that's also rendered (depth > 0); null for top-level
           *  rows and orphans whose parent is paginated/filtered out. */
          parentId: string | null;
          /** Whether the row renders a family disclosure (has sub-threads). */
          hasChildren: boolean;
          /** The lifecycle section this row lives in, or null in the flat
           *  alternate views (drafts/attention/review/running/search), which have
           *  no collapsible sections — ←/→ are inert there. */
          sectionKey: DisplaySection | null;
      };

const SECTION_KEY_PREFIX = '__section_';
/** The stable nav + FLIP key for a section header (`__section_<name>`). */
export function sectionNavKey(sectionKey: string): string {
    return `${SECTION_KEY_PREFIX}${sectionKey}`;
}
function isSectionNavKey(key: string): boolean {
    return key.startsWith(SECTION_KEY_PREFIX);
}
/** Highlight identity for a node: a section's nav key, or a thread's id. */
export function nodeKey(node: DrawerNavNode): string {
    return node.kind === 'section' ? sectionNavKey(node.sectionKey) : node.id;
}

/** Stable DOM id for a nav node, used as the drawer container's
 *  `aria-activedescendant` target and on each row / section header. Namespaced so
 *  it can't collide with ids elsewhere; `null` (no highlight) → `undefined` so the
 *  attribute is omitted. Pure — exported for unit testing. */
export function navKeyDomId(key: string | null): string | undefined {
    return key ? `drawer-nav-${key}` : undefined;
}

/** Flat (depth-0, no-section, no-family) nodes for the alternate views — ←/→ are
 *  inert on these; only ↑/↓/Enter apply. */
function flatThreadNodes(ids: readonly string[]): DrawerNavNode[] {
    return ids.map((id): DrawerNavNode => ({
        kind: 'thread', id, depth: 0, parentId: null, hasChildren: false, sectionKey: null,
    }));
}

/** Currently keyboard-highlighted node in the drawer — a thread id or a section
 *  nav key (`__section_<name>`). */
const highlightedKey = signal<string | null>(null);

/** Ordered list of keyboard-navigable nodes, set by ThreadList (section headers +
 *  threads) or the flat alternate views (threads only). */
const navNodes = signal<DrawerNavNode[]>([]);

export function selectHighlighted() {
    const key = highlightedKey.value;
    if (!key) return;
    // Enter on a section header toggles its collapse; on a thread, focuses it.
    if (isSectionNavKey(key)) {
        toggleSectionCollapse(key.slice(SECTION_KEY_PREFIX.length));
        return;
    }
    const id = key;
    const searchResult = threadSearchResults.value;
    if (threadSearchQuery.value.trim().length > 0 && searchResult.status === 'loaded') {
        const match = searchResult.data.find((r: ThreadSearchResult) => r.thread_id === id);
        if (match) void ensureThreadInMap(match);
    }
    focusThread(id);
}

/** Open the highlighted thread row's overflow (⋯) menu — the keyboard route to
 *  every per-row action (pin/unpin, archive, copy, download, info), since the
 *  inline row buttons are `tabindex=-1` (mouse-only) so the drawer stays a single
 *  tab stop. Driven by the customizable `openThreadActions` shortcut. No-op unless
 *  the drawer is focused and a THREAD (not a section header) is highlighted. Opens
 *  the menu by clicking its trigger, located the same way the highlight scroller
 *  finds rows (`[data-thread-nav]`); the menu's Overlay then owns focus/Escape. */
export function openHighlightedThreadActions(): void {
    if (focusedPane.value !== 'drawer') return;
    const key = highlightedKey.value;
    if (!key || isSectionNavKey(key)) return;
    const trigger = document.querySelector<HTMLElement>(
        `[data-thread-nav="${key}"] button[aria-haspopup="menu"]`,
    );
    trigger?.click();
}

/** Collapse/expand the FOCUSED thread's own sub-thread family — the keyboard
 *  counterpart to clicking that row's disclosure chevron, but keyed off the open
 *  (focused) thread rather than the drawer-highlighted row, so it works from any
 *  pane without first moving focus into the drawer. Unlike the drawer's ←/→ tree
 *  nav it NEVER climbs to a parent: it always toggles the focused thread's own
 *  family, so collapsing it hides that thread's entire descendant subtree (direct
 *  sub-threads and theirs). No-op when no thread is focused or the focused thread
 *  has no sub-threads (nothing to toggle). Driven by the `toggleSubthreads`
 *  shortcut. */
export function toggleFocusedThreadFamily(): void {
    const id = focusedThreadId.value;
    if (!id) return;
    const thread = threadMap.value.get(id);
    if (!thread || thread.meta.totalChildrenCount <= 0) return;
    toggleFamilyCollapse(id);
}

/** Live collapse state for the pure ←/→ decision functions. */
export interface NavCollapseState {
    sectionCollapsed: (sectionKey: string) => boolean;
    familyCollapsed: (threadId: string) => boolean;
}

/** What ←/→ should do to the highlighted node. Computed purely by `leftAction` /
 *  `rightAction` and applied by `applyCollapseAction`, so the tree semantics are
 *  unit-testable without signals or the DOM. */
export type NavCollapseAction =
    | { type: 'none' }
    /** Collapse a section; highlight lands on (stays on) its header. */
    | { type: 'collapseSection'; sectionKey: string }
    /** Expand a section; highlight stays on its header. */
    | { type: 'expandSection'; sectionKey: string }
    /** Collapse a family; highlight moves to `focusKey` — the thread itself when
     *  collapsing its own family, or the parent when collapsing from a child. */
    | { type: 'collapseFamily'; threadId: string; focusKey: string }
    /** Expand a family; highlight stays on the thread. */
    | { type: 'expandFamily'; threadId: string }
    /** Pure move (descend into a revealed child / first row); highlight → `key`. */
    | { type: 'focusKey'; key: string };

/** ← (collapse / ascend), tree-style. On a section header: collapse it (no-op if
 *  already collapsed). On a thread with an expanded family: collapse that family,
 *  staying put. On a sub-thread otherwise: collapse the PARENT's family (hiding
 *  it and its siblings) and focus the parent. On a top-level thread with nothing
 *  left to collapse: collapse the whole section and focus its header. Pure. */
export function leftAction(node: DrawerNavNode, st: NavCollapseState): NavCollapseAction {
    if (node.kind === 'section') {
        return st.sectionCollapsed(node.sectionKey)
            ? { type: 'none' }
            : { type: 'collapseSection', sectionKey: node.sectionKey };
    }
    if (node.hasChildren && !st.familyCollapsed(node.id)) {
        return { type: 'collapseFamily', threadId: node.id, focusKey: node.id };
    }
    if (node.depth > 0 && node.parentId) {
        return { type: 'collapseFamily', threadId: node.parentId, focusKey: node.parentId };
    }
    if (node.sectionKey) {
        return { type: 'collapseSection', sectionKey: node.sectionKey };
    }
    return { type: 'none' };
}

/** → (expand / descend), tree-style. On a collapsed section/family: expand it,
 *  staying put. On an expanded section: descend to its first thread. On an
 *  expanded parent thread: descend to its first child. On a leaf: nothing.
 *  Pure — `nodes`/`idx` locate the node so descend can read the row that follows. */
export function rightAction(
    node: DrawerNavNode,
    nodes: readonly DrawerNavNode[],
    idx: number,
    st: NavCollapseState,
): NavCollapseAction {
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
    if (node.hasChildren && !st.familyCollapsed(node.id)) {
        // The first child renders immediately after the parent, one level deeper.
        const next = nodes[idx + 1];
        return next && next.kind === 'thread' && next.depth > node.depth
            ? { type: 'focusKey', key: nodeKey(next) }
            : { type: 'none' };
    }
    return { type: 'none' };
}

function setHighlight(key: string): void {
    highlightedKey.value = key;
    scrollNavKeyIntoView(key);
}

function scrollNavKeyIntoView(key: string): void {
    const sel = isSectionNavKey(key) ? `[data-flip-id="${key}"]` : `[data-thread-nav="${key}"]`;
    document.querySelector(sel)?.scrollIntoView({ block: 'nearest' });
}

function liveCollapseState(): NavCollapseState {
    return {
        sectionCollapsed: (sectionKey) => collapsedSections.value.has(sectionKey),
        familyCollapsed: (threadId) => collapsedFamilies.value.has(threadId),
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

/** ←: collapse the highlighted node tree-style. Returns whether it acted, so the
 *  caller only consumes the keystroke when something happened. */
export function collapseHighlighted(): boolean {
    const idx = highlightedNodeIndex();
    if (idx < 0) return false;
    return applyCollapseAction(leftAction(navNodes.value[idx], liveCollapseState()));
}

/** →: expand the highlighted node / descend into it. Returns whether it acted. */
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

/** Pure seed: where the keyboard highlight starts when the drawer gains focus —
 *  the open thread if it's navigable, else the first node (the top section header
 *  when there's no focused thread), else null (empty list). Exported for unit
 *  testing. */
export function pickInitialHighlight(openThreadId: string | null, navKeys: string[]): string | null {
    if (openThreadId && navKeys.includes(openThreadId)) return openThreadId;
    return navKeys[0] ?? null;
}

/** Seed the keyboard highlight when the drawer is focused via ⌘⇧1, so Enter has
 *  an immediate target. `navNodes` is set in a post-render effect, so on a fresh
 *  open it can still be empty for the first frame(s); retry a few frames while
 *  it's empty so the seed lands on a real node. With a focused thread the seed is
 *  that thread; without one it falls to the top section header. A genuinely empty
 *  list just seeds null after the retries, which is harmless. */
export function seedDrawerHighlight(): void {
    let tries = 0;
    const seed = () => {
        const keys = navNodes.value.map(nodeKey);
        if (keys.length === 0 && tries++ < 3) { requestAnimationFrame(seed); return; }
        highlightedKey.value = pickInitialHighlight(focusedThreadId.value, keys);
    };
    requestAnimationFrame(seed);
}

/** The drawer pane's list-nav keydown handler. Module-level (it reads only
 *  signals and module functions) so the filter-panel suppression below is
 *  unit-testable without mounting the pane. */
export function handleDrawerKeyDown(e: KeyboardEvent): void {
    // The filter panel covers the list and owns its own controls: an Enter on one
    // of its rows must not ALSO fire "open the highlighted thread" as the key
    // bubbles out to this container, and its arrows must not walk a list nobody
    // can see.
    if (threadFilterPanelOpen.value) return;
    // List-nav owns only un-modified arrows/Enter. Any chord with a primary
    // modifier or Alt is a global shortcut (⌘⇧↵ maximize pane group, ⌘⌥↑↓
    // history, …), so let it bubble to the document handler instead of stealing
    // Enter as "select highlighted" / arrows as "move highlight".
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
        // Tree collapse: own family → parent's family → section, focusing the
        // node revealed by each step. Consume the key only when it acts; an
        // unhandled ← falls through (nothing else binds the horizontal arrows).
        if (collapseHighlighted()) e.preventDefault();
    } else if (e.key === 'ArrowRight') {
        // Tree expand / descend, the inverse of ←.
        if (expandHighlighted()) e.preventDefault();
    }
}

export function ThreadDrawer({ forceVisible }: { forceVisible?: boolean } = {}) {
    // On mobile, the drawer overlay is disabled — mobile has a dedicated threads
    // pane (pane 0) that keeps dots, header, and content in sync via mobileView.
    // The drawer overlay (threadDrawerOpen) is desktop-only.
    const visible = forceVisible || (threadDrawerOpen.value && splitRatio.value > 0);
    // Keep the list mounted through the width-collapse transition —
    // unmounting at close start blanks the drawer body while it's still
    // sliding shut. Scaled by the Animation speed slider, exactly as the
    // transition it outlives is (the 50ms is slack, so it stays outside the
    // scaled term); reading the scale here subscribes this component to the
    // slider, which is what makes a mid-session change take effect.
    const renderContent = useLingeringFlag(visible, scaledDurationMs(PANE_TRANSITION_MS) + 50);
    const isSearching = threadSearchQuery.value.trim().length > 0;
    const view = drawerView.value;
    // Search overrides the selected view; otherwise the selector decides.
    const activeView = isSearching ? 'search' : view;
    const filterPanelOpen = threadFilterPanelOpen.value;

    const drawerRef = useRef<HTMLDivElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    // Don't restore while in an alternate view — saved offset is for the full list.
    useScrollMemory(listRef, 'lucidos-scroll-thread-drawer', { paused: activeView !== 'all' });

    // Keep the container's `aria-activedescendant` in lockstep with the keyboard
    // highlight WITHOUT re-rendering the list. The drawer is a single focusable
    // (role="tree", tabindex=0); the active row is pointed to by id rather than
    // moving DOM focus per row (the aria-activedescendant model — see the plan).
    // Setting the attribute imperatively in a signal effect preserves the drawer's
    // per-row render budget: reading `highlightedKey` in the component body would
    // re-run ThreadList on every ↑/↓, the storm DrawerSectionTitle/ThreadRow were
    // split out to avoid. Rows + section headers carry matching `navKeyDomId` ids.
    useSignalEffect(() => {
        const id = navKeyDomId(highlightedKey.value);
        const el = drawerRef.current;
        if (!el) return;
        if (id) el.setAttribute('aria-activedescendant', id);
        else el.removeAttribute('aria-activedescendant');
    });

    useEffect(() => {
        highlightedKey.value = null;
    }, [activeView]);

    // The filter panel is a view OF this pane, so it cannot outlive the pane
    // being visible. Its open state is a signal and it holds an Escape-registry
    // entry, so a drawer closed (or a thread side collapsed) with the panel up
    // would otherwise leave an invisible surface eating the user's next Escape,
    // and reopening the drawer would land on the filter instead of the list.
    // Mobile passes `forceVisible`, so this never fires there: the threads pane
    // keeps whichever view it was showing when the user swiped away, which is
    // what every other pane does.
    useEffect(() => {
        if (!visible) closeThreadFilterPanel();
    }, [visible]);

    return (
        <div ref={drawerRef}
             class={`thread-drawer${visible ? '' : ' thread-drawer-collapsed'}`}
             style={visible ? { width: `${threadDrawerWidth.value}px` } : undefined}
             onKeyDown={handleDrawerKeyDown}
             onPointerDown={(e) => handleDrawerPointerDown(e.target)}
             role="tree"
             aria-label="Threads"
             // The single tab stop — but only while open. A collapsed drawer
             // (width 0) must not be a phantom tab stop, so it drops to -1.
             tabIndex={visible ? 0 : -1}>
            <div class="thread-drawer-list" ref={listRef}>
                {renderContent && (
                    activeView === 'search' ? <SearchResults />
                    : activeView === 'drafts' ? <DraftsList />
                    : activeView === 'attention' ? <AttentionList />
                    : activeView === 'review' ? <ReviewList />
                    : activeView === 'running' ? <RunningList />
                    : <ThreadList />
                )}
            </div>
            {/* The filter panel is a view inside THIS pane, covering the list
                rather than replacing it, so the list stays mounted underneath.
                That used to be load-bearing, because the refetch effect's guard
                was seeded at mount and a change it missed was a change nobody
                caught up on. It is not any more, twice over: the list renders
                and pages from the *applied thread filter*, which deliberately
                does NOT move while the panel covers it, and the guard is
                `filterChangedSinceLoad()`, which compares against what the
                loaded window was actually fetched against, so an unmounted list
                reloads on its next mount. Which is what makes the four STATUS
                views safe, since those really do replace this list. */}
            {filterPanelOpen && <ThreadFilterPanel onClose={closeThreadFilterPanel} />}
        </div>
    );
}


import { attentionThreads, reviewThreads, runningThreads, composingThreads, computeDrawerCategorization, depthStyle, draftThreads, hasCollapsedAncestor, nestByParent, threadHasUnsentDraft } from './family-graph';
import type { DrawerCategorization, NestedThread } from './family-graph';
export * from './family-graph';
function ThreadList() {
    const containerRef = useRef<HTMLDivElement>(null);
    const portalRef = useRef<HTMLDivElement>(null);
    const sentinelRef = useRef<HTMLDivElement>(null);
    const hydrated = threadsLoaded.value;

    // The *applied* selection, not the live signals the filter panel's
    // checkboxes write: while that panel is up it covers this list completely,
    // so reading it live meant every tick recategorized, rebuilt every row and
    // refetched behind an opaque panel, delaying the paint of the very checkbox
    // the user just ticked. See `store/appliedThreadFilter.ts`.
    const applied = appliedThreadFilter.value;
    // Empty channel filter means "show nothing" — including composing threads.
    // Otherwise the composing drafts would be the only thing visible.
    const composingList = applied.channels.size === 0 ? [] : composingThreads(threadMap.value);

    // The categorization pipeline (family graph → top-thread filter → section
    // routing → sort) is O(loaded threads) across several passes. It used to run
    // on EVERY ThreadList render — including renders triggered by signals it does
    // NOT depend on (a section collapse, the archive-count refresh, a pagination
    // loading-flip). In a workspace with many coding-agent families the loaded set
    // (inbox + saved + archive page + family extensions) is large enough that
    // re-running it per render blocks the main thread and delays click handling
    // (dev-ws input lag, 2026-06-24). Read its real inputs into locals — so the
    // component still SUBSCRIBES to them — and memoize on exactly those, so a
    // re-render that didn't change them reuses the result. The four filter sets
    // arrive as ONE object whose identity changes only when the selection really
    // moved (`appliedThreadFilter`), which is what lets the whole facet
    // selection be a single memo key. Collapse-filtering and nesting stay below:
    // they key on `collapsedFamilies` and are cheap over the already-categorized
    // rendered set.
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
                categorized: { current: [], saved: [], archive: [], statusMap: new Map() },
                familyGraph: { byId: new Map(), rootByThread: new Map() },
                decorations: { routedByThread: new Map(), liftedRoots: new Set(), archivedSubThreads: new Set() },
            },
        [hydrated, threadMapValue, applied],
    );
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
    // A section's header shimmers (AI running-text) while any thread it holds is
    // running — checked over the section's FULL flat list (incl. nested threads
    // and collapsed families), not the post-collapse render list, so the signal
    // survives collapsing the section or a running thread's family. statusMap is
    // the same status snapshot the rows render their dots from.
    const sectionHasRunning = (threads: ThreadState[]) =>
        threads.some(t => statusMap.get(t.meta.id) === 'running');
    const sectionByName: Record<DisplaySection, { name: DisplaySection; count: number; threads: NestedThread[]; hasRunning: boolean }> = {
        saved: { name: 'saved', count: saved.length, threads: filterCollapsed(nestByParent(saved)), hasRunning: sectionHasRunning(saved) },
        current: {
            name: 'current',
            count: composingList.length + current.length,
            threads: [...nestByParent(composingList), ...filterCollapsed(nestByParent(current))],
            hasRunning: sectionHasRunning(current),
        },
        archive: { name: 'archive', count: archiveCount, threads: filterCollapsed(nestByParent(archive)), hasRunning: sectionHasRunning(archive) },
    };
    const sections = THREAD_DRAWER_SECTION_ORDER.map(name => sectionByName[name]);

    const sectionDefs = sections
        .filter(s => s.threads.length > 0)
        .map(s => ({
            name: s.name,
            ids: [
                sectionNavKey(s.name),
                ...s.threads.map(n => n.thread.meta.id),
            ],
        }));

    // Build the keyboard-navigable node list: each rendered section header,
    // followed by its visible (non-collapsed) thread rows — the exact order the
    // drawer renders. ↑/↓ walk these; ←/→ collapse/expand them tree-style.
    const collapsed = collapsedSections.value;
    const navList: DrawerNavNode[] = [];
    for (const s of sections) {
        if (s.threads.length === 0) continue; // empty sections render null
        navList.push({ kind: 'section', sectionKey: s.name });
        if (collapsed.has(s.name)) continue;
        for (const n of s.threads) {
            navList.push({
                kind: 'thread',
                id: n.thread.meta.id,
                depth: n.depth,
                // depth > 0 guarantees the direct parent is rendered (nestByParent
                // only nests under a present parent), so it's a valid focus target.
                parentId: n.depth > 0 ? (n.thread.meta.parentThreadId ?? null) : null,
                hasChildren: n.thread.meta.totalChildrenCount > 0,
                sectionKey: s.name,
            });
        }
    }
    // Key the effect on structure AND the ←/→-relevant fields, so a thread
    // gaining/losing children (or a re-nest) refreshes the cached nodes.
    const navKey = navList
        .map(n => n.kind === 'section'
            ? sectionNavKey(n.sectionKey)
            : `${n.id}:${n.depth}:${n.parentId ?? ''}:${n.hasChildren ? 1 : 0}`)
        .join(',');
    useEffect(() => { navNodes.value = navList; }, [navKey]);

    // Reset key: the whole applied selection, not just its channel set. A filter
    // change re-populates the list wholesale, which is not threads moving
    // between sections, so nothing should fly. Keyed on the channels alone, a
    // repo / app / trigger sub-selection change animated the swap.
    useFlipTransitions(containerRef, portalRef, sectionDefs, applied);

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
    // viewport off.
    //
    // The guard is `filterChangedSinceLoad()`, a STORE fact, not a `useRef`
    // seeded at mount. This list renders under the default `all` status only,
    // and the Filter panel can change the selection under the other four, so a
    // per-instance ref answers the wrong question twice: it misses the change
    // (nobody is watching) and then seeds with the new value, suppressing the
    // catch-up on the way back. Comparing against what was actually fetched
    // makes the mount run correct rather than merely skipped: silent on a
    // window that already matches, and a full reload on one that does not.
    //
    // Keyed on the APPLIED selection, so a run of ticks made while the filter
    // panel covers this list settles into ONE reload when the panel closes,
    // rather than a page fetch plus an archived-count fetch plus a fill loop
    // per tick, all for rows nobody can see.
    useEffect(() => {
        // Before the first window lands there is nothing to bring up to date,
        // and the initial load stamps the selection it fetched against.
        if (!hydrated || !filterChangedSinceLoad()) return;
        void reloadAfterFilterChange().then(() => void loadWhileSentinelVisible());
    }, [hydrated, applied]);

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
                    const { title, Icon } = SECTION_META[s.name];
                    return (
                        <DrawerSection key={s.name} sectionKey={s.name} title={title} Icon={Icon} count={s.count} hasRunning={s.hasRunning}>
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
                                        isArchivedSubThread={decorations.archivedSubThreads.has(id)}
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

/** Shared collapsed state so ThreadList can read it when building navNodes. */
const collapsedSections = signal(loadStringSet(COLLAPSED_KEY));

/** Per-family collapse state keyed by parent thread id. Mirrors the
 *  `collapsedSections` precedent: localStorage-backed, per-device, not
 *  event-sourced. */
const collapsedFamilies = signal(loadStringSet(COLLAPSED_FAMILIES_KEY));

/** Collapse/expand a lifecycle section. The single source of section-collapse
 *  truth — the header click, Enter, and the ←/→ tree nav all route here. */
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

/** Collapse/expand a thread's sub-thread family. The single source of
 *  family-collapse truth — the disclosure chevron and the ←/→ tree nav route here. */
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

/** Shared icon + label content for EVERY drawer section header — the
 *  collapsible lifecycle headers (Pinned/Current/Archive via `DrawerSection`)
 *  and the flat alternate-view headers (Drafts/Needs attention/Results). Keeping
 *  the markup in one place means the icon-to-label spacing (owned by the
 *  `.thread-drawer .list-section-title` gap) is identical everywhere and can't
 *  drift per section. */
function DrawerSectionHeader({ Icon, title, hasRunning }: { Icon?: ComponentType<{ size?: string }>; title: string; hasRunning?: boolean }) {
    return (
        <>
            {Icon && <span class="drawer-section-icon"><Icon size="0.875rem" /></span>}
            {/* Section label shimmers (AI running-text) while the section
                holds a running thread; otherwise it's a plain label. Uses the
                INVERTED shimmer here — the bold label rests at full strength and
                a muted band sweeps across (the standard dim-base + bright-sweep
                read as "dimmed" against this header's weight; the in-thread
                step/status shimmer keeps the standard direction). */}
            <span class={`drawer-section-label${hasRunning ? ' running-shimmer running-shimmer-invert' : ''}`}>{title}</span>
        </>
    );
}

function DrawerSection({ sectionKey, title, Icon, count, hasRunning, children }: { sectionKey: string; title: string; Icon?: ComponentType<{ size?: string }>; count: number; hasRunning?: boolean; children: ComponentChildren }) {
    const collapsed = collapsedSections.value.has(sectionKey);
    return (
        <div class={`drawer-section${collapsed ? ' drawer-section-collapsed' : ''}`}>
            <DrawerSectionTitle
                sectionKey={sectionKey} title={title} Icon={Icon}
                count={count} hasRunning={hasRunning} collapsed={collapsed}
            />
            {!collapsed && children}
        </div>
    );
}

/** The clickable, keyboard-highlightable section header row. Split out from
 *  DrawerSection so the highlight subscription (`highlightedKey`) lives here:
 *  moving the ↑/↓ highlight among threads then re-renders only the (≤3) section
 *  headers, not each section's whole subtree. */
function DrawerSectionTitle({ sectionKey, title, Icon, count, hasRunning, collapsed }: { sectionKey: string; title: string; Icon?: ComponentType<{ size?: string }>; count: number; hasRunning?: boolean; collapsed: boolean }) {
    const highlighted = highlightedKey.value === sectionNavKey(sectionKey);
    return (
        <div class={`list-section-title list-section-title-collapsible${collapsed ? ' collapsed' : ''}${highlighted ? ' list-section-title-highlighted' : ''}`}
             id={navKeyDomId(sectionNavKey(sectionKey))}
             data-flip-id={sectionNavKey(sectionKey)}
             onClick={() => toggleSectionCollapse(sectionKey)}
             role="treeitem"
             aria-selected={highlighted}
             aria-expanded={!collapsed}>
            <DrawerSectionHeader Icon={Icon} title={title} hasRunning={hasRunning} />
            {/* Thread count rides in a badge only while the section is
                collapsed — expanded sections show the rows themselves. */}
            {collapsed && <span class="collapse-count-badge">{count}</span>}
        </div>
    );
}

/** The repo / app / trigger name chip. A long name WRAPS inside the chip's
 *  CSS `max-width` (see `.thread-row-context` in drawer.css), but a constrained
 *  box sits at its constraint, not at the widest resulting line — so a name that
 *  wraps to e.g. "Habit" / "Tracker" leaves dead space to the right of the
 *  shorter line. Pure CSS can't shrink a box to the longest line of *wrapped*
 *  text (the line breaks depend on the width we'd be deriving from them), so we
 *  measure the rendered line boxes and pin the chip to its widest line. A
 *  single-line name needs no measurement — flex `align-items: flex-end` +
 *  fit-content already hugs it (max-content < max-width), so we leave it alone.
 *
 *  Four things make the pin robust to WHEN it runs, to transforms, and to UI
 *  scale — the chip may only ever SHRINK below max-width, never below a word:
 *
 *  1. Measure at the chip's own `max-width`, NOT its live rendered width.
 *     `.thread-row` is `.list-row` (flex-wrap) and the drawer animates its own
 *     width (`transition: width` on open, plus resize-divider drags), so
 *     measuring the LIVE box while the chip laid out mid-transition (drawer still
 *     animating open from width 0, or the right column momentarily wrapped narrow)
 *     would read a transient TINY box and pin the name into single-word /
 *     broken-word lines. The chip's `flex-shrink: 0` column never squeezes it
 *     below `max-width` at rest, so the max-width wrap IS the rest-state wrap:
 *     forcing the measurement to that width makes the pin independent of the
 *     animation frame (every word fits within max-width, so the widest wrapped
 *     line is always ≥ the widest word).
 *
 *  2. Divide the measured line widths by the element's own scale factor
 *     (visual `getBoundingClientRect` width ÷ layout `offsetWidth`).
 *     `getClientRects` returns TRANSFORM-scaled geometry, so a FLIP row animation
 *     (the wrapper animates `scale(0.95 → 1.03)` on mount / reorder) mid-measure
 *     would shrink every line and pin a hair too narrow (the intermittent
 *     "Track / er" break). Un-scaling recovers the true line widths at any frame.
 *
 *  3. Pin the width in `rem` (px ÷ root font-size), NOT `px`. The chip's
 *     font-size and padding are rem-based, so a rem width tracks the text when the
 *     user changes UI scale (root font-size) WITHOUT a reload or a re-measure. A
 *     px pin goes stale on a scale change (too narrow scaling up → words break;
 *     too wide scaling down → dead space) until a reload re-measures.
 *
 *  4. Safety net: if the pin somehow raised the wrapped line count (a corrupted
 *     measurement we didn't anticipate), drop the pin and fall back to the
 *     max-width box — dead space, never a broken word. Line COUNT is
 *     transform-invariant, so this check holds even mid-animation.
 *
 *  Verified in Chromium across the full UI-scale matrix and every FLIP scale
 *  (0.7 → 1.03): no word break and no dead space in any cell. */
function ContextChip({ name }: { name: string }) {
    const ref = useRef<HTMLSpanElement>(null);
    useLayoutEffect(() => {
        const el = ref.current;
        if (!el) return;
        const cs = getComputedStyle(el);
        // (1) Force the wrap to the chip's max-width, not the transient live width,
        // so the measurement reproduces the rest state regardless of the drawer's
        // current open/resize animation frame. Fall back to natural width only if
        // max-width is somehow not a length (e.g. overridden to `none`).
        const maxW = parseFloat(cs.maxWidth);
        el.style.width = Number.isFinite(maxW) && maxW > 0 ? `${maxW}px` : '';
        const range = document.createRange();
        range.selectNodeContents(el);
        const rects = range.getClientRects();
        const naturalLines = rects.length;
        if (naturalLines <= 1) { el.style.width = ''; return; } // single line — hug via fit-content
        // (2) Un-scale: getClientRects is transform-scaled, so divide by this
        // element's own scale (visual ÷ layout width) to recover true line widths
        // even while a FLIP `scale(...)` animation runs on an ancestor.
        const scaleX = el.offsetWidth > 0 ? el.getBoundingClientRect().width / el.offsetWidth : 1;
        let maxLine = 0;
        for (let i = 0; i < rects.length; i++) maxLine = Math.max(maxLine, rects[i].width / scaleX);
        // box-sizing is border-box app-wide (base.css), so the pinned width must
        // include the chip's own padding + border; the rects cover text only.
        const chrome = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight)
            + parseFloat(cs.borderLeftWidth) + parseFloat(cs.borderRightWidth);
        // (3) +1px absorbs sub-pixel rounding so the widest line never re-wraps.
        // Pin in rem (px / root-font-size) so the width tracks the text across UI scale.
        el.style.width = `${(Math.ceil(maxLine) + chrome + 1) / getRemPx()}rem`;
        // (4) Safety net: if the pin increased the line count, a word may have
        // broken — revert to the max-width box (dead space beats a broken word).
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
    // A coding draft hasn't bound a backend yet — it spawns with THIS draft's
    // resolved backend at send time (see sendCompose), so the tag reflects that
    // draft's pick (Codex vs Claude Code) rather than always "Claude Code" or a
    // global another draft changed. A plain chat draft is a Lucidos thread —
    // `formatThreadChannelLabel('chat')` reads "Lucidos Agent", the same tag
    // started chat threads wear.
    const draftMode = getDraft(thread.meta.id).mode;
    const modeLabel = draftMode === 'claude_code'
        ? formatThreadChannelLabel('claude_code', resolveCodingAgent(thread.meta.id))
        : formatThreadChannelLabel('chat');
    // The repo/app chip mirrors started threads. A coding draft hasn't bound its
    // meta yet, so it reads THIS draft's per-draft scope override (the same value
    // `sendCompose` would bind) rather than `meta.repoName`/`codingAgentKind`.
    const reposLoadable = repositories.value;
    const draftScope = resolveScope(thread.meta.id);
    const contextName = composeDraftContextName(
        draftMode,
        draftScope,
        reposLoadable.status === 'loaded' ? reposLoadable.data : [],
    );
    const createdLabel = formatCreatedTimestamp(thread.meta.createdAt);

    return (
        <div data-flip-id={thread.meta.id} style={depthStyle(depth)} class={depth > 0 ? 'thread-row-wrap is-nested' : 'thread-row-wrap'}>
            {/* Dot lives on the wrapper (outside the row's nested clip-path) so it
                holds a fixed left column at every depth — matching ThreadRowContent. */}
            <ThreadStatusIcon status="idle" />
            {/* The draft's structured details (Status / Type / Created) live behind
                the ⋯ menu's Info item now, not a row tooltip — see DraftOverflowMenu. */}
            <div class={classes.join(' ')}
                 id={navKeyDomId(thread.meta.id)}
                 data-thread-nav={thread.meta.id}
                 role="treeitem"
                 aria-selected={isHighlighted}
                 // focusThread reveals the thread pane itself (revealThreadPane
                 // handles the mobile swipe + desktop pane-group focus).
                 onClick={() => focusThread(thread.meta.id)}>
                <div class="thread-row-left">
                    <span class="thread-row-title-row">
                        <span class="thread-row-title">{threadDisplayTitle(thread)}</span>
                        <span class="draft-indicator">Draft</span>
                    </span>
                    {createdLabel && <span class="thread-row-created">{createdLabel}</span>}
                </div>
                <div class="thread-row-right">
                    {modeLabel && <span class="label message-channel-tag">{modeLabel}</span>}
                    {contextName && <ContextChip name={contextName} />}
                    <span class="thread-row-actions">
                        {/* Mouse-only (tabIndex=-1): the drawer is a single tab stop;
                            the keyboard reaches draft actions via the ⋯ menu shortcut. */}
                        <DraftOverflowMenu
                            threadId={thread.meta.id}
                            mode={draftMode}
                            scope={draftScope}
                            contextName={contextName}
                            createdAt={thread.meta.createdAt}
                            stopPropagation
                            extraClass="thread-row-action"
                            tabIndex={-1}
                        />
                    </span>
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
    /** Preformatted absolute created date/time shown as secondary row text.
     *  Empty string omits the line (test fixtures and unhydrated rows can lack
     *  a valid timestamp). */
    createdLabel: string;
    channel: string;
    /** Coding-agent backend for `claude_code`-channel threads — drives the
     *  "Codex" vs "Claude Code" channel tag. Absent for non-coding-agent/legacy rows. */
    codingAgent?: 'claude-code' | 'codex';
    /** Precomputed status dot, derived by the caller via `resolveVisualStatus`
     *  from the snapshot `status` (the same `effectiveThreadStatus` the panel
     *  header uses) — so every drawer row's dot is built in one pass and stays
     *  in lockstep with the list, instead of being re-read live per row. */
    visualStatus: VisualStatus;
    /** Repo / app / trigger name chip shown next to the channel tag. Undefined
     *  for plain chat — a Lucidos thread carries no channel tag and no context
     *  name; the bare row (no chips) is itself the signal it's a regular chat. */
    contextName?: string;
    totalChildren: number;
    needsReview: boolean;
    hasDraft: boolean;
    /** Pinned (saved) state — drives the pin button's filled/outline glyph.
     *  In the memo equality check so a pin toggle repaints just this row. */
    isSaved: boolean;
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
    /** Set when this row's own natural section is Archive while its family
     *  routed to a live section (Current / Pinned). Drives the disabled
     *  styling, so an archived sub-thread listed under a parent with live work
     *  reads as already put away. */
    isArchivedSubThread?: boolean;
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

function ThreadRowContentImpl(props: Partial<ThreadRowContentProps>) {
    const sk = useSkeleton();
    const depth = props.depth ?? 0;
    const hasFamily = !!props.collapsible && (props.totalChildren ?? 0) > 0;

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
    // On the WRAPPER, not the row: the status dot is the wrapper's child (it
    // holds a fixed left column at every depth), and it has to dim with the
    // rest of the row.
    if (props.isArchivedSubThread) wrapClasses.push('is-archived');

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

    // Shared by the disclosure button's click + keydown so the collapse logic
    // lives in one place. stopPropagation keeps the row's onClick from also
    // firing on click, and — for keydown — keeps the drawer container's
    // Enter→selectHighlighted handler from stealing the keystroke (see below).
    const toggleFamily = (e: Event) => {
        e.stopPropagation();
        props.onToggleFamily?.();
    };

    // Every thread carries a channel tag now (chat reads "Lucidos Agent"); the
    // guard stays so a future empty label (or an unknown channel) doesn't paint
    // an empty bordered chip.
    const channelLabel = props.channel ? formatThreadChannelLabel(props.channel, props.codingAgent) : null;

    // The status dot is the wrapper's child, not the row's, so it stays in a
    // fixed left column at every depth — anchored to the un-indented wrapper
    // instead of tracking the title's per-depth indent (see drawer.css).
    return (
        <div class={wrapClasses.join(' ')}
             style={depthStyle(depth)}
             {...(props.flipId ? { 'data-flip-id': props.flipId } : {})}>
            <ThreadStatusIcon status={sk ? null : (props.visualStatus ?? null)} />
            <div class={classes.join(' ')}
                 id={props.id ? navKeyDomId(props.id) : undefined}
                 data-thread-nav={props.id}
                 // Tree-item semantics for the single-focus model: the drawer
                 // container (role="tree", tabindex=0) points its
                 // `aria-activedescendant` at the highlighted row's id; the row
                 // itself never takes DOM focus. Omitted on skeleton rows (no id).
                 role={props.id ? 'treeitem' : undefined}
                 aria-selected={props.id ? (props.isHighlighted ?? false) : undefined}
                 // The thread's structured details (Status / You / Agent / Type /
                 // Exchanges / Started) live behind the ⋯ menu's Info item now,
                 // not a row tooltip — see ThreadOverflowMenu.
                 // Prefetch on press-in: pointerdown fires before the tap's click
                 // (touch-start / mouse-down), so the event load starts earlier and
                 // content is often ready by the time focusThread switches the view.
                 // loadThreadEvents is idempotent (no-op if already loading/loaded,
                 // or if this row isn't in threadMap yet — e.g. a search hit), so a
                 // canceled press just warms the cache and the tap never double-fetches.
                 onPointerDown={sk ? undefined : () => { if (props.id) void loadThreadEvents(props.id); }}
                 onClick={sk ? undefined : props.onClick}>
                {hasFamily && (
                    <button
                        type="button"
                        class="family-disclosure"
                        // Mouse-only: the drawer is a single tab stop, so per-row
                        // controls leave the Tab order. Keyboard collapses/expands
                        // the family via ←/→ on the highlighted row instead.
                        tabIndex={-1}
                        onClick={toggleFamily}
                        onKeyDown={(e) => {
                            // The drawer container's keydown handler intercepts
                            // Enter (selectHighlighted) at the bubble phase and
                            // preventDefaults it, which cancels this native
                            // button's Enter→click activation — so Enter would
                            // otherwise never toggle the family. Handle Enter/Space
                            // here: preventDefault blocks Space page-scroll and the
                            // native synthetic click (so the toggle fires exactly
                            // once), and toggleFamily's stopPropagation keeps the
                            // drawer handler from also acting on the keystroke.
                            if (e.key === 'Enter' || e.key === ' ') {
                                e.preventDefault();
                                toggleFamily(e);
                            }
                        }}
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
                        <SkText class="thread-row-title" w="11rem">{props.title}</SkText>
                        {props.hasDraft && <span class="draft-indicator" data-tooltip="Has unsent draft">Draft</span>}
                    </span>
                    {(sk || props.createdLabel) && <SkText class="thread-row-created" w="5rem">{props.createdLabel}</SkText>}
                </div>
                <div class="thread-row-right">
                    {sk ? (
                        <SkBlock w="4.5rem" h="1.15rem" round />
                    ) : channelLabel ? (
                        <span
                            class={`label message-channel-tag${props.channel === 'error_unknown_channel' ? ' channel-error' : ''}`}
                        >{channelLabel}</span>
                    ) : null}
                    {!sk && props.contextName && <ContextChip name={props.contextName} />}
                    {!sk && props.id && (
                        <span class="thread-row-actions">
                            {/* Mouse-only (tabIndex=-1): the drawer is a single tab
                                stop. The keyboard reaches every row action through
                                the ⋯ menu via the "Open thread actions" shortcut. */}
                            <PinThreadButton threadId={props.id} saved={props.isSaved ?? false} stopPropagation extraClass="thread-row-action" tabIndex={-1} />
                            <ThreadOverflowMenu threadId={props.id} title={props.title ?? ''} stopPropagation extraClass="thread-row-action" tabIndex={-1} />
                        </span>
                    )}
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
    && prev.createdLabel === next.createdLabel
    && prev.channel === next.channel
    && prev.codingAgent === next.codingAgent
    && prev.visualStatus === next.visualStatus
    && prev.contextName === next.contextName
    && prev.totalChildren === next.totalChildren
    && prev.needsReview === next.needsReview
    && prev.hasDraft === next.hasDraft
    && prev.isSaved === next.isSaved
    && prev.isFocused === next.isFocused
    && prev.isHighlighted === next.isHighlighted
    && prev.isLiftedParent === next.isLiftedParent
    && prev.isResponsibleChild === next.isResponsibleChild
    && prev.isArchivedSubThread === next.isArchivedSubThread
    && prev.collapsible === next.collapsible
    && prev.isCollapsed === next.isCollapsed
);

// Reading composeDrafts here would fan a re-render to every visible ThreadRow
// per keystroke — the lag this signal was added to prevent.

export function ThreadRow({ threadId, status, depth = 0, isLiftedParent, isResponsibleChild, isArchivedSubThread, enableFamilyToggle }: {
    threadId: string;
    status: ThreadStatus;
    depth?: number;
    isLiftedParent?: boolean;
    isResponsibleChild?: boolean;
    isArchivedSubThread?: boolean;
    /** Render the bottom-left disclosure chevron when this thread has
     *  sub-threads. Only the nested ThreadList sets it — search / drafts render
     *  flat lists where collapsing nothing visible would be a no-op. */
    enableFamilyToggle?: boolean;
}) {
    // Signal reads stay here so each row's subscription set is narrow: the
    // row re-renders on threadMap / focusedThreadId / highlightedKey /
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
        meta.liveEventWaitCount > 0,
    );
    const isFocused = focusedThreadId.value === meta.id;
    const isHighlighted = highlightedKey.value === meta.id;
    const hasDraft = threadHasUnsentDraft(thread);
    const hasFamily = !!enableFamilyToggle && meta.totalChildrenCount > 0;
    const isCollapsed = hasFamily && collapsedFamilies.value.has(meta.id);

    return (
        <ThreadRowContent
            id={meta.id}
            depth={depth}
            flipId={meta.id}
            title={threadDisplayTitle(thread)}
            createdLabel={formatCreatedTimestamp(meta.createdAt)}
            channel={meta.channel}
            codingAgent={meta.codingAgent}
            visualStatus={visualStatus}
            contextName={threadContextName(meta)}
            totalChildren={meta.totalChildrenCount}
            needsReview={meta.section === 'inbox' && status !== 'running'}
            hasDraft={hasDraft}
            isSaved={meta.saved}
            isFocused={isFocused}
            isHighlighted={isHighlighted}
            isLiftedParent={isLiftedParent}
            isResponsibleChild={isResponsibleChild}
            isArchivedSubThread={isArchivedSubThread}
            collapsible={hasFamily}
            isCollapsed={isCollapsed}
            onToggleFamily={() => toggleFamilyCollapse(meta.id)}
            onClick={() => focusThread(meta.id)}
        />
    );
}

/** One-tap shortcut back to the unfiltered "All statuses" view, offered by every
 *  status-filter view (Drafts / Needs attention / Review / Running) in both
 *  states: under the "nothing here" message when the filter is empty, and under
 *  the last row when it isn't. Either way the user has reached the end of what
 *  this status holds, and what they're after lives under another one, so the way
 *  out sits where they're already looking rather than back up in the filter
 *  control. The all-statuses and search views don't use it: "All statuses" IS
 *  the destination, and an exhausted search wants a different query, not a
 *  filter reset. */
function SeeAllStatusesLink() {
    return (
        <button type="button" class="accent-link" onClick={() => setDrawerView('all')}>
            See all statuses
        </button>
    );
}

/** Empty-state for the four status-filter views: the "nothing here" message
 *  plus the shared shortcut. */
function EmptyFilteredView({ message }: { message: string }) {
    return (
        <div class="empty-state">
            {message}
            <div class="empty-state-action">
                <SeeAllStatusesLink />
            </div>
        </div>
    );
}

/** Trailing twin of `EmptyFilteredView`'s shortcut, closing out a status-filter
 *  view that DID have rows. */
function FilteredViewFooter() {
    return (
        <div class="filtered-view-footer">
            <SeeAllStatusesLink />
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
    useEffect(() => { navNodes.value = flatThreadNodes(ids); }, [idsKey]);

    if (!hydrated) return null;
    if (drafts.length === 0) {
        return <EmptyFilteredView message="No drafts" />;
    }
    return (
        <div>
            <div class="list-section-title">
                <DrawerSectionHeader Icon={DraftsIcon} title="Drafts" />
            </div>
            {drafts.map(t => t.meta.state === 'composing'
                ? <ComposingThreadRow key={t.meta.id} thread={t} />
                : <ThreadRow key={t.meta.id} threadId={t.meta.id} status={effectiveThreadStatus(t)} />)}
            <FilteredViewFooter />
        </div>
    );
}

/** Single-section view of every Current/Saved thread where the agent is stuck
 *  waiting on the user — awaiting an answer/permission or a failed turn (see
 *  `threadNeedsAttention`). Mirrors `DraftsList`: bypasses the
 *  channel/trigger/repo filters and the four lifecycle sections so the user sees
 *  everything that needs them in one place, flat and most-recent-first. Same
 *  pagination caveat as drafts — attention threads ride at the top of the loaded
 *  window. */
function AttentionList() {
    const hydrated = threadsLoaded.value;
    const threads = hydrated ? attentionThreads(threadMap.value) : [];

    const ids = threads.map(t => t.meta.id);
    const idsKey = ids.join(',');
    useEffect(() => { navNodes.value = flatThreadNodes(ids); }, [idsKey]);

    if (!hydrated) return null;
    if (threads.length === 0) {
        return <EmptyFilteredView message="Nothing needs attention" />;
    }
    return (
        <div>
            <div class="list-section-title">
                <DrawerSectionHeader Icon={AttentionIcon} title="Needs attention" />
            </div>
            {threads.map(t => <ThreadRow key={t.meta.id} threadId={t.meta.id} status={effectiveThreadStatus(t)} />)}
            <FilteredViewFooter />
        </div>
    );
}

/** Single-section view of every Current/Saved thread carrying a change ready to
 *  apply (see `threadInReview`). Mirrors `AttentionList`: bypasses the
 *  channel/trigger/repo filters and the four lifecycle sections so the user sees
 *  everything awaiting review in one place, flat and most-recent-first. Same
 *  pagination caveat — review threads ride at the top of the loaded window. */
function ReviewList() {
    const hydrated = threadsLoaded.value;
    const threads = hydrated ? reviewThreads(threadMap.value) : [];

    const ids = threads.map(t => t.meta.id);
    const idsKey = ids.join(',');
    useEffect(() => { navNodes.value = flatThreadNodes(ids); }, [idsKey]);

    if (!hydrated) return null;
    if (threads.length === 0) {
        return <EmptyFilteredView message="Nothing to review" />;
    }
    return (
        <div>
            <div class="list-section-title">
                <DrawerSectionHeader title="Review" />
            </div>
            {threads.map(t => <ThreadRow key={t.meta.id} threadId={t.meta.id} status={effectiveThreadStatus(t)} />)}
            <FilteredViewFooter />
        </div>
    );
}

/** Single-section view of every Current/Saved thread actively working on a
 *  response (see `threadIsRunning`). Mirrors `AttentionList`/`ReviewList`:
 *  bypasses the channel/trigger/repo filters and the four lifecycle sections so
 *  the user sees everything in flight in one place, flat and most-recent-first.
 *  Same pagination caveat — running threads ride at the top of the loaded
 *  window. */
function RunningList() {
    const hydrated = threadsLoaded.value;
    const threads = hydrated ? runningThreads(threadMap.value) : [];

    const ids = threads.map(t => t.meta.id);
    const idsKey = ids.join(',');
    useEffect(() => { navNodes.value = flatThreadNodes(ids); }, [idsKey]);

    if (!hydrated) return null;
    if (threads.length === 0) {
        return <EmptyFilteredView message="Nothing running" />;
    }
    return (
        <div>
            <div class="list-section-title">
                {/* Every thread in this view is running (the empty case already
                    returned), so the header always shimmers — the same "live"
                    affordance the lifecycle sections show while they hold a
                    running thread. */}
                <DrawerSectionHeader Icon={RunningIcon} title="Running" hasRunning />
            </div>
            {threads.map(t => <ThreadRow key={t.meta.id} threadId={t.meta.id} status={effectiveThreadStatus(t)} />)}
            <FilteredViewFooter />
        </div>
    );
}

function SearchResults() {
    const loadable = threadSearchResults.value;
    const showLoading = useDelayedLoading(loadable);

    const resultIds = loadable.status === 'loaded' ? loadable.data.map((r: ThreadSearchResult) => r.thread_id) : [];
    const resultKey = resultIds.join(',');
    useEffect(() => { navNodes.value = flatThreadNodes(resultIds); }, [resultKey]);

    if (loadable.status === 'failed') {
        return <div class="empty-state error-text">Search failed</div>;
    }

    return (
        <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf row={() => <ThreadRowContent />} />}>
            {loadable.status === 'loaded' ? (
                loadable.data.length === 0 ? (
                    <div class="empty-state">No threads found</div>
                ) : (
                    <div>
                        <div class="list-section-title">
                            <DrawerSectionHeader title="Results" />
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
    // Prefer live status from threadMap (SSE-updated), fall back to API result
    const status: ThreadStatus = liveThread ? effectiveThreadStatus(liveThread) : (result.status as ThreadStatus);
    // Same `resolveVisualStatus` formula as the live rows, fed the `status`
    // snapshot above. A search hit not yet hydrated into threadMap has no
    // child/proposal/subscription info, so those default to false.
    const visualStatus = resolveVisualStatus(
        status,
        (liveThread?.meta.activeChildrenCount ?? 0) > 0,
        liveThread?.meta.codingAgentProposed ?? false,
        (liveThread?.meta.liveEventWaitCount ?? 0) > 0,
    );
    const section = liveThread?.meta.section ?? result.section;
    const isFocused = focusedThreadId.value === result.thread_id;
    const isHighlighted = highlightedKey.value === result.thread_id;
    // Context name works whether or not the hit is hydrated into threadMap —
    // ThreadMeta is structurally a ThreadContextFields; the result's snake-case
    // fields map onto the same shape. (The richer details — Status / You / Agent
    // / Exchanges / Started — live behind the ⋯ menu's Info item, which reads the
    // live meta itself, so an unhydrated hit simply omits the Info item.)
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
