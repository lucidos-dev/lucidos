import { computed } from '@preact/signals';
import { effectiveThreadStatus, getThreadDisplaySection, threadMap, threadNeedsAttention, threadInReview, threadIsRunning, type ThreadChannel } from '../../store/store';
import { byRecent, byCreated, recencyKey, reviewTier, isExcludedFromSections } from '../../store/thread-events';
import type { ThreadState } from '../../store/thread-events';
import type { DisplaySection } from '../../generated/thread-lifecycle';
import type { ThreadStatus } from '../../store/thread-events';
import { draftPresentThreadIds } from '../../store/composeDrafts';
import { threadPassesChannelFilter } from '../../store/threadFilter';

// Pure family-graph / categorization logic for the thread drawer. Extracted
// from ThreadDrawer.tsx (re-exported there); imported by drawer/*.test.ts.

const SECTION_PRIORITY: Record<DisplaySection, number> = {
    current: 0,
    saved: 1,
    archive: 2,
};

/** Walk parentThreadId up to the topmost ancestor present in `byId`. Stops at
 *  the first ancestor not in the map (paginated out / filtered) and returns
 *  that ancestor's child as the root — so an orphan child is its own family
 *  root. On a parentThreadId cycle (data corruption), returns the
 *  lexicographically-smallest id in the cycle so every member converges on the
 *  same root — without that, two cycle members would get different roots and
 *  break the "every family member maps to the same record" invariant. */
function rootAncestorId(start: ThreadState, byId: ReadonlyMap<string, ThreadState>): string {
    let current = start;
    const visited = new Set<string>([current.meta.id]);
    while (current.meta.parentThreadId) {
        const parent = byId.get(current.meta.parentThreadId);
        if (!parent) return current.meta.id;
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

/** Pre-computed family graph shared by `categorizeThreads` and
 *  `computeFamilyKeys` — the drawer renders both on every tick, so a single
 *  pass over the input avoids 3× parent-chain walks per render. */
export type FamilyGraph = {
    byId: ReadonlyMap<string, ThreadState>;
    /** Excluded threads (composing/discarded) are absent — callers iterate the
     *  input list and skip via `isExcludedFromSections`, or look up here and
     *  treat `undefined` as "skip". */
    rootByThread: ReadonlyMap<string, string>;
};

export function computeFamilyGraph(threads: ThreadState[]): FamilyGraph {
    const byId = new Map<string, ThreadState>();
    for (const t of threads) byId.set(t.meta.id, t);
    const rootByThread = new Map<string, string>();
    for (const t of threads) {
        if (isExcludedFromSections(t)) continue;
        rootByThread.set(t.meta.id, rootAncestorId(t, byId));
    }
    return { byId, rootByThread };
}

/** Apply the filter at top-thread scope: a family is included iff its
 *  top-thread (the family root) passes `passesFilter`; every descendant
 *  inherits visibility from the root regardless of its own channel. This is
 *  what stops the family-row count from diverging from the rendered children
 *  — a per-thread filter would drop a coding-agent sub-thread under a chat top-thread
 *  while the parent still claimed "1/1 sub-thread done" via
 *  meta.totalChildrenCount. Orphans (parent paginated out, so the rootByThread
 *  walk lands on the thread itself) are judged as their own top-thread.
 *
 *  `matchAnyMember` flips the gate from root-only to any-member: a family is
 *  included if ANY of its threads passes the filter. This is required when a
 *  trigger/repo/app SUB-selection is active, because the repo/app/trigger is a
 *  property of a specific coding-agent thread, not of its (often chat/trigger)
 *  top-thread — so a coding-agent thread in the selected repo/app must surface, with its
 *  parent kept for context, even when the root's channel is filtered out. The
 *  whole family still renders (matched + sibling threads) so the family-row
 *  count stays honest. For a plain channel filter (no sub-selection) the
 *  root-only gate stands: channel-only filtering deliberately does NOT surface
 *  a coding-agent sub-thread buried under a chat top-thread (that's search's job). */
export function filterByTopThread(
    threads: ThreadState[],
    graph: FamilyGraph,
    passesFilter: (top: ThreadState) => boolean,
    matchAnyMember = false,
): ThreadState[] {
    const passingRoots = new Set<string>();
    for (const t of threads) {
        const rootId = graph.rootByThread.get(t.meta.id) ?? t.meta.id;
        // Root-only mode considers just the family root; any-member mode lets
        // any matching thread mark its whole family as passing.
        if (!matchAnyMember && rootId !== t.meta.id) continue;
        if (passesFilter(t)) passingRoots.add(rootId);
    }
    return threads.filter(t => {
        const rootId = graph.rootByThread.get(t.meta.id) ?? t.meta.id;
        return passingRoots.has(rootId);
    });
}

/** Per-row data for the family-routing cues (see *Lifted family* and *Archived
 *  sub-thread cue* in `docs/glossary.md`): which section each row routed to,
 *  which family roots are lifted, and which sub-threads are archived while
 *  rendering outside Archive. */
export type FamilyDecorations = {
    routedByThread: ReadonlyMap<string, DisplaySection>;
    liftedRoots: ReadonlySet<string>;
    /** Sub-threads (non-root family members) whose OWN natural section is
     *  Archive while their family routed to a live section (Current / Pinned):
     *  the archived child still listed under a parent that has live work. The
     *  drawer renders these disabled so a thread the user already put away
     *  can't read as pending work. The family ROOT is deliberately excluded:
     *  an archived root in a live section is exactly the lifted-parent case,
     *  which owns that row's cue already (`liftedRoots`). */
    archivedSubThreads: ReadonlySet<string>;
};

/** Map of family root id → the display section the family renders in. Default
 *  rule: the highest-priority section reached by any family member (current >
 *  saved > archive). Override: a saved root pins the family to
 *  Saved regardless of descendant status — save is an explicit user pin, same
 *  rule as the leaf-thread isSaved check in `displaySection`. Drives both
 *  routing (categorizeThreads picks this section for every member of the
 *  family) and decorations (the lifted-parent cue fires when the root's
 *  natural section differs from this routed one). Pass once per render to
 *  avoid two passes over the threads list. */
export type FamilySectionMap = ReadonlyMap<string, DisplaySection>;

export function computeFamilySections(threads: ThreadState[], graph: FamilyGraph): Map<string, DisplaySection> {
    const familySection = new Map<string, DisplaySection>();
    for (const t of threads) {
        if (isExcludedFromSections(t)) continue;
        const sec = getThreadDisplaySection(t);
        const root = graph.rootByThread.get(t.meta.id);
        if (root === undefined) continue;
        const cur = familySection.get(root);
        if (cur === undefined || SECTION_PRIORITY[sec] < SECTION_PRIORITY[cur]) {
            familySection.set(root, sec);
        }
    }
    // A saved root pins the whole family to Saved — save is an explicit user
    // pin that overrides automatic categorization, matching the leaf-thread
    // rule where isSaved beats running/review/etc. in displaySection.
    for (const root of familySection.keys()) {
        if (graph.byId.get(root)?.meta.saved) {
            familySection.set(root, 'saved');
        }
    }
    return familySection;
}

/** Pure: derive the lifted-parent + responsible-child + archived-sub-thread
 *  decoration data the drawer renders on top of family routing. Pass a
 *  precomputed `familySections` to share the routing pass with
 *  `categorizeThreads`. */
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
        if (isExcludedFromSections(t)) continue;
        const root = graph.rootByThread.get(t.meta.id);
        if (root === undefined) continue;
        const routed = familySection.get(root);
        if (routed === undefined) continue;
        routedByThread.set(t.meta.id, routed);
        // Archived sub-thread rendered in a live section. This reads the
        // thread's NATURAL section, so a thread STORED as archived but running
        // (or holding changes, or with a waiting descendant) resolves to
        // `current` here and is deliberately not dimmed: it has live work to
        // show. Roots are excluded because `liftedRoots` already owns them.
        // The two cheap tests come first so the section lookup is skipped for
        // every root and for every member of an Archive-routed family.
        if (t.meta.id !== root && routed !== 'archive' && getThreadDisplaySection(t) === 'archive') {
            archivedSubThreads.add(t.meta.id);
        }
        const rootThread = graph.byId.get(root);
        if (rootThread && getThreadDisplaySection(rootThread) !== routed) {
            liftedRoots.add(root);
        }
    }
    return { routedByThread, liftedRoots, archivedSubThreads };
}

/** True if any ancestor of `threadId` (walking `parentThreadId` via `graph`)
 *  is present in `collapsed`. Used as the filter pass after `nestByParent` to
 *  hide the descendants of a collapsed family without rebuilding the nesting.
 *  Cycle-safe (tracks visited ids); returns false for unknown ids so a stale
 *  reference between batches can't blow up the render. */
export function hasCollapsedAncestor(
    threadId: string,
    collapsed: ReadonlySet<string>,
    graph: FamilyGraph,
): boolean {
    const visited = new Set<string>([threadId]);
    let current = graph.byId.get(threadId);
    if (!current) return false;
    let parentId = current.meta.parentThreadId;
    while (parentId) {
        if (collapsed.has(parentId)) return true;
        if (visited.has(parentId)) return false;
        visited.add(parentId);
        const parent = graph.byId.get(parentId);
        if (!parent) return false;
        parentId = parent.meta.parentThreadId;
    }
    return false;
}

/** Group threads into drawer sections. A thread's section is determined by its
 *  *family* — the thread plus every transitive descendant reachable via
 *  parentThreadId — taking the highest-priority section reached by any member
 *  (current > saved > archive). This keeps a family rendered together
 *  so nestByParent can put children directly under their parent even when the
 *  child intrinsically belongs to a different section. Live work anywhere keeps the
 *  family together: a parent with one still-working coding-agent child and two completed
 *  siblings stays in Current as one unit instead of any member splitting off. One override beats the priority order: a saved
 *  root pins the family to Saved regardless of descendant status — save is
 *  an explicit user pin, same rule as for leaf threads. Composing and
 *  discarded threads stay excluded; the caller surfaces composing threads at
 *  the top of the Current section.
 *
 *  Pass an existing `graph` to skip the parent-walk pass when the caller has
 *  already built one (see `computeFamilyGraph`). */
export function categorizeThreads(
    threads: ThreadState[],
    graph?: FamilyGraph,
    familySections?: FamilySectionMap,
): ThreadSections {
    const familyGraph = graph ?? computeFamilyGraph(threads);
    const { rootByThread } = familyGraph;
    const familySection = familySections ?? computeFamilySections(threads, familyGraph);
    const out: ThreadSections = {
        current: [], saved: [], archive: [],
        statusMap: new Map(),
    };

    for (const t of threads) {
        if (isExcludedFromSections(t)) continue;
        const status = effectiveThreadStatus(t);
        out.statusMap.set(t.meta.id, status);
        const display = familySection.get(rootByThread.get(t.meta.id)!)!;
        switch (display) {
            case 'current': out.current.push(t); break;
            case 'saved': out.saved.push(t); break;
            case 'archive': out.archive.push(t); break;
        }
    }
    return out;
}

/** Result of `computeDrawerCategorization`: the section buckets plus the family
 *  graph and the lifted-parent decorations the drawer renders on top of them. */
export type DrawerCategorization = {
    categorized: ThreadSections;
    familyGraph: FamilyGraph;
    decorations: FamilyDecorations;
};

/** The drawer's categorization pass as one pure call: build the family graph,
 *  filter to passing top-threads, route families to sections, and sort. Extracted
 *  from `ThreadList` so the component can MEMOIZE it on its real inputs (the
 *  loaded threads + the channel filter + the trigger/repo/app selections) instead
 *  of re-running this whole O(threads) multi-pass on every render — including
 *  renders triggered by signals it does NOT depend on (a section collapse, the
 *  archive-count refresh, a pagination loading-flip). Output is identical to the
 *  inlined sequence it replaced (pinned by `drawer-categorization-memo.test.ts`).
 *  Collapse-filtering and nesting stay in the component — they key on the
 *  collapsed-family set and are cheap over the already-categorized rendered set. */
export function computeDrawerCategorization(
    threads: ThreadState[],
    channelFilter: ReadonlySet<ThreadChannel>,
    triggerSelection: ReadonlySet<string>,
    repoSelection: ReadonlySet<string>,
    appSelection: ReadonlySet<string>,
): DrawerCategorization {
    // Filter at top-thread scope so a family is shown iff its top-thread passes —
    // a per-thread filter would drop sub-threads while the parent row still
    // advertised them via meta.totalChildrenCount. Share the one graph across
    // routing, sort keys, decorations, and the collapse filter; rebuilding it for
    // the filtered subset would double the per-render parent-walk cost.
    //
    // When a trigger/repo/app sub-selection is active, switch to any-member
    // matching: the repo/app/trigger is a property of a specific coding-agent
    // thread, not its (often chat/trigger) top-thread, so a coding-agent thread in
    // the selected repo/app must surface with its family even when the root's
    // channel is filtered out.
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
    // Current + Archive sort by creation time (newest first) — a stable order that
    // doesn't reshuffle as agents churn or threads gain a CTA, and Archive's
    // createdAt sort matches both the date each row displays and the axis
    // `loadOlderThreads` pages by. Saved sorts by the family's freshest user
    // action. (See `sortDrawerSections`.)
    sortDrawerSections(categorized, familyKeys);
    return { categorized, familyGraph, decorations };
}

/** Per-thread sort key computed over the whole family — the parent inherits the
 *  freshest user action from any descendant, so the family rises together. Every
 *  member of one family resolves to the same record. */
export type FamilyKeys = {
    /** Max last-user-action across the family (see `recencyKey`). Drives the
     *  Saved section's recency — the family rises to its freshest USER touch,
     *  not the agent's last churn. (Current and Archive sort by creation time
     *  instead, so they don't consult this.) */
    recentKey: string;
};

/** The Current-section rows in the exact order the thread drawer renders them:
 *  creation-time sorted (newest first, see `byCreated`), then nested so each
 *  sub-thread follows its parent (see `nestByParent`). The post-archive focus
 *  picker walks THIS order so "next in queue" is the next *visible row*, in the
 *  same order the user sees — sharing the comparator with the drawer's Current
 *  sort keeps the two from drifting (the divergence between drawer order and
 *  focus order is what let archiving a parent jump focus into an unrelated
 *  family's sub-thread). `visible` must already be top-thread filtered; `graph`
 *  is built over the full thread set so the parent walks resolve. */
export function orderedCurrentForReview(
    visible: ThreadState[],
    graph: FamilyGraph,
): ThreadState[] {
    const current = categorizeThreads(visible, graph).current;
    current.sort(byCreated);
    return nestByParent(current).map(n => n.thread);
}

/** Apply the drawer's per-section display sort in place.
 *
 *  Current and Archive order by creation time (newest first, see `byCreated`):
 *  a stable order that doesn't reshuffle as a family's recency shifts, and —
 *  crucially for Archive — matches the `createdAt` date each row displays.
 *  Archive is also PAGED by `createdAt` (see `loadOlderThreads`), so display and
 *  pagination share one axis and the section is gap-free as it scrolls.
 *
 *  Saved orders by the family's freshest user action (`byFamilyRecent` over
 *  `familyKeys`) — an explicit-pin section, fully loaded, where bubbling the
 *  family to its latest USER touch is wanted. `familyKeys` must cover every
 *  thread in `sections.saved` (built by `computeFamilyKeys` over the full set). */
export function sortDrawerSections(
    sections: ThreadSections,
    familyKeys: ReadonlyMap<string, FamilyKeys>,
): void {
    const byFamilyRecent = (a: ThreadState, b: ThreadState) =>
        familyKeys.get(b.meta.id)!.recentKey.localeCompare(familyKeys.get(a.meta.id)!.recentKey);
    sections.current.sort(byCreated);
    sections.saved.sort(byFamilyRecent);
    sections.archive.sort(byCreated);
}

/** Compute family-aware recency keys for every non-composing/non-discarded
 *  thread. Returns a Map keyed by thread id; every member of the same family
 *  maps to the same record (the family's freshest `recentKey`), so the Saved
 *  section sorts families as a unit. Accepts an optional pre-built graph (see
 *  `computeFamilyGraph`) to share the parent walk with `categorizeThreads`. */
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

    // Iterate the input threads (not rootByThread) so callers can pass a
    // graph built over a larger set than the threads being keyed — the
    // drawer reuses a single full-thread graph for routing/decoration/keys
    // to avoid a second parent-walk pass per render.
    const out = new Map<string, FamilyKeys>();
    for (const t of threads) {
        const root = rootByThread.get(t.meta.id);
        if (root === undefined) continue;
        const keys = perRoot.get(root);
        if (keys !== undefined) out.set(t.meta.id, keys);
    }
    return out;
}

/** Composing-draft rows, surfaced at the top of the Current section. Empty
 *  composing rows are filtered out — POST/DELETE
 *  races, SSE skeletons from a peer's ThreadStarted with no follow-up
 *  compose change, and failed local discards leave server-side rows whose
 *  only UI surface would be a placeholder "Empty draft" title. */
export function composingThreads(threads: ReadonlyMap<string, ThreadState>): ThreadState[] {
    const out: ThreadState[] = [];
    for (const t of threads.values()) {
        if (t.meta.state === 'composing' && threadHasUnsentDraft(t)) out.push(t);
    }
    out.sort(byRecent);
    return out;
}

export type NestedThread = { thread: ThreadState; depth: number };

/** CSS-variable style for a row wrapper. Inherits down to `.thread-row`, which
 *  uses it to widen its left padding and to inset the nested-row clip-path one
 *  step per level. (The status icon is pinned to a fixed left column and does
 *  NOT shift with depth.) Typed as a string-keyed map so TypeScript accepts the
 *  custom property — CSSProperties doesn't model `--*` keys. */
export function depthStyle(depth: number): { [key: string]: string } {
    return { '--thread-depth': String(depth) };
}

/** Reorder a sorted thread list so each child appears immediately after its
 *  parent, indented one level deeper. Children are nested only when their
 *  parent is present in the same input list — orphans (parent paginated out,
 *  filtered away, or in a different section) render at root level. Roots and
 *  siblings keep the input's relative order, so the section sort still drives
 *  the visible top-level sequence. */
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

/** Needs-attention view rows: every Current/Saved thread where the agent is
 *  stuck waiting on the user — awaiting answer/permission or a failed turn (see
 *  `threadNeedsAttention`). Mirrors `draftThreads`: bypasses the
 *  channel/trigger/repo filters and the lifecycle section grouping. Ordered by
 *  `reviewTier` first — a User Q / permission request (tier 0, the agent is
 *  stalled until the user answers) floats above a failed turn (tier 1) — then
 *  most-recent-first within each tier. The tier is read with
 *  `effectiveThreadStatus` to match the `threadNeedsAttention` predicate that
 *  selected these rows. Shares that predicate with the selector's badge count
 *  (`attentionThreadCount`) so the two can never disagree. */
export function attentionThreads(threads: ReadonlyMap<string, ThreadState>): ThreadState[] {
    const out: ThreadState[] = [];
    for (const t of threads.values()) {
        if (threadNeedsAttention(t)) out.push(t);
    }
    out.sort((a, b) => {
        const ta = reviewTier(a, effectiveThreadStatus(a));
        const tb = reviewTier(b, effectiveThreadStatus(b));
        if (ta !== tb) return ta - tb;
        return byRecent(a, b);
    });
    return out;
}

/** Review view rows: every Current/Saved thread carrying a change ready to apply
 *  (see `threadInReview`). Mirrors `draftThreads`/`attentionThreads`: bypasses
 *  the channel/trigger/repo filters and the lifecycle section grouping. Single
 *  tier (all proposed), so ordered purely most-recent-first. Shares its
 *  predicate with the selector's badge count (`reviewThreadCount`) so the two
 *  can never disagree. */
export function reviewThreads(threads: ReadonlyMap<string, ThreadState>): ThreadState[] {
    const out: ThreadState[] = [];
    for (const t of threads.values()) {
        if (threadInReview(t)) out.push(t);
    }
    out.sort(byRecent);
    return out;
}

/** Running view rows: every Current/Saved thread actively working on a response
 *  (see `threadIsRunning`). Mirrors `attentionThreads`/`reviewThreads`: bypasses
 *  the channel/trigger/repo filters and the lifecycle section grouping. Single
 *  tier (all running), so ordered purely most-recent-first. Shares its predicate
 *  with the selector's badge count (`runningThreadCount`) so the two can never
 *  disagree. */
export function runningThreads(threads: ReadonlyMap<string, ThreadState>): ThreadState[] {
    const out: ThreadState[] = [];
    for (const t of threads.values()) {
        if (threadIsRunning(t)) out.push(t);
    }
    out.sort(byRecent);
    return out;
}

export type ThreadSections = {
    current: ThreadState[];
    saved: ThreadState[];
    archive: ThreadState[];
    statusMap: Map<string, ThreadStatus>;
};

export function threadHasUnsentDraft(thread: ThreadState | undefined): boolean {
    if (!thread) return false;
    return draftPresentThreadIds.value.has(thread.meta.id);
}

/** Number of rows the drafts view would render — i.e. non-discarded threads
 *  carrying an unsent draft. Same predicate as `draftThreads`, but avoids
 *  building/sorting an array. Drives the drafts-icon badge in the thread
 *  headers. Memoized; recomputes only when `threadMap` or
 *  `draftPresentThreadIds` change. The empty-presence-set fast path keeps the
 *  common (no-drafts) case O(1). */
export const draftThreadCount = computed<number>(() => {
    const draftIds = draftPresentThreadIds.value;
    if (draftIds.size === 0) return 0;
    const threads = threadMap.value;
    let count = 0;
    for (const id of draftIds) {
        const t = threads.get(id);
        if (t && t.meta.state !== 'discarded') count += 1;
    }
    return count;
});
