import { useRef, useLayoutEffect } from 'preact/hooks';
import { durationScale } from '../store/store';
import { isReducedMotion } from '../utils/motion';
import { isMobile } from '../utils/viewport';
import { EASING_DISCLOSURE, FADE_REACH, disclosureDurationMs } from '../utils/disclosureMotion';

interface Rect { top: number; left: number; width: number; height: number }
interface Motion { keyframes: Keyframe[]; timing: KeyframeAnimationOptions }

const EASING_DESKTOP = 'cubic-bezier(0.22, 0, 0, 1)';
const EASING_MOBILE = 'cubic-bezier(0.25, 0.1, 0.25, 1)';
const PX_PER_SEC = 600;
const MIN_MS = 250;
const MAX_MS = 450;
const LINGER: Keyframe = { opacity: '1', offset: 0.5 };

export const FLIP_GAP_END_ATTR = 'data-flip-gap-end';

function box(el: Element): Rect {
    const r = el.getBoundingClientRect();
    return { top: r.top, left: r.left, width: r.width, height: r.height };
}

function gapBoxes(container: HTMLElement): Map<string, Rect> {
    const boxes = new Map<string, Rect>();
    for (const gap of container.querySelectorAll<HTMLElement>(`[${FLIP_GAP_END_ATTR}]`)) {
        boxes.set(gap.getAttribute(FLIP_GAP_END_ATTR)!, box(gap));
    }
    return boxes;
}

function gapsIn(ids: readonly string[], gaps: ReadonlyMap<string, Rect>): Rect[] {
    return ids.map(id => gaps.get(id)).filter((r): r is Rect => !!r);
}

export interface FlipSection { name: string; ids: string[] }

export interface DisclosureRun { anchor: string | null; ids: string[] }

export function contiguousRuns(order: string[], members: ReadonlySet<string>): DisclosureRun[] {
    const runs: DisclosureRun[] = [];
    let current: DisclosureRun | null = null;
    order.forEach((id, i) => {
        if (!members.has(id)) {
            current = null;
            return;
        }
        if (!current) {
            current = { anchor: i > 0 ? order[i - 1] : null, ids: [] };
            runs.push(current);
        }
        current.ids.push(id);
    });
    return runs;
}

export interface DisclosureRow { ghost: HTMLElement; rect: Rect }

interface DisclosureEntry { id: string; rect: Rect; ghost?: HTMLElement; real?: HTMLElement }

export function blockExtent(rects: Rect[], anchorRect: Rect | undefined): { line: number; travel: number } {
    const line = anchorRect ? anchorRect.top + anchorRect.height : rects[0].top;
    const bottom = Math.max(...rects.map(r => r.top + r.height));
    return { line, travel: bottom - line };
}

export function rollDistance(rects: Rect[], line: number, travel: number, viewportHeight: number): number {
    if (line < 0) return 0;
    const onScreen = rects.filter(r => r.top < viewportHeight).map(r => r.top + r.height - line);
    return onScreen.length > 0 ? Math.min(travel, Math.max(...onScreen)) : 0;
}

export function crossesViewport(rect: Rect, line: number, distance: number, viewportHeight: number): boolean {
    return rect.top - distance < viewportHeight && rect.top + rect.height > Math.max(0, line);
}

function bottomOf(rect: Rect | undefined): number {
    return rect ? rect.top + rect.height : -Infinity;
}

interface BlockGeometry { line: number; roll: number; rollEnd: number; cut: number }

function blockGeometry(rects: Rect[], anchorRect: Rect | undefined): BlockGeometry {
    const { line, travel } = blockExtent(rects, anchorRect);
    const roll = rollDistance(rects, line, travel, window.innerHeight);
    return { line, roll, rollEnd: line + roll, cut: travel - roll };
}

function rollsInView(rect: Rect, { line, roll, rollEnd }: BlockGeometry): boolean {
    return rect.top < rollEnd && crossesViewport(rect, line, roll, window.innerHeight);
}

function closingIndex(rects: (Rect | undefined)[], { rollEnd }: BlockGeometry): number {
    let best = -1;
    rects.forEach((r, i) => {
        if (r && r.top < rollEnd && (best < 0 || bottomOf(r) > bottomOf(rects[best]))) best = i;
    });
    return best;
}

function currentLift(el: HTMLElement): number {
    const transform = getComputedStyle(el).transform;
    if (!transform || transform === 'none' || typeof DOMMatrixReadOnly === 'undefined') return 0;
    return new DOMMatrixReadOnly(transform).m42;
}

function rowHairlineLeft(row: HTMLElement): number | null {
    const threadRow = row.matches('.thread-row') ? row : row.querySelector<HTMLElement>('.thread-row');
    if (!threadRow) return null;
    const left = parseFloat(getComputedStyle(threadRow, '::after').left);
    return Number.isFinite(left) ? threadRow.getBoundingClientRect().left + left : null;
}

export function rowEndScale(line: { left: number; width: number }, rowLeft: number | null): number {
    if (rowLeft === null || line.width <= 0) return 1;
    return Math.min(1, Math.max(0, (line.left + line.width - rowLeft) / line.width));
}

export function mountDisclosureMask(host: HTMLElement, line: number, rows: DisclosureRow[]): HTMLElement {
    const mask = document.createElement('div');
    mask.className = 'flip-disclosure-mask';
    mask.setAttribute('aria-hidden', 'true');
    mask.inert = true;
    host.appendChild(mask);
    const origin = mask.getBoundingClientRect();
    const bottom = Math.max(...rows.map(({ rect }) => rect.top + rect.height));
    mask.style.top = `${line - origin.top}px`;
    mask.style.height = `${bottom - line}px`;
    placeCopies(mask, rows, line, origin.left);
    return mask;
}

export function mountDepartureLayer(host: HTMLElement, rows: DisclosureRow[]): HTMLElement {
    const layer = document.createElement('div');
    layer.className = 'flip-departure-layer';
    layer.setAttribute('aria-hidden', 'true');
    layer.inert = true;
    host.appendChild(layer);
    const origin = layer.getBoundingClientRect();
    placeCopies(layer, rows, origin.top, origin.left);
    return layer;
}

function placeCopies(box: HTMLElement, rows: DisclosureRow[], top: number, left: number) {
    for (const { ghost, rect } of rows) {
        Object.assign(ghost.style, {
            position: 'absolute', margin: '0', boxSizing: 'border-box',
            top: `${rect.top - top}px`, left: `${rect.left - left}px`,
            width: `${rect.width}px`, height: `${rect.height}px`,
        });
        box.appendChild(ghost);
    }
}

function sameOrder(a: readonly string[], b: readonly string[]): boolean {
    return a.length === b.length && a.every((id, i) => id === b[i]);
}

export function startTogetherOnNextFrame(anims: Animation[], live: () => boolean) {
    if (anims.length === 0) return;
    for (const a of anims) a.pause();
    requestAnimationFrame(() => {
        if (!live()) return;
        const now = document.timeline?.currentTime ?? null;
        for (const a of anims) {
            if (now === null) a.play();
            else a.startTime = now;
        }
    });
}

function playOutDeparture(layer: HTMLElement, exits: Animation[]) {
    startTogetherOnNextFrame(exits, () => layer.isConnected);
    Promise.allSettled(exits.map(a => a.finished)).then(() => layer.remove());
}

function flightDurationMs(distance: number): number {
    return Math.max(MIN_MS, Math.min(MAX_MS, distance / PX_PER_SEC * 1000));
}

export function ghostOf(row: HTMLElement): HTMLElement {
    const ghost = row.cloneNode(true) as HTMLElement;
    for (const node of [ghost, ...ghost.querySelectorAll<HTMLElement>('*')]) {
        node.removeAttribute('id');
        node.removeAttribute('data-flip-id');
        node.removeAttribute('data-thread-nav');
    }
    ghost.setAttribute('aria-hidden', 'true');
    ghost.inert = true;
    return ghost;
}

export function useFlipTransitions(
    containerRef: { current: HTMLElement | null },
    sections: FlipSection[],
    resetKey?: unknown,
    disclosureKeys: readonly unknown[] = [],
) {
    const prevResetKey = useRef<unknown>(resetKey);
    const resetted = prevResetKey.current !== resetKey;
    prevResetKey.current = resetKey;
    const prevDisclosureKeys = useRef(disclosureKeys);
    const disclosing = !resetted && disclosureKeys.some((key, i) => key !== prevDisclosureKeys.current[i]);
    prevDisclosureKeys.current = disclosureKeys;
    const prevSections = useRef<Map<string, string>>(new Map());
    const prevOrder = useRef<string[]>([]);
    const runningAnims = useRef<Animation[]>([]);
    const animatedEls = useRef<HTMLElement[]>([]);
    const hiddenEls = useRef<HTMLElement[]>([]);
    const overlays = useRef<HTMLElement[]>([]);
    const standIns = useRef(new Map<string, HTMLElement>());
    const departing = useRef(new Map<string, { ghost: HTMLElement; exit: Animation; from: string }>());
    const lateRows = useRef<MutationObserver | null>(null);
    useLayoutEffect(() => () => lateRows.current?.disconnect(), []);

    const currentOrder: string[] = [];
    const currentSections = new Map<string, string>();
    for (const section of sections) {
        for (const id of section.ids) {
            currentOrder.push(id);
            currentSections.set(id, section.name);
        }
    }

    const transitioned = new Set<string>();
    const newItems = new Set<string>();
    const removed = new Set<string>();
    const prev = prevSections.current;
    if (prev.size > 0 && !resetted) {
        for (const [id, section] of currentSections) {
            const prevSection = prev.get(id);
            if (!prevSection) {
                newItems.add(id);
            } else if (prevSection !== section) {
                transitioned.add(id);
            }
        }
        for (const id of prev.keys()) {
            if (!currentSections.has(id)) removed.add(id);
        }
    }

    const isThreadRow = (id: string) => !id.startsWith('__section_');
    const revealRuns = disclosing
        ? contiguousRuns(currentOrder, new Set([...newItems].filter(isThreadRow)))
        : [];
    const hideRuns = disclosing
        ? contiguousRuns(prevOrder.current, new Set([...removed].filter(isThreadRow)))
        : [];
    const revealed = new Set(revealRuns.flatMap(run => run.ids));
    const hiding = new Set(hideRuns.flatMap(run => run.ids));

    const affectedSiblings = new Set<string>();
    const hasChanges = transitioned.size > 0 || newItems.size > 0 || removed.size > 0;
    if (hasChanges && prevOrder.current.length > 0) {
        for (const id of currentOrder) {
            if (!transitioned.has(id) && !newItems.has(id)) {
                affectedSiblings.add(id);
            }
        }
    }

    const oldRects = new Map<string, Rect>();
    const ghosts = new Map<string, HTMLElement>();
    const departures = new Map<string, HTMLElement>();
    const seen = new Map<string, { rect: Rect; opacity: number }>();
    const hidingLineLefts = new Map<string, number | null>();
    const lifts = new Map<string, number>();
    let oldGaps: ReadonlyMap<string, Rect> = new Map();
    const container = containerRef.current;
    if (container && hasChanges) {
        oldGaps = gapBoxes(container);
        for (const [id, copy] of disclosing ? standIns.current : []) {
            seen.set(id, { rect: box(copy), opacity: Number(getComputedStyle(copy).opacity) });
        }
        const rows = container.querySelectorAll<HTMLElement>('[data-flip-id]');
        const hidingRows = new Map<string, HTMLElement>();
        const moving = runningAnims.current.length > 0;
        for (const row of rows) {
            const id = row.dataset.flipId!;
            if (moving) lifts.set(id, currentLift(row));
            const rect = (hiding.has(id) ? seen.get(id)?.rect : undefined) ?? box(row);
            oldRects.set(id, rect);
            if (hiding.has(id)) hidingRows.set(id, row);
            const onScreen = rect.top + rect.height > 0 && rect.top < window.innerHeight;
            const left = removed.has(id) || transitioned.has(id);
            if (!disclosing && left && onScreen) departures.set(id, ghostOf(row));
        }
        for (const run of hideRuns) {
            const rects = run.ids.map(id => oldRects.get(id)).filter((r): r is Rect => !!r);
            if (rects.length === 0) continue;
            const geometry = blockGeometry([...rects, ...gapsIn(run.ids, oldGaps)], run.anchor ? oldRects.get(run.anchor) : undefined);
            for (const id of run.ids) {
                const rect = oldRects.get(id);
                const row = hidingRows.get(id);
                if (!rect || !row || !rollsInView(rect, geometry)) continue;
                ghosts.set(id, ghostOf(seen.has(id) ? standIns.current.get(id)! : row));
            }
            if (run.anchor && !isThreadRow(run.anchor)) {
                const closing = run.ids[closingIndex(run.ids.map(id => oldRects.get(id)), geometry)];
                const row = closing ? hidingRows.get(closing) : undefined;
                hidingLineLefts.set(run.anchor, row ? rowHairlineLeft(row) : null);
            }
        }
    }

    const priorOrder = prevOrder.current;
    prevSections.current = currentSections;
    prevOrder.current = currentOrder;

    useLayoutEffect(() => {
        if (lateRows.current && (resetted || hasChanges || !sameOrder(priorOrder, currentOrder))) {
            lateRows.current.disconnect();
            lateRows.current = null;
        }

        const el = containerRef.current;
        if (!el || !hasChanges || oldRects.size === 0) return;

        if (isReducedMotion()) return;

        for (const [id, copy] of departing.current) {
            if (currentSections.get(id) !== copy.from) continue;
            copy.exit.cancel();
            copy.ghost.remove();
        }

        for (const a of runningAnims.current) {
            try { a.cancel(); } catch {  }
        }
        for (const r of animatedEls.current) {
            r.classList.remove('flip-animating');
        }
        for (const h of hiddenEls.current) {
            h.style.opacity = '';
        }
        for (const c of overlays.current) {
            c.remove();
        }
        runningAnims.current = [];
        animatedEls.current = [];
        hiddenEls.current = [];
        overlays.current = [];
        standIns.current.clear();

        const mobile = isMobile();
        const scale = durationScale.value;
        const easing = mobile ? EASING_MOBILE : EASING_DESKTOP;

        const rows = el.querySelectorAll<HTMLElement>('[data-flip-id]');
        const rowById = new Map<string, HTMLElement>();
        for (const row of rows) rowById.set(row.dataset.flipId!, row);
        const newAnims: Animation[] = [];
        const touched: HTMLElement[] = [];
        const hidden: HTMLElement[] = [];
        const created: HTMLElement[] = [];

        const anchorDrift = (anchor: string | null) => {
            const oldRect = anchor ? oldRects.get(anchor) : undefined;
            const anchorEl = anchor ? rowById.get(anchor) : undefined;
            return oldRect && anchorEl ? oldRect.top - anchorEl.getBoundingClientRect().top : 0;
        };

        const carryHairline = (
            host: HTMLElement, anchor: string, rollEnd: number, rowLeft: number | null,
            fromY: number, toY: number, reveal: boolean, timing: KeyframeAnimationOptions,
        ) => {
            const key = `__hairline_${anchor}`;
            const hairline = document.createElement('div');
            hairline.className = 'flip-disclosure-hairline';
            hairline.setAttribute('aria-hidden', 'true');
            host.appendChild(hairline);
            const top = rollEnd - 1;
            hairline.style.top = `${top - host.getBoundingClientRect().top}px`;
            created.push(hairline);
            const box = hairline.getBoundingClientRect();
            const atRow = rowEndScale(box, rowLeft);
            const was = seen.get(key);
            standIns.current.set(key, hairline);
            const fromScale = was && box.width > 0 ? was.rect.width / box.width : reveal ? 1 : atRow;
            const at = (y: number, s: number) => ({ transform: `translateY(${y}px) scaleX(${s})` });
            return hairline.animate(
                [at(was ? was.rect.top - top : fromY, fromScale), at(toY, reveal ? atRow : 1)], timing);
        };

        const disclosureBlock = (
            run: DisclosureRun, entries: DisclosureEntry[], geometry: BlockGeometry, reveal: boolean,
            rowLeft: number | null,
        ) => {
            const rows = entries.filter((e): e is DisclosureEntry & DisclosureRow => !!e.ghost);
            if (rows.length === 0) return null;
            const waiting = entries.filter(e => e.real && e.rect.top >= geometry.rollEnd).map(e => e.real!);
            return { run, rows, waiting, ...geometry, drift: anchorDrift(run.anchor), reveal, rowLeft };
        };
        const gaps = gapBoxes(el);
        const blocks = [
            ...revealRuns.map(run => {
                const reals = run.ids.map(id => rowById.get(id)).filter((r): r is HTMLElement => !!r);
                if (reals.length === 0) return null;
                const rects = reals.map(real => real.getBoundingClientRect());
                const anchorRect = run.anchor ? rowById.get(run.anchor)?.getBoundingClientRect() : undefined;
                const geometry = blockGeometry([...rects, ...gapsIn(run.ids, gaps)], anchorRect);
                const closing = reals[closingIndex(rects, geometry)];
                return disclosureBlock(
                    run,
                    reals.map((real, i) => ({
                        id: real.dataset.flipId!, real, rect: rects[i],
                        ghost: rollsInView(rects[i], geometry) ? ghostOf(real) : undefined,
                    })),
                    geometry, true,
                    run.anchor && !isThreadRow(run.anchor) && closing ? rowHairlineLeft(closing) : null,
                );
            }),
            ...hideRuns.map(run => {
                const ids = run.ids.filter(id => oldRects.has(id));
                const rects = ids.map(id => oldRects.get(id)!);
                return disclosureBlock(
                    run,
                    ids.map((id, i) => ({ id, ghost: ghosts.get(id), rect: rects[i] })),
                    blockGeometry([...rects, ...gapsIn(run.ids, oldGaps)], run.anchor ? oldRects.get(run.anchor) : undefined), false,
                    run.anchor ? hidingLineLefts.get(run.anchor) ?? null : null,
                );
            }),
        ].filter(b => b !== null);
        const disclosureDuration = blocks.length > 0
            ? disclosureDurationMs(Math.max(...blocks.map(b => b.roll))) * durationScale.value
            : 0;

        for (const { run, rows: blockRows, waiting, line, roll, rollEnd, drift, reveal, rowLeft } of blocks) {
            const host = (run.anchor ? rowById.get(run.anchor)?.parentElement : null) ?? el;
            const mask = mountDisclosureMask(host, line, blockRows);
            created.push(mask);
            for (const real of [...blockRows.map(r => r.real), ...waiting]) {
                if (!real) continue;
                real.style.opacity = '0';
                hidden.push(real);
            }
            const timing = { duration: disclosureDuration, easing: EASING_DISCLOSURE, fill: 'forwards' as const };
            const startY = (id: string, rect: Rect) => {
                const was = seen.get(id);
                if (!was) return reveal ? -roll : 0;
                return reveal ? was.rect.top - rect.top - drift : 0;
            };
            const endY = reveal ? 0 : -roll;
            const top = blockRows[0];
            const maskAt = (driftY: number, copyY: number): Keyframe => ({
                transform: `translateY(${driftY}px)`,
                [FADE_REACH]: `${Math.max(0, line - (top.rect.top + copyY))}px`,
            });
            const drifted = reveal ? drift : -drift;
            newAnims.push(mask.animate(reveal
                ? [maskAt(drifted, startY(top.id, top.rect)), maskAt(0, endY)]
                : [maskAt(0, startY(top.id, top.rect)), maskAt(drifted, endY)], timing));
            for (const { id, ghost, rect } of blockRows) {
                const was = seen.get(id);
                const fromOpacity = was ? String(was.opacity) : reveal ? '0' : '1';
                newAnims.push(ghost.animate([
                    { transform: `translateY(${startY(id, rect)}px)`, opacity: fromOpacity },
                    { transform: `translateY(${endY}px)`, opacity: reveal ? '1' : '0' },
                ], timing));
                standIns.current.set(id, ghost);
            }
            if (run.anchor && !isThreadRow(run.anchor) && rowLeft !== null) {
                newAnims.push(carryHairline(host, run.anchor, rollEnd, rowLeft,
                    reveal ? drift - roll : 0, reveal ? 0 : -(roll + drift), reveal, timing));
            }
        }

        const orderIndex = new Map(currentOrder.map((id, i) => [id, i]));
        const cutBelow = (id: string) => {
            let hidingCut = 0;
            let end = 0;
            const at = orderIndex.get(id) ?? -1;
            for (const block of blocks) {
                const after = block.run.anchor ? orderIndex.get(block.run.anchor) ?? -1 : -1;
                if (at <= after) continue;
                if (block.reveal) end -= block.cut;
                else hidingCut += block.cut;
            }
            const start = hidingCut > 0 ? -Math.max(0, (lifts.get(id) ?? 0) + hidingCut) : 0;
            return { start, end };
        };
        const anchorIndex = (run: DisclosureRun) => run.anchor ? orderIndex.get(run.anchor) ?? -1 : -1;
        const stillAfter = [
            ...revealRuns.map(run => [run, run.anchor ? rowById.get(run.anchor)?.getBoundingClientRect() : undefined] as const),
            ...hideRuns.map(run => [run, run.anchor ? oldRects.get(run.anchor) : undefined] as const),
        ]
            .filter(([, rect]) => rect !== undefined && rect.top + rect.height < 0)
            .map(([run]) => anchorIndex(run));
        const rollingAfter = blocks.map(block => anchorIndex(block.run));
        const belowStill = (id: string) => {
            const at = orderIndex.get(id) ?? -1;
            const nearest = (anchors: number[]) => Math.max(-Infinity, ...anchors.filter(after => at > after));
            return nearest(stillAfter) > nearest(rollingAfter);
        };

        const settled = new Set([...transitioned].filter(id => {
            const now = rowById.get(id)?.getBoundingClientRect();
            const was = oldRects.get(id);
            return !!now && !!was && Math.abs(now.left - was.left) < 1 && Math.abs(now.top - was.top) < 1;
        }));

        const leaving = [...departures]
            .filter(([id]) => !settled.has(id))
            .map(([id, ghost]) => ({ id, ghost, rect: oldRects.get(id)! }));
        if (leaving.length > 0) {
            const layer = mountDepartureLayer(el, leaving);
            const exits: Animation[] = [];
            for (const { id, ghost } of leaving) {
                const exit = ghost.animate([
                    { transform: 'scaleY(1)', transformOrigin: 'top', opacity: '1' },
                    LINGER,
                    { transform: 'scaleY(0)', transformOrigin: 'top', opacity: '0' },
                ], { duration: MIN_MS * scale, easing, fill: 'forwards' });
                exits.push(exit);
                departing.current.set(id, { ghost, exit, from: prev.get(id)! });
                const forget = () => {
                    if (departing.current.get(id)?.exit === exit) departing.current.delete(id);
                };
                exit.finished.then(forget, forget);
            }
            playOutDeparture(layer, exits);
        }

        const play = (row: HTMLElement, motion: Motion): Animation => {
            row.classList.add('flip-animating');
            touched.push(row);
            const anim = row.animate(motion.keyframes, motion.timing);
            newAnims.push(anim);
            return anim;
        };
        const arrival = (id: string): Motion | null => {
            if (newItems.has(id)) {
                const isSection = id.startsWith('__section_');
                const keyframes = (isSection || mobile)
                    ? [{ opacity: '0' }, { opacity: '1' }]
                    : [
                        { opacity: '0', transform: 'scale(0.95)', transformOrigin: 'center' },
                        { opacity: '1', transform: 'scale(1.03)', transformOrigin: 'center', offset: 0.6 },
                        { opacity: '1', transform: 'scale(1)', transformOrigin: 'center' },
                    ];
                return { keyframes, timing: { duration: (isSection ? 350 : 450) * scale, easing, fill: 'none' } };
            }
            if (transitioned.has(id) && !settled.has(id)) {
                return { keyframes: [{ opacity: '0' }, { opacity: '1' }], timing: { duration: MIN_MS * scale, easing, fill: 'none' } };
            }
            return null;
        };

        const slides = new Map<string, Motion | null>();
        for (const row of rows) {
            const id = row.dataset.flipId!;
            if (revealed.has(id)) continue;
            const arriving = arrival(id);
            if (arriving) {
                play(row, arriving);
                continue;
            }
            if (!affectedSiblings.has(id)) continue;
            const oldRect = oldRects.get(id);
            if (!oldRect) continue;
            if (belowStill(id)) {
                slides.set(id, null);
                continue;
            }

            const newRect = row.getBoundingClientRect();
            const deltaX = oldRect.left - newRect.left;
            const deltaY = oldRect.top - newRect.top;
            if (Math.abs(deltaX) < 1 && Math.abs(deltaY) < 1) {
                slides.set(id, null);
                continue;
            }

            const pushed = disclosureDuration > 0;
            const duration = pushed
                ? disclosureDuration
                : flightDurationMs(Math.sqrt(deltaX * deltaX + deltaY * deltaY)) * scale;

            const { start, end } = cutBelow(id);
            const slide: Motion = {
                keyframes: [
                    { transform: `translate(${deltaX}px, ${deltaY + start}px)` },
                    { transform: `translate(0, ${end}px)` },
                ],
                timing: { duration, easing: pushed ? EASING_DISCLOSURE : easing, fill: 'none' },
            };
            slides.set(id, slide);
            play(row, slide);
        }

        const priorIndex = new Map(priorOrder.map((id, i) => [id, i]));
        const inheritedSlide = (id: string): Motion | null => {
            const at = orderIndex.get(id);
            const was = priorIndex.get(id);
            if (at === undefined || was === undefined) return null;
            for (let i = at - 1; i >= 0; i--) {
                const above = currentOrder[i];
                if (!affectedSiblings.has(above)) return null;
                if (slides.has(above)) return priorIndex.get(above) === was - (at - i) ? slides.get(above)! : null;
                if (rowById.has(above)) return null;
            }
            return null;
        };
        const lateMotion = (id: string): Motion | null => {
            const arriving = arrival(id);
            if (arriving || !affectedSiblings.has(id)) return arriving;
            if (slides.has(id)) return slides.get(id)!;
            const inherited = inheritedSlide(id);
            if (inherited) slides.set(id, inherited);
            return inherited;
        };
        const joinLate = (row: HTMLElement) => {
            const id = row.dataset.flipId!;
            if (rowById.get(id) === row || revealed.has(id)) return;
            rowById.set(id, row);
            const motion = lateMotion(id);
            if (!motion) return;
            const anim = play(row, motion);
            const lead = newAnims[0];
            if (lead.playState === 'paused') anim.pause();
            else if (lead.startTime !== null) anim.startTime = lead.startTime;
        };

        runningAnims.current = newAnims;
        startTogetherOnNextFrame(newAnims, () => runningAnims.current === newAnims);
        animatedEls.current = touched;
        hiddenEls.current = hidden;
        overlays.current = created;

        if (newAnims.length > 0) {
            const observer = new MutationObserver((records) => {
                for (const record of records) {
                    for (const node of record.addedNodes) {
                        if (!(node instanceof HTMLElement)) continue;
                        if (node.dataset.flipId !== undefined) joinLate(node);
                        for (const row of node.querySelectorAll<HTMLElement>('[data-flip-id]')) joinLate(row);
                    }
                }
            });
            observer.observe(el, { childList: true, subtree: true });
            lateRows.current = observer;

            const allFinished = (): Promise<void> => {
                const count = newAnims.length;
                return Promise.allSettled(newAnims.map(a => a.finished))
                    .then(() => (newAnims.length > count ? allFinished() : undefined));
            };
            allFinished().then(() => {
                if (runningAnims.current !== newAnims) return;
                lateRows.current?.disconnect();
                lateRows.current = null;
                for (const r of touched) {
                    r.classList.remove('flip-animating');
                }
                for (const h of hidden) {
                    h.style.opacity = '';
                }
                for (const c of created) {
                    c.remove();
                }
                runningAnims.current = [];
                animatedEls.current = [];
                hiddenEls.current = [];
                overlays.current = [];
                standIns.current.clear();
            });
        }
    });
}
