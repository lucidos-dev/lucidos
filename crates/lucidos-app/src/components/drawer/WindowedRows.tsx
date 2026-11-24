import { createContext, type VNode } from 'preact';
import { useContext, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { effect, signal, type ReadonlySignal } from '@preact/signals';
import { planRowWindow, RowHeights, needsNewSpan, renderSpanFor, segmentsKey, type RowSegment, type Span } from './rowWindow';
import { getRemPx } from '../../utils/dom';
import { FLIP_GAP_END_ATTR } from '../../hooks/useFlipAnimation';

const heights = new RowHeights();

const sectionTops = new Map<string, number>();

const SETTLE_MS = 150;
const REVEAL_MISS_LIMIT = 30;
const FALLBACK_ROW_REM = 3.5;

export type RevealMode = 'nearest' | 'centre';

interface Reveal {
    id: string;
    mode: RevealMode;
    then?: (row: HTMLElement, list: HTMLElement) => void;
    misses: number;
}

let pendingReveal: Reveal | null = null;
let activeController: RowWindowController | null = null;

export function revealDrawerRow(id: string, mode: RevealMode, then?: Reveal['then']): void {
    pendingReveal = { id, mode, then, misses: 0 };
    activeController?.revealChanged();
}

export function rowWindowIsMounted(): boolean {
    return activeController !== null;
}

export function replanDrawerRowWindow(): void {
    activeController?.replan();
}

function sameSpans(a: readonly Span[], b: readonly Span[]): boolean {
    return a.length === b.length && a.every((s, i) => s.top === b[i].top && s.bottom === b[i].bottom);
}

function rowSelector(id: string): string {
    return `[data-thread-nav="${CSS.escape(id)}"]`;
}

function environmentOf(list: HTMLElement): string {
    return `${list.clientWidth}:${getRemPx()}`;
}

export class RowWindowController {
    readonly spans = signal<Span[]>([]);
    readonly version = signal(0);
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
        if (heights.useEnvironment(environmentOf(list))) this.settleAfterChange();
        const resize = new ResizeObserver(() => {
            if (heights.useEnvironment(environmentOf(list))) this.settleAfterChange();
            this.replan();
        });
        resize.observe(list);
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

    pinnedIds(): string[] {
        const ids: string[] = [];
        const highlighted = this.highlightedRow.peek();
        if (highlighted) ids.push(highlighted);
        if (pendingReveal) ids.push(pendingReveal.id);
        return ids;
    }

    isUnstable(): boolean {
        return performance.now() < this.unstableUntil;
    }

    replan(): void {
        const list = this.list;
        if (!list || this.frozen) return;
        const vh = list.clientHeight;
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

    environmentChanged(): void {
        if (this.frozen) return;
        heights.invalidate();
        this.settleAfterChange();
    }

    private settleAfterChange(): void {
        this.unstableUntil = performance.now() + SETTLE_MS;
        clearTimeout(this.settleTimer);
        this.settleTimer = setTimeout(() => this.bump(), SETTLE_MS + 16);
        this.bump();
    }

    revealChanged(): void {
        this.bump();
        this.tryReveal();
    }

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

export function WindowedRows({ name, ids, layoutKeys, renderRow }: {
    name: string;
    ids: readonly string[];
    layoutKeys: readonly (string | null)[];
    renderRow: (index: number) => VNode;
}) {
    const controller = useContext(RowWindowContext);
    if (!controller) throw new Error(`WindowedRows "${name}" rendered outside a row window`);
    const bodyRef = useRef<HTMLDivElement>(null);
    const [, rerender] = useState(0);
    const spans = controller.spans.value;
    void controller.version.value;

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
