import type { ComponentChildren } from 'preact';
import { useRef, useEffect, useLayoutEffect, useCallback, useMemo } from 'preact/hooks';
import { memo } from 'preact/compat';
import { batch, signal, useSignalEffect } from '@preact/signals';
import { threadDrawerOpen, threadDrawerWidth, threadMap, focusedThreadId, threadsLoaded, splitRatio, effectiveThreadStatus, getThreadDisplaySection, threadSearchQuery, threadSearchResults, threadHasMore, threadLoadingMore, archiveThreadCount, drawerView, setDrawerView, repositories, focusedPane, scaledDurationMs, showToast, ALL_CHANNELS, clearThreadFilter } from '../../store/store';
import { appliedThreadFilter, type ThreadFilterSelection } from '../../store/appliedThreadFilter';
import { resolveScope, resolveCodingAgent } from '../../store/composeSelections';
import { composeDraftContextName } from '../../store/composeDestination';
import { threadPassesChannelFilter } from '../../store/threadFilter';
import { threadFilterPanelOpen, setThreadFilterPaneVisible, closeThreadFilterPanel } from '../../store/threadFilterPanel';
import { ThreadFilterCover } from './ThreadFilterCover';
import { focusPane, showThreadList } from '../../store/actions/pane';
import { focusThread } from '../../store/actions/threads';
import { handleArchiveAll } from '../../store/actions/archive-all';
import { openAttentionThread } from '../../store/actions/event-navigation';
import { loadOlderThreads, reloadAfterFilterChange, filterChangedSinceLoad, ensureThreadInMap, loadThreadEvents } from '../../store/actions/thread-loading';
import { ThreadStatusIcon, visualStatusFor, type VisualStatus } from '../shared/ThreadStatusIcon';
import { PinThreadButton } from '../shared/PinThreadButton';
import { ThreadOverflowMenu } from '../shared/ThreadOverflowMenu';
import { DraftOverflowMenu } from '../shared/DraftOverflowMenu';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { SectionHeaderContent } from '../shared/SectionHeader';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import type { ThreadState, ThreadStatus } from '../../store/thread-events';
import { getDraft } from '../../store/composeDrafts';
import type { DisplaySection } from '../../generated/thread-lifecycle';
import { formatThreadChannelLabel } from '../../utils/formatChannel';
import { threadContextName, type ThreadContextFields } from './threadContextName';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { useFlipTransitions, type FlipSection } from '../../hooks/useFlipAnimation';
import { useDelayedFlag, useDelayedLoading, useLingeringFlag } from '../../hooks/useDelayedLoading';
import { PANE_TRANSITION_MS } from '../layout/splitHelpers';
import { forgetSavedScroll, readSavedScroll, useScrollMemory } from '../../hooks/useScrollMemory';
import { useRowActionsGesture } from './useRowActionsGesture';
import { getRemPx } from '../../utils/dom';
import { scrollBehavior } from '../../utils/motion';
import type { ThreadSearchResult } from '../../api/threads';
import { PinIcon, InboxIcon, ArchiveIcon, DraftsIcon, AttentionIcon, RunningIcon, ChevronRightIcon } from '../shared/icons';
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

// The pin's heavy head sits in the top of its box, so centred on the label it
// reads high. This heading lowers it. The pin buttons keep the plain glyph.
function SectionPinIcon({ size }: { size?: string }) {
    return <PinIcon size={size} className="section-icon-pin" />;
}

// Per-section header display: label + icon. The `'saved'` section reads "Pinned"
// in the UI, a label-only override. Every USER-FACING surface says "Pinned" /
// "Pin" / "Unpin". The INTERNAL identifiers still say "saved": the section key,
// `is_saved`, the `ThreadSaved` / `ThreadUnsaved` events, and the
// `/threads/save` + `/unsave` routes. RENAME NOTED, not made: a full-stack
// rename is its own change. The other two sections derive their label from the
// section key.
const SECTION_META: Record<DisplaySection, { title: string; Icon: ComponentType<{ size?: string }> }> = {
    saved: { title: 'Pinned', Icon: SectionPinIcon },
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
           *  under a parent that is also rendered. Null for top-level rows and
           *  for orphans whose parent is paginated or filtered out. */
          parentId: string | null;
          /** Whether the row renders a family disclosure (has sub-threads). */
          hasChildren: boolean;
          /** Direct children hidden by default as fully-archived branches
           *  (`hiddenDirectChildCount`). > 0 means ←/→ can also reveal/hide
           *  this row's archived-reveal toggle, since it has no Tab stop of
           *  its own. 0/undefined in the flat alternate views. */
          hiddenArchivedCount?: number;
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
    const searching = threadSearchQuery.value.trim().length > 0;
    if (searching && searchResult.status === 'loaded') {
        const match = searchResult.data.find((r: ThreadSearchResult) => r.thread_id === id);
        if (match) void ensureThreadInMap(match);
    }
    if (!searching && drawerView.value === 'attention') openAttentionThread(id);
    else focusThread(id);
}

/** Open the highlighted thread row's overflow (⋯) menu: the keyboard route to
 *  every per-row action. The inline row buttons are `tabindex=-1`, so the drawer
 *  stays a single tab stop. Driven by the customizable `openThreadActions`
 *  shortcut. Returns false, and does nothing, unless the drawer is focused and
 *  a drawn THREAD row is highlighted; the shortcut then opens the open thread's
 *  menu. Opens by clicking the trigger, located the way the highlight scroller
 *  finds rows (`[data-thread-nav]`); the menu's Overlay then owns focus and
 *  Escape. */
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

/** Collapse or expand the FOCUSED thread's own sub-thread family, the keyboard
 *  counterpart to clicking that row's disclosure chevron. It is keyed off the
 *  open thread rather than the drawer-highlighted row, so it works from any pane
 *  without first moving focus into the drawer.
 *
 *  Unlike the drawer's ←/→ tree nav it NEVER climbs to a parent. Collapsing
 *  therefore hides the focused thread's entire descendant subtree. A no-op when
 *  no thread is focused, or the focused thread has no sub-threads. Driven by the
 *  `toggleSubthreads` shortcut. */
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
    /** Whether this thread's archived-reveal toggle is currently on. */
    archivedRevealed: (threadId: string) => boolean;
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
    /** Hide a thread's revealed archived children again, staying put. The
     *  toggle has no Tab stop of its own, so ←/→ is its only keyboard path. */
    | { type: 'hideArchived'; threadId: string }
    /** Reveal a thread's hidden archived children; highlight stays put. */
    | { type: 'revealArchived'; threadId: string }
    /** Pure move (descend into a revealed child / first row); highlight → `key`. */
    | { type: 'focusKey'; key: string };

/** ← (collapse / ascend), tree-style. On a section header: collapse it (no-op if
 *  already collapsed). On a thread with an expanded family: collapse that family,
 *  staying put. Next, if its archived-reveal toggle is on: turn it off, staying
 *  put — the toggle has no Tab stop, so this is its only keyboard path. On a
 *  sub-thread otherwise: collapse the PARENT's family (hiding it and its
 *  siblings) and focus the parent. On a top-level thread with nothing left to
 *  collapse: collapse the whole section and focus its header. Pure. */
export function leftAction(node: DrawerNavNode, st: NavCollapseState): NavCollapseAction {
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
    return { type: 'none' };
}

/** → (expand / descend), tree-style. On a collapsed section/family: expand it,
 *  staying put. Next, an off archived-reveal toggle: turn it on, staying put —
 *  checked BEFORE descending, so a row with a live family too still reaches it
 *  by keyboard. On an expanded section: descend to its first thread. On an
 *  expanded parent with nothing left to reveal here: descend to its first
 *  child. Pure — `nodes`/`idx` locate the node so descend reads the next row. */
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
    if (node.hiddenArchivedCount && !st.archivedRevealed(node.id)) {
        return { type: 'revealArchived', threadId: node.id };
    }
    if (node.hasChildren && !st.familyCollapsed(node.id)) {
        // The first child renders immediately after the parent, one level deeper.
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
    const sel = isSectionNavKey(key) ? `[data-flip-id="${key}"]` : `[data-thread-nav="${key}"]`;
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

/** Pure seed: where the keyboard highlight starts when the drawer gains focus.
 *  The open thread if it is navigable, else the first node, else null. */
export function pickInitialHighlight(openThreadId: string | null, navKeys: string[]): string | null {
    if (openThreadId && navKeys.includes(openThreadId)) return openThreadId;
    return navKeys[0] ?? null;
}

/** Seed the keyboard highlight when the drawer is focused via ⌘⇧1, so Enter has
 *  an immediate target. `navNodes` is set in a post-render effect, so on a fresh
 *  open it can be empty for the first frames. Retrying a few frames is what
 *  lands the seed on a real node. A genuinely empty list seeds null after the
 *  retries, which is harmless. */
export function seedDrawerHighlight(): void {
    let tries = 0;
    const seed = () => {
        const keys = navNodes.value.map(nodeKey);
        if (keys.length === 0 && tries++ < 3) { requestAnimationFrame(seed); return; }
        highlightedKey.value = pickInitialHighlight(focusedThreadId.value, keys);
    };
    requestAnimationFrame(seed);
}

/** What it takes to show a thread in the full thread list. */
export type ThreadListReveal =
    /** Expand `section`, each of `collapsedAncestors`, and reveal each of
     *  `archivedAncestorsToReveal`'s hidden archived children, then scroll to
     *  the row. */
    | { kind: 'reveal'; section: DisplaySection; collapsedAncestors: string[]; archivedAncestorsToReveal: string[] }
    /** The thread filter hides the thread's family. */
    | { kind: 'hidden-by-filter' }
    /** No section lists the thread, filter or not. */
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
    return { section: decorations.routedByThread.get(threadId), familyGraph, decorations };
}

/** Walks up from `threadId` while each step is hidden, collecting that step's
 *  PARENT id: the parents whose archived-reveal toggle has to flip on so
 *  `filterHiddenArchived` renders the whole chain down to `threadId`. Mirrors
 *  `collapsedAncestorIds`'s shape, for the same reason: landing on a thread
 *  must reveal it, wherever it is hidden from. */
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

/** Pure: decide how to reveal `threadId` in the full list, under `filter` and
 *  the current family collapse state. Routes through the same categorization
 *  the list renders from, so the section it names is the one the row sits in. */
export function planThreadListReveal(
    threadId: string,
    threads: ThreadState[],
    filter: ThreadFilterSelection,
    collapsed: ReadonlySet<string>,
): ThreadListReveal {
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

/** Show `threadId`'s row in the thread list: open the list, undo whatever
 *  hides the row, and scroll to it. The thread filter is the user's
 *  own choice, so a filtered-out thread gets a toast offering to clear it
 *  rather than a silent reset. */
export function revealThreadInList(threadId: string): void {
    showThreadList();
    closeThreadFilterPanel();
    const plan = planThreadListReveal(
        threadId, Array.from(threadMap.value.values()), appliedThreadFilter.value, collapsedFamilies.value,
    );
    if (plan.kind === 'hidden-by-filter') {
        showToast('This thread is hidden by the thread filter.', 'info', {
            action: { label: 'Clear filter', onClick: () => { clearThreadFilter(); revealThreadInList(threadId); } },
        });
        return;
    }
    if (plan.kind === 'not-listed') {
        showToast('This thread is not in the thread list.', 'info');
        return;
    }
    // Leaving another view re-arms the list's scroll memory, and its restore
    // would land after the scroll below. The reveal decides where the list sits.
    forgetSavedScroll(DRAWER_SCROLL_KEY);
    batch(() => {
        threadSearchQuery.value = '';
        threadSearchResults.value = { status: 'not-loaded' };
        setDrawerView('all');
        setSectionCollapsed(plan.section, false);
        for (const id of plan.collapsedAncestors) setFamilyCollapsed(id, false);
        for (const id of plan.archivedAncestorsToReveal) setArchivedRevealed(id, true);
    });
    scrollToRevealedRow(threadId);
}

/** Frames to wait for the row to mount after the list opens and expands. */
const REVEAL_MAX_FRAMES = 10;
const DRAWER_SCROLL_KEY = 'lucidos-scroll-thread-drawer';
/** The page-in in flight, if any. Its identity is its cancellation token: a
 *  new page-in or an unmount replaces or clears it, and the old loop stops. */
let activePageIn: object | null = null;
/** Set once the drawer's scroll memory stops restoring (`onRestoreSettled`),
 *  and reset before each attach. Work done for the restore stops with it. */
let drawerRestoreSettled = false;
/** Set before each attach and consumed by the one page-in that serves it. A
 *  drawer reopen does not re-attach, so it finds nothing armed and pages
 *  nothing in. */
let drawerRestoreArmed = false;

/** Glide only the list, never its ancestors: on mobile the list sits in a
 *  swipe pane, and `scrollIntoView` would also scroll the swipe container. */
function centreRowInList(row: HTMLElement, list: HTMLElement): void {
    const r = row.getBoundingClientRect();
    const l = list.getBoundingClientRect();
    const top = list.scrollTop + (r.top + r.height / 2) - (l.top + l.height / 2);
    list.scrollTo({ top, behavior: scrollBehavior() });
}

/** The row's centre within the list's content, independent of scroll. The
 *  centre, not the top: the row's own title can re-wrap as the list widens. */
function rowContentCentre(row: HTMLElement, list: HTMLElement): number {
    const r = row.getBoundingClientRect();
    return r.top + r.height / 2 - list.getBoundingClientRect().top + list.scrollTop;
}

function scrollToRevealedRow(threadId: string): void {
    let frames = 0;
    const step = () => {
        const row = document.querySelector<HTMLElement>(`.thread-drawer-list [data-thread-nav="${threadId}"]`);
        const list = row?.closest<HTMLElement>('.thread-drawer-list');
        if (!row || !list) {
            if (frames++ < REVEAL_MAX_FRAMES) requestAnimationFrame(step);
            return;
        }
        followRowUntilSettled(row, list);
    };
    requestAnimationFrame(step);
}

/** Frames the row and the list width must hold still before the reveal stops
 *  following the row. */
const SETTLED_FRAMES = 3;
/** Runaway guard on following a row whose layout never settles. */
const MAX_FOLLOW_FRAMES = 600;

/** Keep the row centred while the layout above it still moves. Opening the
 *  drawer animates its width, and a narrow list wraps rows taller. Chromium's
 *  scroll anchoring hides the drift, but WebKit has none.
 *
 *  Settled means the row's centre AND the list's width held for a few
 *  frames. The rows reflow some frames after the width starts to grow, so the
 *  position alone reads as settled too early. */
function followRowUntilSettled(row: HTMLElement, list: HTMLElement): void {
    let lastCentre = Number.NaN;
    let lastWidth = Number.NaN;
    let stillFrames = 0;
    let frames = 0;
    const tick = () => {
        if (!row.isConnected) return;
        const centre = rowContentCentre(row, list);
        const width = list.clientWidth;
        // A pixel of slack: sub-pixel rounding must not read as movement.
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

/** The drawer pane's list-nav keydown handler. Module-level (it reads only
 *  signals and module functions) so the filter-panel suppression below is
 *  unit-testable without mounting the pane. */
export function handleDrawerKeyDown(e: KeyboardEvent): void {
    // The filter panel covers the list and owns its own controls. An Enter on
    // one of its rows must not ALSO open the highlighted thread as the key
    // bubbles out here. Its arrows must not walk a list nobody can see.
    if (threadFilterPanelOpen.value) return;
    // List-nav owns only un-modified arrows and Enter. Any chord with a primary
    // modifier or Alt is a global shortcut, so it bubbles to the document
    // handler instead.
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
    // The drawer overlay (`threadDrawerOpen`) is desktop-only. Mobile has a
    // dedicated threads pane keeping dots, header and content in sync via
    // `mobileView`.
    const visible = forceVisible || (threadDrawerOpen.value && splitRatio.value > 0);
    // Keep the list mounted through the width-collapse transition. Unmounting
    // at close start blanks the drawer body while it is still sliding shut.
    // Scaled by the Animation speed slider, as the transition it outlives is;
    // the 50ms is slack, so it stays outside the scaled term. Reading the scale
    // here subscribes this component to the slider, which is what makes a
    // mid-session change take effect.
    const renderContent = useLingeringFlag(visible, scaledDurationMs(PANE_TRANSITION_MS) + 50);
    const isSearching = threadSearchQuery.value.trim().length > 0;
    const view = drawerView.value;
    // Search overrides the selected view; otherwise the selector decides.
    const activeView = isSearching ? 'search' : view;

    const drawerRef = useRef<HTMLDivElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    // Don't restore while in an alternate view — saved offset is for the full list.
    // Declared before the hook, so the reset runs before each attach.
    const restorePaused = activeView !== 'all';
    useEffect(() => {
        drawerRestoreSettled = false;
        drawerRestoreArmed = true;
    }, [restorePaused]);
    useScrollMemory(listRef, DRAWER_SCROLL_KEY, {
        paused: restorePaused,
        // Busy until the first threads land and the page-in takes over.
        restoreIsBusy: () => !threadsLoaded.value || drawerRestoreArmed || activePageIn !== null,
        onRestoreSettled: () => {
            drawerRestoreSettled = true;
            drawerRestoreArmed = false;
        },
    });

    // Keep the container's `aria-activedescendant` in lockstep with the keyboard
    // highlight WITHOUT re-rendering the list. The drawer is a single focusable,
    // and the active row is pointed to by id rather than by moving DOM focus.
    // Setting the attribute imperatively in a signal effect preserves the
    // per-row render budget: reading `highlightedKey` in the component body
    // would re-run ThreadList on every ↑/↓, the storm DrawerSectionTitle and
    // ThreadRow were split out to avoid. Rows and section headers carry
    // matching `navKeyDomId` ids.
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

    // The filter panel is a view OF this pane and keeps its open state through
    // a collapse, as the drawer's other views do. Only its Escape entry follows
    // visibility, so a hidden panel never eats the next Escape. Runs at mount
    // too, for a boot with the drawer collapsed.
    useEffect(() => {
        setThreadFilterPaneVisible(visible);
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
            {/* Not a tab stop. Chromium makes a scroll container keyboard
                focusable when it holds no focusable children, and every row
                control here is `tabindex=-1` by design. The drawer would then
                have two tab stops, so a Tab meant to leave it landed on this
                unlabelled div. The container above keeps the arrows, which move
                the highlight and scroll its row into view. */}
            <div class="thread-drawer-list" ref={listRef} tabIndex={-1}>
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
            <ThreadFilterCover paneVisible={visible} />
        </div>
    );
}


import { attentionThreads, reviewThreads, runningThreads, composingThreads, collapsedAncestorIds, computeDrawerCategorization, depthStyle, draftThreads, filterHiddenArchived, nestByParent, renderedFamilyRows, threadHasUnsentDraft, visibleChildrenCount } from './family-graph';
import type { DrawerCategorization, FamilyGraph, NestedThread } from './family-graph';
export * from './family-graph';
function ThreadList() {
    const containerRef = useRef<HTMLDivElement>(null);
    const portalRef = useRef<HTMLDivElement>(null);
    const sentinelRef = useRef<HTMLDivElement>(null);
    const hydrated = threadsLoaded.value;

    // The *applied* selection, not the live signals the filter panel's
    // checkboxes write. That panel covers this list completely, so reading it
    // live means every tick recategorizes, rebuilds every row and refetches
    // behind an opaque panel. That delays the paint of the checkbox the user
    // just ticked. See `store/appliedThreadFilter.ts`.
    const applied = appliedThreadFilter.value;
    // Empty channel filter means "show nothing" — including composing threads.
    // Otherwise the composing drafts would be the only thing visible.
    const composingList = applied.channels.size === 0 ? [] : composingThreads(threadMap.value);

    // The categorization pipeline (family graph → top-thread filter → section
    // routing → sort) is O(loaded threads) across several passes. Renders from
    // signals it does NOT depend on would re-run it for nothing. In a workspace
    // with many coding-agent families that blocks the main thread and delays
    // click handling.
    //
    // So read its real inputs into locals, which keeps the component SUBSCRIBED
    // to them, and memoize the categorization on exactly those. The four filter
    // sets arrive as ONE object (`appliedThreadFilter`) whose identity moves
    // only when the selection does. The whole selection is then one memo key.
    // Collapse-filtering and nesting stay below: they key on `collapsedFamilies`
    // and are cheap over the categorized set.
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
                decorations: {
                    routedByThread: new Map(), liftedRoots: new Set(), archivedSubThreads: new Set(),
                    hiddenArchivedThreads: new Set(), hiddenDirectChildCount: new Map(),
                },
            },
        [hydrated, threadMapValue, applied],
    );
    const { current, saved, archive, statusMap } = categorized;

    // Drop fully-archived branches a live family isn't revealing (per-row "N
    // archived" toggle). Archive's own members are never in `hiddenArchivedThreads`
    // (a fully-archived family already routes there), so it needs no filter.
    const revealedArchivedSet = revealedArchivedFamilies.value;
    const visibleCurrent = filterHiddenArchived(current, decorations.hiddenArchivedThreads, revealedArchivedSet, familyGraph);
    const visibleSaved = filterHiddenArchived(saved, decorations.hiddenArchivedThreads, revealedArchivedSet, familyGraph);

    // Drop descendants of collapsed families. The parent itself stays visible
    // (its disclosure chevron lets the user re-expand).
    const collapsedFamiliesSet = collapsedFamilies.value;
    const familyRows = (visible: ThreadState[]) =>
        renderedFamilyRows(visible, collapsedFamiliesSet, familyGraph, decorations);
    // `count` is the section's full thread total, before family-collapse
    // filtering, and is the number the section's count badge shows. Archived
    // branches hidden by default are excluded too, same as composing/discarded
    // threads — they aren't part of this view until revealed.
    // `threads` is the post-collapse render list. Archive is special: its badge
    // reads the server-sourced `archiveThreadCount` (see `refreshArchivedCount`)
    // so it stays stable as rows page in and never drifts on a collapse.
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
        saved: { name: 'saved', count: visibleSaved.length, threads: familyRows(visibleSaved), hasRunning: sectionHasRunning(saved) },
        current: {
            name: 'current',
            count: composingList.length + visibleCurrent.length,
            threads: [...nestByParent(composingList), ...familyRows(visibleCurrent)],
            hasRunning: sectionHasRunning(current),
        },
        archive: { name: 'archive', count: archiveCount, threads: familyRows(archive), hasRunning: sectionHasRunning(archive) },
    };
    const sections = THREAD_DRAWER_SECTION_ORDER.map(name => sectionByName[name]);

    // The rows as rendered: a collapsed section is its header alone, so the
    // FLIP hook sees its threads leave and return. Its threads are `tucked`, so
    // a thread archived into a collapsed Archive flies onto its header.
    const collapsed = collapsedSections.value;
    const sectionDefs: FlipSection[] = sections
        .filter(s => s.threads.length > 0)
        .map(s => {
            const ids = s.threads.map(n => n.thread.meta.id);
            return collapsed.has(s.name)
                ? { name: s.name, ids: [sectionNavKey(s.name)], tucked: ids }
                : { name: s.name, ids: [sectionNavKey(s.name), ...ids] };
        });

    // Build the keyboard-navigable node list: each rendered section header,
    // followed by its visible (non-collapsed) thread rows — the exact order the
    // drawer renders. ↑/↓ walk these; ←/→ collapse/expand them tree-style.
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
                // Excludes default-hidden archived children: ←/→ here drives the
                // live sub-thread collapse, not the separate archived-reveal toggle.
                hasChildren: visibleChildrenCount(n.thread, decorations) > 0,
                hiddenArchivedCount: decorations.hiddenDirectChildCount.get(n.thread.meta.id) ?? 0,
                sectionKey: s.name,
            });
        }
    }
    // Key the effect on structure AND the ←/→-relevant fields, so a thread
    // gaining/losing children (or a re-nest) refreshes the cached nodes.
    const navKey = navList
        .map(n => n.kind === 'section'
            ? sectionNavKey(n.sectionKey)
            : `${n.id}:${n.depth}:${n.parentId ?? ''}:${n.hasChildren ? 1 : 0}:${n.hiddenArchivedCount ?? 0}`)
        .join(',');
    useEffect(() => { navNodes.value = navList; }, [navKey]);

    // Reset key: the whole applied selection, not just its channel set. A filter
    // change re-populates the list wholesale, which is not threads moving
    // between sections, so nothing should fly. Keyed on the channels alone, a
    // repo / app / trigger sub-selection change animated the swap. The three
    // sets mark a family, section or archived-reveal toggle, which unrolls
    // its rows instead of snapping them.
    useFlipTransitions(containerRef, portalRef, sectionDefs, applied, [collapsedFamiliesSet, collapsed, revealedArchivedSet]);

    // Infinite scroll, part 1: the fill loop. Keep loading until the sentinel is
    // pushed back out of view, or there is nothing more. The loop is
    // load-bearing. An IntersectionObserver fires only on enter/exit
    // *transitions*. So a page that does not refill the viewport leaves the
    // sentinel intersecting with no new event, and pagination stalls.
    // `fillingRef` makes the loop re-entrancy-safe, so a scroll event firing the
    // observer mid-fill cannot double-load.
    const fillingRef = useRef(false);
    const loadWhileSentinelVisible = useCallback(async () => {
        if (fillingRef.current) return;
        const sentinel = sentinelRef.current;
        const root = sentinel?.closest('.thread-drawer-list');
        if (!sentinel || !root) return;
        fillingRef.current = true;
        try {
            // Runaway backstop. Filling a viewport needs 2 to 3 pages, so this
            // cap is far above that. It guards the case where loaded rows render
            // NOWHERE visible, such as filter matches that are all
            // collapsed-family descendants. The map then grows every page, so
            // the size guard never trips, and the sentinel never moves. Without
            // a cap one intersection would page through the whole matching set.
            // Subsequent scrolls resume pagination normally.
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
                // Let the re-render commit so the next iteration measures the new
                // layout (whether the freshly-loaded rows pushed the sentinel out).
                await nextFrame();
                // Stop when nothing was FETCHED: a concurrent call owns the round
                // trip, or the page failed. A landed page that added no rows is
                // not a stop, since its cursor moved and the next page continues
                // below it. Bailing on a map that did not grow instead ends the
                // archive short on any stretch of already-loaded history.
                if (!landed) break;
            }
        } finally {
            fillingRef.current = false;
        }
    }, []);

    // When the filter changes, re-arm pagination AND eagerly fetch the first
    // page of matching threads, then fill the viewport. A different filter is a
    // different cursor space, and the matches may sit entirely outside the
    // loaded window. The IntersectionObserver alone
    // strands the user there, since it only re-fires on a scroll transition.
    // `reloadAfterFilterChange` makes population deterministic, and
    // `loadWhileSentinelVisible` tops the viewport off.
    //
    // The guard is `filterChangedSinceLoad()`, a STORE fact, not a `useRef`
    // seeded at mount. This list renders under the default `all` status only,
    // and the Filter panel can change the selection under the other four. A
    // per-instance ref therefore answers the wrong question twice: it misses the
    // change, then seeds with the new value and suppresses the catch-up.
    // Comparing against what was actually fetched makes the mount run correct
    // rather than merely skipped.
    //
    // Keyed on the APPLIED selection. A run of ticks made while the filter
    // panel covers this list settles into ONE reload when the panel closes.
    useEffect(() => {
        // Before the first window lands there is nothing to bring up to date,
        // and the initial load stamps the selection it fetched against.
        if (!hydrated || !filterChangedSinceLoad()) return;
        void reloadAfterFilterChange().then(() => void loadWhileSentinelVisible());
    }, [hydrated, applied]);

    // Once per mount, bring the saved position into reach (see
    // `pageInToSavedScroll`). The drawer's scroll memory restores it the moment
    // the list is tall enough.
    // When it ends, hand back to the fill loop: a short list's first page
    // request may have lost the race to it.
    useEffect(() => {
        const list = containerRef.current?.closest<HTMLElement>('.thread-drawer-list');
        if (!hydrated || !list) return;
        void pageInToSavedScroll(list).then(() => void loadWhileSentinelVisible());
        return () => { activePageIn = null; };
    }, [hydrated]);

    const hasMore = threadHasMore.value;
    // Pagination feeds the Archive section, so a collapsed Archive has nothing
    // to load INTO (see `archivePaginationAllowed`). While it is shut there is
    // no sentinel at all, and expanding it mounts one.
    const paginationAllowed = archivePaginationAllowed(collapsed);
    // Delay-gated, like every other loader. The fill loop pages repeatedly, and
    // a page that lands inside SPINNER_DELAY_MS would flash this line once per
    // page. Read before the unhydrated return so the hook order holds.
    const showLoadingMore = useDelayedFlag(threadLoadingMore.value);

    // `paginationAllowed` is a DEPENDENCY, not just a guard, and that is the
    // whole point: expanding Archive must re-run this effect so its kick below
    // fires. An IntersectionObserver reports transitions only. The sentinel of
    // a list too short to scroll is already intersecting, so nothing else would
    // ever ask for a page. Without it the Archive stays at whatever the initial
    // window held, with no way to reach the rest: a list that does not overflow
    // cannot be scrolled either.
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
        // Kick once: a short list whose sentinel is already visible produces no
        // scroll transition, so without this the first viewport never fills.
        void loadWhileSentinelVisible();
        return () => observer.disconnect();
    }, [hydrated, hasMore, paginationAllowed, loadWhileSentinelVisible]);

    if (!hydrated) {
        return (
            <>
                <div ref={containerRef} />
                <div ref={portalRef} class="flip-portal" />
            </>
        );
    }

    return (
        <>
            <div ref={containerRef} class="thread-drawer-rows">
                {sections.map(s => {
                    if (s.threads.length === 0) return null;
                    const { title, Icon } = SECTION_META[s.name];
                    return (
                        <DrawerSection key={s.name} sectionKey={s.name} title={title} Icon={Icon} count={s.count} hasRunning={s.hasRunning}
                            action={s.name === 'current' && s.count > 0 ? <ArchiveAllButton /> : undefined}>
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
                                        hiddenArchivedCount={decorations.hiddenDirectChildCount.get(id) ?? 0}
                                        isArchivedRevealed={revealedArchivedSet.has(id)}
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
                {/* The next page's rows, drawn above the sentinel that asked for them. */}
                <LoadingFade showSkeleton={showLoadingMore} skeleton={<ListSkeletonOf count={2} row={() => <ThreadRowContent />} />}>
                    {null}
                </LoadingFade>
                {hasMore && paginationAllowed && (
                    <div ref={sentinelRef} class="thread-drawer-load-more" />
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

const REVEALED_ARCHIVED_KEY = 'lucidos-drawer-revealed-archived';

/** Per-parent "show archived children" state, keyed by parent thread id. A
 *  fully-archived branch renders hidden under a live parent by default
 *  (`computeHiddenArchivedThreads`); this is the opt-in to reveal it anyway,
 *  dimmed as before. Mirrors `collapsedFamilies`: localStorage-backed,
 *  per-device, not event-sourced. */
const revealedArchivedFamilies = signal(loadStringSet(REVEALED_ARCHIVED_KEY));

/** Reveal/hide a thread's archived children. The single source of truth for
 *  the per-row "N archived" toggle switch. */
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

/** Page in older threads until the saved list position fits, so a reload deep
 *  in Archive lands where the reader left it. Pagination otherwise waits for
 *  the sentinel, which a list still parked at the top never shows.
 *
 *  It stops with the restore it serves: when the scroll memory lands, gives up
 *  or the reader takes over. A landed page counts as progress even when it adds
 *  no rows, since the cursor still moved. */
async function pageInToSavedScroll(list: HTMLElement): Promise<void> {
    const token = {};
    activePageIn = token;
    try {
        // A child's effects run before the drawer's, so give the drawer a frame
        // to arm its restore on a view switch.
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
            // False also means another load owns the round trip, such as the
            // fill loop at boot. Wait that one out rather than stopping short.
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

/** Frames to wait for another thread-page load to finish. */
const PAGINATION_IDLE_MAX_FRAMES = 600;

/** Resolve true once no thread-page load is in flight, or false after the
 *  frame budget runs out. */
async function paginationIdle(): Promise<boolean> {
    for (let frames = 0; frames < PAGINATION_IDLE_MAX_FRAMES; frames++) {
        if (!threadLoadingMore.value) return true;
        await nextFrame();
    }
    return false;
}

// Skip pagination when Archive is collapsed. Collapsing shrinks the list and
// pops the sentinel into view. The fill loop would then pull the ENTIRE archive
// into memory while every row is hidden. Archive is the bottom section and the
// one absorbing paginated older threads.
//
// So this decides whether the load-more sentinel EXISTS, and expanding the
// section is what mounts one and re-arms the fill loop. A guard read only
// inside the loop cannot do that: the loop runs when an observer fires, and an
// expand fires nothing.
//
// There is NO filter-active bypass. The count badge reads the
// server-sourced `archiveThreadCount` (see `refreshArchivedCount`), so a filter
// whose matches are all archived shows its true count while collapsed.
// Expanding the section makes the fill loop load the matches, and
// `reloadAfterFilterChange` fetches the first page on every filter change. So
// selecting a facet is never stranded on "No threads".
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

function DrawerSection({ sectionKey, title, Icon, count, hasRunning, action, children }: { sectionKey: string; title: string; Icon?: ComponentType<{ size?: string }>; count: number; hasRunning?: boolean; action?: ComponentChildren; children: ComponentChildren }) {
    const collapsed = collapsedSections.value.has(sectionKey);
    return (
        <div class="drawer-section">
            <DrawerSectionTitle
                sectionKey={sectionKey} title={title} Icon={Icon}
                count={count} hasRunning={hasRunning} collapsed={collapsed} action={action}
            />
            {!collapsed && children}
        </div>
    );
}

/** The clickable, keyboard-highlightable section header row. Split out from
 *  DrawerSection so the highlight subscription (`highlightedKey`) lives here:
 *  moving the ↑/↓ highlight among threads then re-renders only the (≤3) section
 *  headers, not each section's whole subtree. */
function DrawerSectionTitle({ sectionKey, title, Icon, count, hasRunning, collapsed, action }: { sectionKey: string; title: string; Icon?: ComponentType<{ size?: string }>; count: number; hasRunning?: boolean; collapsed: boolean; action?: ComponentChildren }) {
    const highlighted = highlightedKey.value === sectionNavKey(sectionKey);
    return (
        <div class={`list-section-title list-section-title-collapsible${collapsed ? ' collapsed' : ''}${highlighted ? ' list-section-title-highlighted' : ''}`}
             id={navKeyDomId(sectionNavKey(sectionKey))}
             data-flip-id={sectionNavKey(sectionKey)}
             onClick={() => toggleSectionCollapse(sectionKey)}
             role="treeitem"
             aria-selected={highlighted}
             aria-expanded={!collapsed}>
            <SectionHeaderContent Icon={Icon} title={title} count={count} running={hasRunning} />
            {action && <span class="list-section-actions drawer-section-actions">{action}</span>}
        </div>
    );
}

/** The Current header's Archive all (ADR 0349). Its click is its own: it
 *  never also collapses the section underneath it. */
function ArchiveAllButton() {
    return (
        <button type="button" class="drawer-archive-all"
                onClick={(e) => { e.stopPropagation(); void handleArchiveAll(); }}>
            Archive all
        </button>
    );
}

/** The row's buttons at the right end of the title's first line. The outer
 *  span is one title line tall. Inside it, the inner inline box centers the
 *  buttons on the title's capital letters, as the status mark does. */
function RowActions({ children }: { children: ComponentChildren }) {
    return <span class="thread-row-actions-line"><span class="thread-row-actions">{children}</span></span>;
}

/** The repo / app / trigger name chip. A long name WRAPS inside the chip's CSS
 *  `max-width` (`.thread-row-context` in drawer.css). A constrained box sits at
 *  its constraint, not at its widest line, so a wrapped name leaves dead space.
 *  CSS cannot shrink a box to the longest line of *wrapped* text. So measure
 *  the rendered line boxes and pin the chip to its widest line. A single-line
 *  name needs none of it: fit-content already hugs it.
 *
 *  Four things keep the pin robust. It may only SHRINK below max-width.
 *
 *  1. Measure at the chip's own `max-width`, not its live width. The live one
 *     is transient while the drawer animates open or a divider is dragged.
 *  2. Divide the measured widths by the element's own scale factor.
 *     `getClientRects` returns transform-scaled geometry, so a FLIP row
 *     animation would otherwise pin a hair too narrow.
 *  3. Pin in `rem`, so the width tracks the text across a UI-scale change. A px
 *     pin goes stale until a reload re-measures.
 *  4. If the pin raised the wrapped line count, drop it: dead space beats a
 *     broken word. Line COUNT is transform-invariant. */
function ContextChip({ name }: { name: string }) {
    const ref = useRef<HTMLSpanElement>(null);
    useLayoutEffect(() => {
        const el = ref.current;
        if (!el) return;
        const cs = getComputedStyle(el);
        // (1) Fall back to the natural width only when max-width is not a
        // length, such as an override to `none`.
        const maxW = parseFloat(cs.maxWidth);
        el.style.width = Number.isFinite(maxW) && maxW > 0 ? `${maxW}px` : '';
        const range = document.createRange();
        range.selectNodeContents(el);
        const rects = range.getClientRects();
        const naturalLines = rects.length;
        if (naturalLines <= 1) { el.style.width = ''; return; } // hug via fit-content
        // (2) Un-scale by visual ÷ layout width, recovering the true line widths
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
    // A coding draft has not bound a backend. It spawns with THIS draft's
    // resolved backend at send time (see `sendCompose`). So the tag must read
    // that draft's own pick, never a global another draft changed. A plain chat
    // draft is a Lucidos thread, so `formatThreadChannelLabel('chat')` gives it
    // the same tag started chat threads wear.
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
    // Same deal as a started row: tap focuses the draft, and a right-click or
    // a mobile hold opens its menu. No prefetch, a draft has no events.
    const gesture = useRowActionsGesture({
        enabled: true,
        onTap: () => focusThread(thread.meta.id),
    });
    // Mouse-only (tabIndex=-1): the drawer is a single tab stop, and the
    // keyboard reaches draft actions through the ⋯ menu shortcut.
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
                 // The tap's focusThread reveals the thread pane itself
                 // (revealThreadPane handles the mobile swipe + desktop
                 // pane-group focus). It rides the gesture handlers so a mobile
                 // hold opens the menu without also focusing the draft.
                 {...gesture.handlers}>
                <div class="thread-row-left">
                    <span class="thread-row-title-row">
                        <span class="thread-row-title-text">
                            <ThreadStatusIcon status="idle" />
                            <span class="thread-row-title">{threadDisplayTitle(thread)}</span>
                            <span class="draft-indicator">Draft</span>
                        </span>
                        {/* On mobile there is no ⋯, and an empty actions box
                            is still a flex item that opens the row's gap. So
                            the menu renders bare there, drawing nothing until
                            the hold opens its portal. */}
                        {gesture.hostOpener.trigger ? <RowActions>{draftMenu}</RowActions> : draftMenu}
                    </span>
                    {(createdLabel || modeLabel || contextName) && (
                        <span class="thread-row-meta">
                            {createdLabel && <span class="thread-row-created">{createdLabel}</span>}
                            {modeLabel && <span class="label message-channel-tag">{modeLabel}</span>}
                            {contextName && <ContextChip name={contextName} />}
                        </span>
                    )}
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
    /** FLIP-animation key. Every `ThreadRow` sets it; a search hit renders
     *  without one, so it carries no `data-flip-id`. */
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
    /** Precomputed status dot, derived by the caller via `visualStatusFor`
     *  from the snapshot `status`. Every drawer row's dot is built in one pass
     *  and stays in lockstep with the list, rather than being re-read per
     *  row. */
    visualStatus: VisualStatus;
    /** Repo / app / trigger name chip shown next to the channel tag. Undefined
     *  for plain chat: a Lucidos thread carries no channel tag and no context
     *  name, so the bare row is itself the signal. */
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
     *  "Show / Hide N sub-threads" footer link under the date. */
    collapsible?: boolean;
    /** Whether this family is currently collapsed (children hidden). */
    isCollapsed?: boolean;
    /** Toggle this family's collapse state. Excluded from the memo equality
     *  check (like `onClick`) — the closure changes per parent render but
     *  always closes over the same thread id. */
    onToggleFamily?: () => void;
    /** Direct children of this row that are fully-archived branches hidden by
     *  default (`hiddenDirectChildCount`). > 0 shows the "N archived" reveal
     *  toggle; absent or 0 shows nothing, even in a collapsible context. */
    hiddenArchivedCount?: number;
    /** Whether this row's hidden archived children are currently revealed. */
    isArchivedRevealed?: boolean;
    /** Toggle `isArchivedRevealed`. Excluded from the memo equality check,
     *  like `onToggleFamily`. */
    onToggleArchivedRevealed?: () => void;
    onClick: () => void;
}

/** The family toggle's visible text, which is also its accessible name. Both
 *  states name the count, so opening a family never hides how big it is. */
export function familyDisclosureLabel(totalChildren: number, isCollapsed: boolean): string {
    const count = `${totalChildren} sub-thread${totalChildren === 1 ? '' : 's'}`;
    return `${isCollapsed ? 'Show' : 'Hide'} ${count}`;
}

function ThreadRowContentImpl(props: Partial<ThreadRowContentProps>) {
    const sk = useSkeleton();
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

    // Shared by the disclosure button's click and keydown, so the collapse
    // logic lives in one place. `stopPropagation` keeps the row's `onClick`
    // from also firing, and on keydown keeps the drawer container's
    // Enter→selectHighlighted handler from stealing the keystroke.
    const toggleFamily = (e: Event) => {
        e.stopPropagation();
        props.onToggleFamily?.();
    };

    // Fires once per toggle, on the input's own `onChange` — the click that
    // triggers it is stopped separately, on the wrapping label (see below).
    const toggleArchived = (e: Event) => {
        e.stopPropagation();
        props.onToggleArchivedRevealed?.();
    };

    // Every thread carries a channel tag. The guard stays so an empty label, or
    // an unknown channel, never paints an empty bordered chip.
    const channelLabel = props.channel ? formatThreadChannelLabel(props.channel, props.codingAgent) : null;
    const hasChips = sk || !!channelLabel || !!props.contextName;
    const chips = hasChips && (
        <>
            {sk ? (
                <SkBlock w="4.5rem" h="1.15rem" round />
            ) : channelLabel ? (
                <span
                    class={`label message-channel-tag${props.channel === 'error_unknown_channel' ? ' channel-error' : ''}`}
                >{channelLabel}</span>
            ) : null}
            {!sk && props.contextName && <ContextChip name={props.contextName} />}
        </>
    );

    // Tap opens the thread. A desktop right-click opens the ⋯ menu at the
    // pointer. On mobile a hold opens it instead, and no ⋯ is rendered.
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
                 // Tree-item semantics for the single-focus model: the drawer
                 // container (role="tree", tabindex=0) points its
                 // `aria-activedescendant` at the highlighted row's id; the row
                 // itself never takes DOM focus. Omitted on skeleton rows (no id).
                 role={props.id ? 'treeitem' : undefined}
                 aria-selected={props.id ? (props.isHighlighted ?? false) : undefined}
                 // Prefetch on press-in. pointerdown fires before the tap's
                 // click, so the event load starts earlier and content is often
                 // ready by the time focusThread switches the view.
                 // `loadThreadEvents` is idempotent, so a canceled press just
                 // warms the cache and the tap never double-fetches.
                 //
                 // Tap, prefetch and the mobile long press all come from
                 // `useRowActionsGesture`, which owns the composition: the
                 // gesture has to swallow its own paired click, so the row
                 // cannot keep a separate `onClick`.
                 {...gesture.handlers}>
                <div class="thread-row-left">
                    <span class="thread-row-title-row">
                        <span class="thread-row-title-text">
                            <ThreadStatusIcon status={sk ? null : (props.visualStatus ?? null)} />
                            <SkText class="thread-row-title" w="11rem">{props.title}</SkText>
                            {props.hasDraft && <span class="draft-indicator" data-tooltip="Has unsent draft">Draft</span>}
                        </span>
                        {!sk && props.id && (
                            <RowActions>
                                {/* Mouse-only (tabIndex=-1): the drawer is a single tab
                                    stop. The keyboard reaches every row action through
                                    the ⋯ menu via the "Open thread actions" shortcut. */}
                                <PinThreadButton threadId={props.id} saved={props.isSaved ?? false} stopPropagation extraClass="thread-row-action" tabIndex={-1} />
                                <ThreadOverflowMenu threadId={props.id} title={props.title ?? ''} stopPropagation extraClass="thread-row-action" tabIndex={-1} hostOpener={gesture.hostOpener} />
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
                                    // Mouse-only: the drawer is a single tab stop, so per-row
                                    // controls leave the Tab order. Keyboard collapses/expands
                                    // the family via ←/→ on the highlighted row instead.
                                    tabIndex={-1}
                                    onClick={toggleFamily}
                                    onKeyDown={(e) => {
                                        // The drawer container's keydown handler intercepts
                                        // Enter at the bubble phase and preventDefaults it,
                                        // cancelling this button's Enter→click activation.
                                        // So Enter and Space are handled here instead.
                                        // `preventDefault` blocks Space page-scroll and the
                                        // native synthetic click, firing the toggle exactly
                                        // once, and `toggleFamily`'s `stopPropagation` keeps
                                        // the drawer handler off the keystroke.
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
                                // One label around the switch and its text, so a tap on
                                // either flips it. The label's own click bubbles, and the
                                // browser ALSO forwards a second, separate click onto the
                                // input, so the label only stops propagation here. The
                                // actual flip lives in onChange below, which fires once.
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

/** Skip the render when nothing the row paints has changed. Drawer flushes
 *  fire on every SSE event in the workspace. Without memo, all 100+ visible
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
    && prev.hiddenArchivedCount === next.hiddenArchivedCount
    && prev.isArchivedRevealed === next.isArchivedRevealed
);

export function ThreadRow({ threadId, status, depth = 0, isLiftedParent, isResponsibleChild, isArchivedSubThread, hiddenArchivedCount = 0, isArchivedRevealed, enableFamilyToggle, onOpen = focusThread }: {
    threadId: string;
    status: ThreadStatus;
    depth?: number;
    isLiftedParent?: boolean;
    isResponsibleChild?: boolean;
    isArchivedSubThread?: boolean;
    /** This thread's direct children hidden by default as fully-archived
     *  branches (`FamilyDecorations.hiddenDirectChildCount`). Only the nested
     *  ThreadList passes it — search / drafts render flat lists with no
     *  family tree to hide anything from. */
    hiddenArchivedCount?: number;
    isArchivedRevealed?: boolean;
    /** Render the bottom-left disclosure chevron when this thread has
     *  sub-threads. Only the nested ThreadList sets it — search / drafts render
     *  flat lists where collapsing nothing visible would be a no-op. */
    enableFamilyToggle?: boolean;
    /** What a tap on the row does. Needs attention lands on the waiting event
     *  (`openAttentionThread`); every other list opens the thread as it was. */
    onOpen?: (threadId: string) => void;
}) {
    // Signal reads stay here so each row's subscription set is narrow. The row
    // re-renders on several signals, but the memo on ThreadRowContent below
    // short-circuits when none of the primitives it derives changed. That turns
    // a per-flush N-row VDOM storm into one render per moved row.
    const thread = threadMap.value.get(threadId);
    if (!thread) return null;
    const { meta } = thread;
    // Status dot from the snapshot `status` prop, NOT a live re-read of
    // `meta.status`. ThreadRow re-renders on `focusedThreadId` changes, which do
    // NOT flush `threadMap`. A live `meta.status` read here repaints the focused
    // row's dot to a value diverging from the rest of the list until the next
    // flush. Feeding the snapshot through the shared `visualStatusFor`
    // formula keeps every row's dot in lockstep.
    const visualStatus = visualStatusFor(status, meta);
    const isFocused = focusedThreadId.value === meta.id;
    const isHighlighted = highlightedKey.value === meta.id;
    const hasDraft = threadHasUnsentDraft(thread);
    // Archiving a child never decrements totalChildrenCount. Subtract the ones
    // hidden by default, so the chevron names only what actually renders.
    const liveChildrenCount = meta.totalChildrenCount - hiddenArchivedCount;
    const hasLiveFamily = !!enableFamilyToggle && liveChildrenCount > 0;
    const isCollapsed = hasLiveFamily && collapsedFamilies.value.has(meta.id);

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
            totalChildren={liveChildrenCount}
            needsReview={meta.section === 'inbox' && status !== 'running'}
            hasDraft={hasDraft}
            isSaved={meta.saved}
            isFocused={isFocused}
            isHighlighted={isHighlighted}
            isLiftedParent={isLiftedParent}
            isResponsibleChild={isResponsibleChild}
            isArchivedSubThread={isArchivedSubThread}
            collapsible={hasLiveFamily}
            isCollapsed={isCollapsed}
            onToggleFamily={() => toggleFamilyCollapse(meta.id)}
            hiddenArchivedCount={enableFamilyToggle ? hiddenArchivedCount : 0}
            isArchivedRevealed={isArchivedRevealed}
            onToggleArchivedRevealed={() => toggleArchivedRevealed(meta.id)}
            onClick={() => onOpen(meta.id)}
        />
    );
}

/** One-tap shortcut back to the unfiltered "All statuses" view, offered by every
 *  status-filter view (Drafts / Needs attention / Review / Running) in both
 *  states: under the "nothing here" message when the filter is empty, and under
 *  the last row when it is not. Either way the user has reached the end of this
 *  status. So the way out sits where they are already looking, rather than back
 *  up in the filter control. The all-statuses and search views do not use it:
 *  "All statuses" IS the destination, and an exhausted search wants a different
 *  query, not a filter reset. */
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

/** Single-section view of every thread carrying an unsent draft. It bypasses
 *  the channel / trigger / repo filters and the three lifecycle sections: a user
 *  toggling the drafts icon wants every draft. Drafts come from threads already
 *  in `threadMap`, and older draft-bearing threads outside the pagination
 *  window are not loaded on demand. That is acceptable, since drafts are by
 *  definition recently touched and ride at the top of the window. */
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
                <SectionHeaderContent Icon={DraftsIcon} title="Drafts" count={drafts.length} />
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
 *  channel/trigger/repo filters and the three lifecycle sections so the user sees
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
                <SectionHeaderContent Icon={AttentionIcon} title="Needs attention" count={threads.length} />
            </div>
            {threads.map(t => <ThreadRow key={t.meta.id} threadId={t.meta.id} status={effectiveThreadStatus(t)} onOpen={openAttentionThread} />)}
            <FilteredViewFooter />
        </div>
    );
}

/** Single-section view of every Current/Saved thread carrying a change ready to
 *  apply (see `threadInReview`). Mirrors `AttentionList`: bypasses the
 *  channel/trigger/repo filters and the three lifecycle sections so the user sees
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
                <SectionHeaderContent title="Review" count={threads.length} />
            </div>
            {threads.map(t => <ThreadRow key={t.meta.id} threadId={t.meta.id} status={effectiveThreadStatus(t)} />)}
            <FilteredViewFooter />
        </div>
    );
}

/** Single-section view of every Current/Saved thread actively working on a
 *  response (see `threadIsRunning`). Mirrors `AttentionList`/`ReviewList`:
 *  bypasses the channel/trigger/repo filters and the three lifecycle sections so
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
                <SectionHeaderContent Icon={RunningIcon} title="Running" count={threads.length} running />
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
        // The reason, not a bare "Search failed": this is the only report the
        // user gets, and a dropped `loadable.error` is a swallowed error.
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
                            <SectionHeaderContent title="Results" />
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
    const status: ThreadStatus = liveThread ? effectiveThreadStatus(liveThread) : result.status;
    // Same formula as the live rows, fed the `status` snapshot above. A search
    // hit not yet hydrated into threadMap resolves on that status alone.
    const visualStatus = visualStatusFor(status, liveThread?.meta);
    const section = liveThread?.meta.section ?? result.section;
    const isFocused = focusedThreadId.value === result.thread_id;
    const isHighlighted = highlightedKey.value === result.thread_id;
    // The context name works whether or not the hit is hydrated into
    // `threadMap`. ThreadMeta is structurally a ThreadContextFields, and the
    // result's snake-case fields map onto the same shape.
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
