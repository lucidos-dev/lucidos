import { createContext, type VNode } from 'preact';
import { useContext, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { effect, signal, type ReadonlySignal } from '@preact/signals';
import { planRowWindow, RowHeights, needsNewSpan, renderSpanFor, segmentsKey, type RowSegment, type Span } from './rowWindow';
import { getRemPx } from '../../utils/dom';
import { FLIP_GAP_END_ATTR } from '../../hooks/useFlipAnimation';

/** Every Folders section's measured row heights. Module-scoped, so a list
 *  remounted by a grouping switch or a drawer reopen starts measured. */
const heights = new RowHeights();

/** Each section's last measured top, in list content coordinates. A section
 *  that mounts again (an expand) plans from it before it can measure. */
const sectionTops = new Map<string, number>();

/** How long the list width must hold still before a window shrinks again. */
const SETTLE_MS = 150;
/** Commits a reveal waits for its row to mount before it gives up: the row
 *  is not listed. Counted in commits, not time, so a slow first mount on a
 *  phone does not cancel it. */
const REVEAL_MISS_LIMIT = 30;
/** The height assumed for an unmeasured row below the window, before any row
 *  has been measured. */
const FALLBACK_ROW_REM = 3.5;

export type RevealMode = 'nearest' | 'centre';

interface Reveal {
    id: string;
    mode: RevealMode;
    /** Runs once the row is on screen, for a reveal that keeps following it. */
    then?: (row: HTMLElement, list: HTMLElement) => void;
    /** Commits so far that ended without the row mounted. */
    misses: number;
}

/** The one reveal in flight. Module-scoped, because a reveal can start before
 *  the list it lands in has mounted (Show in Folders from Ongoing). */
let pendingReveal: Reveal | null = null;
let activeController: RowWindowController | null = null;

/** Scroll the drawer to thread row `id`, through the row window.
 *
 *  The row and the viewport it lands in render first. The scroll happens in
 *  the layout pass of the commit that mounted them, so no frame paints a gap.
 *  `nearest` scrolls only as far as needed, as `block: 'nearest'` does.
 *  `centre` approaches a far row from one viewport out. It then hands the row
 *  to `then`, which glides it to the centre. */
export function revealDrawerRow(id: string, mode: RevealMode, then?: Reveal['then']): void {
    pendingReveal = { id, mode, then, misses: 0 };
    activeController?.revealChanged();
}

/** Whether a windowed list is mounted. The Ongoing grouping and search are
 *  not windowed, so their rows scroll into view directly. */
export function rowWindowIsMounted(): boolean {
    return activeController !== null;
}

/** Re-plan the mounted list's window, for a caller whose change it cannot see
 *  (the drawer's scroll memory settling). */
export function replanDrawerRowWindow(): void {
    activeController?.replan();
}

function sameSpans(a: readonly Span[], b: readonly Span[]): boolean {
    return a.length === b.length && a.every((s, i) => s.top === b[i].top && s.bottom === b[i].bottom);
}

function rowSelector(id: string): string {
    return `[data-thread-nav="${CSS.escape(id)}"]`;
}

/** What a row's height depends on beyond its own content. */
function environmentOf(list: HTMLElement): string {
    return `${list.clientWidth}:${getRemPx()}`;
}

/** The list-wide half of the row window: which spans of the list's content
 *  every section must render, and when a section must re-plan. One per
 *  mounted Folders list. */
export class RowWindowController {
    /** List content ranges whose rows render: the viewport's span, plus a
     *  pending restore's and a reveal's. */
    readonly spans = signal<Span[]>([]);
    /** Bumped when every section must re-plan for a reason `spans` does not
     *  carry: a pin, a stale environment, or the environment settling. */
    readonly version = signal(0);
    /** Set while the drawer is closing. The window holds still then, so the
     *  shrinking width it animates through measures and re-plans nothing. */
    frozen = false;

    private list: HTMLElement | null = null;
    private main: Span | null = null;
    private revealSpan: Span | null = null;
    private unstableUntil = 0;
    private settleTimer: ReturnType<typeof setTimeout> | undefined;
    private detachList: (() => void) | null = null;

    constructor(
        private readonly restoreTarget: () => number | null,
        private readonly highlightedRow: ReadonlySignal<string | null>,
    ) {
        // Before the list is measured: its top, and a pending restore's place.
        const vh = window.innerHeight;
        const restore = restoreTarget();
        this.spans.value = [
            renderSpanFor({ top: 0, bottom: vh }),
            ...(restore === null ? [] : [renderSpanFor({ top: restore, bottom: restore + vh })]),
        ];
    }

    attach(list: HTMLElement): void {
        this.list = list;
        activeController = this;
        const onScroll = () => this.replan();
        list.addEventListener('scroll', onScroll, { passive: true });
        // Heights outlive the list, so a width or UI scale that changed while
        // no list was mounted shows up here.
        if (heights.useEnvironment(environmentOf(list))) this.settleAfterChange();
        const resize = new ResizeObserver(() => {
            if (heights.useEnvironment(environmentOf(list))) this.settleAfterChange();
            this.replan();
        });
        resize.observe(list);
        // A highlighted row off the window must mount, so the tree's
        // `aria-activedescendant` names a real element. A highlight on a
        // mounted row re-plans nothing.
        const stopPin = effect(() => {
            const id = this.highlightedRow.value;
            if (id && !list.querySelector(rowSelector(id))) this.bump();
        });
        this.detachList = () => {
            list.removeEventListener('scroll', onScroll);
            resize.disconnect();
            stopPin();
        };
        this.replan();
        this.revealChanged();
    }

    detach(): void {
        this.detachList?.();
        this.detachList = null;
        clearTimeout(this.settleTimer);
        this.list = null;
        if (activeController === this) activeController = null;
    }

    /** Rows rendered wherever they sit: the highlighted one and a reveal's. */
    pinnedIds(): string[] {
        const ids: string[] = [];
        const highlighted = this.highlightedRow.peek();
        if (highlighted) ids.push(highlighted);
        if (pendingReveal) ids.push(pendingReveal.id);
        return ids;
    }

    /** While the list's width or font is changing, a window only grows. */
    isUnstable(): boolean {
        return performance.now() < this.unstableUntil;
    }

    replan(): void {
        const list = this.list;
        if (!list || this.frozen) return;
        const vh = list.clientHeight;
        // A list with no box (a collapsed pane) keeps the window it had.
        if (vh <= 0) return;
        const viewport = { top: list.scrollTop, bottom: list.scrollTop + vh };
        const main = this.main && !needsNewSpan(this.main, viewport) ? this.main : renderSpanFor(viewport);
        this.main = main;
        const next = [main];
        const restore = this.restoreTarget();
        if (restore !== null) next.push(renderSpanFor({ top: restore, bottom: restore + vh }));
        if (this.revealSpan) next.push(this.revealSpan);
        if (!sameSpans(next, this.spans.peek())) this.spans.value = next;
    }

    /** The list's width, a font or the UI scale changed: every height measured
     *  so far is stale. */
    environmentChanged(): void {
        if (this.frozen) return;
        heights.invalidate();
        this.settleAfterChange();
    }

    /** Hold every window from shrinking until the change has settled, and
     *  re-plan every section now. */
    private settleAfterChange(): void {
        this.unstableUntil = performance.now() + SETTLE_MS;
        clearTimeout(this.settleTimer);
        this.settleTimer = setTimeout(() => this.bump(), SETTLE_MS + 16);
        this.bump();
    }

    /** A reveal started: pin its row, then land it once it has mounted. */
    revealChanged(): void {
        this.bump();
        this.tryReveal();
    }

    /** Every section calls this after each commit. */
    afterCommit(): void {
        this.tryReveal();
    }

    private bump(): void {
        this.version.value = this.version.peek() + 1;
    }

    private endReveal(): void {
        pendingReveal = null;
        this.revealSpan = null;
        this.bump();
    }

    /** Land the pending reveal once its row and the viewport it lands in are
     *  mounted. Otherwise render that viewport and try again after the commit.
     *  That also renders every unmeasured row above it, so the row's position
     *  is exact by the time the scroll reads it. */
    private tryReveal(): void {
        const reveal = pendingReveal;
        const list = this.list;
        if (!reveal || !list || this.frozen) return;
        const vh = list.clientHeight;
        const row = list.querySelector<HTMLElement>(rowSelector(reveal.id));
        if (!row) {
            if (++reveal.misses > REVEAL_MISS_LIMIT) this.endReveal();
            return;
        }
        if (vh <= 0) return;
        const st = list.scrollTop;
        const rect = row.getBoundingClientRect();
        const rowTop = rect.top - list.getBoundingClientRect().top + st;
        const rowBottom = rowTop + rect.height;
        const target = reveal.mode === 'centre' ? rowTop + rect.height / 2 - vh / 2
            : rowTop < st ? rowTop
            : rowBottom > st + vh ? rowBottom - vh
            : st;
        const landingViewport = { top: target, bottom: target + vh };
        if (this.spans.peek().every(s => needsNewSpan(s, landingViewport))) {
            const landing = renderSpanFor(landingViewport);
            if (this.revealSpan && sameSpans([this.revealSpan], [landing])) return;
            this.revealSpan = landing;
            this.replan();
            return;
        }
        const { then, mode } = reveal;
        this.endReveal();
        if (mode === 'nearest') {
            if (target !== st) row.scrollIntoView({ block: 'nearest' });
        } else {
            const distance = target - st;
            if (Math.abs(distance) > vh) list.scrollTop = target - Math.sign(distance) * vh;
            then?.(row, list);
        }
        this.replan();
    }
}

export const RowWindowContext = createContext<RowWindowController | null>(null);

/** One controller per mounted Folders list, attached to `.thread-drawer-list`. */
export function useRowWindowController(
    containerRef: { current: HTMLElement | null },
    frozen: boolean,
    restoreTarget: () => number | null,
    highlightedRow: ReadonlySignal<string | null>,
): RowWindowController {
    const controller = useMemo(() => new RowWindowController(restoreTarget, highlightedRow), []);
    controller.frozen = frozen;
    useLayoutEffect(() => {
        const list = containerRef.current?.closest<HTMLElement>('.thread-drawer-list');
        if (!list) return;
        controller.attach(list);
        return () => controller.detach();
    }, []);
    useLayoutEffect(() => {
        if (!frozen) controller.replan();
    }, [frozen]);
    return controller;
}

/** A Folders section's rows, windowed: the rows near the viewport, each pinned
 *  row, and a gap of the exact measured height for every run in between.
 *  `renderRow` builds row `i`. A row's vnode is reused while `renderRow` is the
 *  same function, so moving the window re-renders no row it keeps. */
export function WindowedRows({ name, ids, layoutKeys, renderRow }: {
    name: string;
    ids: readonly string[];
    /** Each row's layout key, by index: equal keys, equal heights. Null for a
     *  row whose height is never cached, which renders wherever it sits above
     *  the viewport. */
    layoutKeys: readonly (string | null)[];
    renderRow: (index: number) => VNode;
}) {
    const controller = useContext(RowWindowContext);
    if (!controller) throw new Error(`WindowedRows "${name}" rendered outside a row window`);
    const bodyRef = useRef<HTMLDivElement>(null);
    const [, rerender] = useState(0);
    const spans = controller.spans.value;
    void controller.version.value;

    // The rows as this render saw them, for the observer's callback.
    const rows = useRef({ ids, layoutKeys });
    rows.current = { ids, layoutKeys };
    const renderedIds = useRef<ReadonlySet<string>>(new Set());

    const plan = (sectionTop: number): RowSegment[] => {
        const pinned = new Set<number>();
        for (const id of controller.pinnedIds()) {
            const i = ids.indexOf(id);
            if (i >= 0) pinned.add(i);
        }
        if (controller.isUnstable()) ids.forEach((id, i) => { if (renderedIds.current.has(id)) pinned.add(i); });
        return planRowWindow({
            count: ids.length,
            heightAt: (i) => {
                const key = layoutKeys[i];
                return key === null ? undefined : heights.valid(ids[i], key);
            },
            estimate: heights.estimate(getRemPx() * FALLBACK_ROW_REM),
            spans: spans.map(s => ({ top: s.top - sectionTop, bottom: s.bottom - sectionTop })),
            pinned,
        });
    };
    const sectionTop = sectionTops.get(name) ?? 0;
    const segments = plan(sectionTop);
    // What this render planned from, for the checks after the commit.
    const planned = useRef<{ top: number; key: string; again: (top: number) => string }>({ top: 0, key: '', again: () => '' });
    planned.current = { top: sectionTop, key: segmentsKey(segments), again: (top) => segmentsKey(plan(top)) };

    const vnodes = useRef<{ renderRow: typeof renderRow; byId: Map<string, VNode> }>({ renderRow, byId: new Map() });
    if (vnodes.current.renderRow !== renderRow) vnodes.current = { renderRow, byId: new Map() };
    const rowVnode = (i: number) => {
        const byId = vnodes.current.byId;
        let vnode = byId.get(ids[i]);
        if (!vnode) {
            vnode = renderRow(i);
            byId.set(ids[i], vnode);
        }
        return vnode;
    };

    const rendered = new Set<string>();
    const children: VNode[] = [];
    for (const segment of segments) {
        if (segment.kind === 'gap') {
            children.push(
                <div key={`gap:${ids[segment.from]}`} class="drawer-row-gap"
                     style={{ height: `${segment.height}px` }} aria-hidden="true"
                     {...{ [FLIP_GAP_END_ATTR]: ids[segment.to - 1] }} />,
            );
            continue;
        }
        for (let i = segment.from; i < segment.to; i++) {
            rendered.add(ids[i]);
            children.push(rowVnode(i));
        }
    }
    renderedIds.current = rendered;

    // One observer per section measures every row it renders, in the frame
    // the row mounts. `borderBoxSize` is the layout box, which a FLIP
    // transform does not change.
    const observed = useRef(new Set<Element>());
    const observedGeneration = useRef(heights.generation);
    const resize = useMemo(() => new ResizeObserver((entries) => {
        if (controller.frozen) return;
        const keyById = new Map(rows.current.ids.map((id, i) => [id, rows.current.layoutKeys[i]]));
        const measured: { id: string; key: string; height: number }[] = [];
        for (const entry of entries) {
            const el = entry.target as HTMLElement;
            const id = el.dataset.flipId;
            const key = id === undefined ? undefined : keyById.get(id);
            const height = entry.borderBoxSize?.[0]?.blockSize ?? el.getBoundingClientRect().height;
            if (!el.isConnected || id === undefined || key == null || height <= 0) continue;
            measured.push({ id, key, height });
        }
        let outcomes = measured.map(m => heights.record(m.id, m.key, m.height));
        if (outcomes.includes('environment')) {
            controller.environmentChanged();
            outcomes = measured.map(m => heights.record(m.id, m.key, m.height));
        }
        const { top, key, again } = planned.current;
        if (outcomes.includes('new') && again(top) !== key) rerender(n => n + 1);
    }), []);

    useLayoutEffect(() => {
        const body = bodyRef.current;
        const list = body?.closest<HTMLElement>('.thread-drawer-list');
        if (!body || !list) return;
        // A row measured before the heights went stale reports nothing new
        // until it resizes, so observe it afresh to measure it again.
        if (observedGeneration.current !== heights.generation) {
            observedGeneration.current = heights.generation;
            for (const el of observed.current) resize.unobserve(el);
            observed.current.clear();
        }
        for (const child of Array.from(body.children) as HTMLElement[]) {
            if (child.dataset.flipId === undefined || observed.current.has(child)) continue;
            resize.observe(child);
            observed.current.add(child);
        }
        for (const el of observed.current) {
            if (el.isConnected) continue;
            resize.unobserve(el);
            observed.current.delete(el);
        }
        // The rows above moved this section: plan again from where it is,
        // before the frame paints.
        if (!controller.frozen && list.clientHeight > 0) {
            const top = body.getBoundingClientRect().top - list.getBoundingClientRect().top + list.scrollTop;
            sectionTops.set(name, top);
            const { top: plannedTop, key, again } = planned.current;
            if (Math.abs(top - plannedTop) > 0.5 && again(top) !== key) rerender(n => n + 1);
        }
        controller.afterCommit();
    });
    useLayoutEffect(() => () => resize.disconnect(), []);

    return <div ref={bodyRef} class="drawer-section-rows">{children}</div>;
}
