import { useRef, useLayoutEffect } from 'preact/hooks';
import { durationScale } from '../store/store';
import { isReducedMotion } from '../utils/motion';
import { isMobile } from '../utils/viewport';

interface Rect { top: number; left: number; width: number; height: number }

const EASING_DESKTOP = 'cubic-bezier(0.22, 0, 0, 1)';
const EASING_MOBILE = 'cubic-bezier(0.25, 0.1, 0.25, 1)'; // simpler ease for mobile perf
// A disclosure gathers speed, then eases into its landing, on every client.
const EASING_DISCLOSURE = 'cubic-bezier(0.4, 0, 0.2, 1)';
const PX_PER_SEC = 600;
const MIN_MS = 250;
const MAX_MS = 450;
const DISCLOSURE_PX_PER_SEC = 1200;
const DISCLOSURE_MIN_MS = 260;
const DISCLOSURE_MAX_MS = 420;
// A departing copy holds full opacity through its first half. Its curve starts
// fast, so fading from the start leaves it half gone by the first frames.
const LINGER: Keyframe = { opacity: '1', offset: 0.5 };

/** A section's rows in render order, its header first. `tucked` holds the
 *  threads a collapsed section keeps out of view. */
export interface FlipSection { name: string; ids: string[]; tucked?: string[] }

/** The header a departing thread folds into: the first row of the section
 *  that now holds it out of view. Null when it left every section. */
export function tuckTarget(id: string, sections: FlipSection[]): string | null {
    return sections.find(s => s.tucked?.includes(id))?.ids[0] ?? null;
}

/** A run of adjacent rows that appear or disappear together, under `anchor`,
 *  the row just before them (their parent, for a sub-thread family). */
export interface DisclosureRun { anchor: string | null; ids: string[] }

/** Split `order` into the contiguous runs of `members`. */
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

/** A row copy and the viewport box it stands in for. */
export interface DisclosureRow { ghost: HTMLElement; rect: Rect }

/** A row a disclosure moves. Only a row the user can see gets a copy. */
interface DisclosureEntry { id: string; rect: Rect; ghost?: HTMLElement; real?: HTMLElement }

/** A block's reveal line (the anchor's bottom edge, else the first row's top)
 *  and its full height, down to its lowest row's bottom edge. */
export function blockExtent(rects: Rect[], anchorRect: Rect | undefined): { line: number; travel: number } {
    const line = anchorRect ? anchorRect.top + anchorRect.height : rects[0].top;
    const bottom = Math.max(...rects.map(r => r.top + r.height));
    return { line, travel: bottom - line };
}

/** How far a block's copies move. A block that fits on screen rolls its whole
 *  height. A long one rolls down to the last row that starts on screen, so it
 *  moves at most about one screen. A block whose line has scrolled above the
 *  screen does not roll: no parent or header is on screen to roll under. */
export function rollDistance(rects: Rect[], line: number, travel: number, viewportHeight: number): number {
    if (line < 0) return 0;
    const onScreen = rects.filter(r => r.top < viewportHeight).map(r => r.top + r.height - line);
    return onScreen.length > 0 ? Math.min(travel, Math.max(...onScreen)) : 0;
}

/** Whether a copy sliding from `rect` to `distance` px above it, clipped at
 *  the reveal line, crosses the viewport at any point. A long section can hold
 *  hundreds of rows, and copying unseen ones would stall the first frame. */
export function crossesViewport(rect: Rect, line: number, distance: number, viewportHeight: number): boolean {
    return rect.top - distance < viewportHeight && rect.top + rect.height > Math.max(0, line);
}

function bottomOf(rect: Rect | undefined): number {
    return rect ? rect.top + rect.height : -Infinity;
}

/** A disclosure block's reveal line. `roll` is how far its copies move and
 *  `rollEnd` where the rolling part ends. `cut` is the height left out of the
 *  roll, which the rows below skip too. */
interface BlockGeometry { line: number; roll: number; rollEnd: number; cut: number }

function blockGeometry(rects: Rect[], anchorRect: Rect | undefined): BlockGeometry {
    const { line, travel } = blockExtent(rects, anchorRect);
    const roll = rollDistance(rects, line, travel, window.innerHeight);
    return { line, roll, rollEnd: line + roll, cut: travel - roll };
}

/** Whether a row rolls with the block and is seen doing it, so it needs a
 *  copy. Rows below a capped roll's cut stay out of the motion. */
function rollsInView(rect: Rect, { line, roll, rollEnd }: BlockGeometry): boolean {
    return rect.top < rollEnd && crossesViewport(rect, line, roll, window.innerHeight);
}

/** Index of the row that closes the rolling part of a block. */
function closingIndex(rects: (Rect | undefined)[], { rollEnd }: BlockGeometry): number {
    let best = -1;
    rects.forEach((r, i) => {
        if (r && r.top < rollEnd && (best < 0 || bottomOf(r) > bottomOf(rects[best]))) best = i;
    });
    return best;
}

/** The vertical offset a running animation currently gives `el`, or 0. */
function currentLift(el: HTMLElement): number {
    const transform = getComputedStyle(el).transform;
    if (!transform || transform === 'none' || typeof DOMMatrixReadOnly === 'undefined') return 0;
    return new DOMMatrixReadOnly(transform).m42;
}

/** Viewport x where a row's own bottom hairline starts, or null for a row
 *  that draws none. A sub-thread's line starts further in, at its indent. */
function rowHairlineLeft(row: HTMLElement): number | null {
    const threadRow = row.matches('.thread-row') ? row : row.querySelector<HTMLElement>('.thread-row');
    if (!threadRow) return null;
    const left = parseFloat(getComputedStyle(threadRow, '::after').left);
    return Number.isFinite(left) ? threadRow.getBoundingClientRect().left + left : null;
}

/** The horizontal scale that shrinks a carried line from its own box to a row
 *  hairline starting at `rowLeft`. The two share a right edge, which is where
 *  the line's transform is anchored. */
export function rowEndScale(line: { left: number; width: number }, rowLeft: number | null): number {
    if (rowLeft === null || line.width <= 0) return 1;
    return Math.min(1, Math.max(0, (line.left + line.width - rowLeft) / line.width));
}

/** Mount `rows` in a mask whose top edge is the reveal line, the anchor's
 *  bottom edge. The mask's overflow clip hides any copy that slides above the
 *  line, so the copies pass under their parent. An animated clip-path did not
 *  render in every WebKit, and an overflow clip renders everywhere. */
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

/** Mount copies of rows that left the list, each at its old box. The layer
 *  scrolls and clips with the list, and does not clip the copies itself. */
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

/** Append each copy to `box` at its viewport rect, offset by `top` and `left`. */
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

/** Hold `anims` on their first frame, then start them all on the frame that
 *  paints them. An archive also opens the next thread, and on WebKit that
 *  render delays the first paint by most of an exit. A shared start also keeps
 *  copies in step with the rows sliding below them. `live` says whether this
 *  batch still owns the drawer. */
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

/** Base duration for a row that moves `distance` px, before scaling. */
function flightDurationMs(distance: number): number {
    return Math.max(MIN_MS, Math.min(MAX_MS, distance / PX_PER_SEC * 1000));
}

/** Base duration for a disclosure that rolls `distance` px, before scaling. */
export function disclosureDurationMs(distance: number): number {
    return Math.min(DISCLOSURE_MAX_MS, Math.max(DISCLOSURE_MIN_MS, distance / DISCLOSURE_PX_PER_SEC * 1000));
}

/** An inert copy of a row that is about to unmount, for it to roll away as.
 *  It sheds every attribute a lookup finds the real row by, so a query for the
 *  thread never lands on the copy. */
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

/**
 * FLIP animation for thread list transitions.
 *
 * Uses a portal div for the flying thread — a cloned element rendered at
 * position:fixed, completely outside the scroll container and stacking context
 * hierarchy, so it renders above all app chrome regardless of CSS transforms or
 * overflow clipping. The portal sits just below the modal/restart/toast layer
 * (see .flip-portal in drawer.css), so a thread whose status changes during an
 * engine restart can't fly above the restart overlay.
 *
 * A render where any of `disclosureKeys` changed is a disclosure: a family
 * showing or hiding its sub-threads, or a section its threads. Those rows
 * unroll from under the row above them, and hidden ones roll back up as
 * ghosts, in lockstep with the rows sliding below.
 *
 * A row that leaves on any other render departs as a copy. An archived thread
 * that lands in a collapsed section flies into that section's header. Any
 * other row folds away where it stood.
 *
 * Speed setting: slider 0 = normal (1x), +10 = 10x faster, -10 = 10x slower.
 */
export function useFlipTransitions(
    containerRef: { current: HTMLElement | null },
    portalRef: { current: HTMLElement | null },
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
    const portalClones = useRef<HTMLElement[]>([]);
    // What a running disclosure shows in place of each thread (its row copy),
    // and of each section's divider (its carried line).
    const standIns = useRef(new Map<string, HTMLElement>());

    // ── Build flat ordered list and section map ──
    const currentOrder: string[] = [];
    const currentSections = new Map<string, string>();
    for (const section of sections) {
        for (const id of section.ids) {
            currentOrder.push(id);
            currentSections.set(id, section.name);
        }
    }

    // ── Detect transitions, new items and removed items ──
    // A row that left counts too: a thread archived into a collapsed Archive
    // leaves the rendered rows, and the rows below it must still slide up.
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

    // ── Disclosure runs: rows a family or section toggle showed or hid ──
    const isThreadRow = (id: string) => !id.startsWith('__section_');
    const revealRuns = disclosing
        ? contiguousRuns(currentOrder, new Set([...newItems].filter(isThreadRow)))
        : [];
    const hideRuns = disclosing
        ? contiguousRuns(prevOrder.current, new Set([...removed].filter(isThreadRow)))
        : [];
    const revealed = new Set(revealRuns.flatMap(run => run.ids));
    const hiding = new Set(hideRuns.flatMap(run => run.ids));

    // ── Build affected siblings: any existing item that isn't moved/new ──
    // When sections appear/disappear, index-range heuristics miss items outside
    // the source-dest range (e.g. a section header below the moved thread).
    // Instead, mark ALL surviving items as potential siblings: the `noMovement`
    // pixel-delta check below skips items that didn't actually move.
    const affectedSiblings = new Set<string>();
    const hasChanges = transitioned.size > 0 || newItems.size > 0 || removed.size > 0;
    if (hasChanges && prevOrder.current.length > 0) {
        for (const id of currentOrder) {
            if (!transitioned.has(id) && !newItems.has(id)) {
                affectedSiblings.add(id);
            }
        }
    }

    // ── Capture OLD positions from the still-current DOM ──
    // Rows about to hide are copied now too: after the commit they are gone.
    // A thread this toggle shows or hides, while a running copy stands in for
    // it, is read through that copy: it is what the user sees. A carried line
    // is read the same way. Every other row reads itself, since it moves
    // unmasked and must not start under a parent.
    const oldRects = new Map<string, Rect>();
    const ghosts = new Map<string, HTMLElement>();
    // Copies of on-screen rows that left without a toggle.
    const departures = new Map<string, HTMLElement>();
    const seen = new Map<string, { rect: Rect; opacity: number }>();
    // Per section header: where the closing row's own hairline starts.
    const hidingLineLefts = new Map<string, number | null>();
    // How far a running batch has each row moved off its layout box.
    const lifts = new Map<string, number>();
    const container = containerRef.current;
    if (container && hasChanges) {
        const box = (el: HTMLElement): Rect => {
            const r = el.getBoundingClientRect();
            return { top: r.top, left: r.left, width: r.width, height: r.height };
        };
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
            if (!disclosing && removed.has(id) && onScreen) departures.set(id, ghostOf(row));
        }
        for (const run of hideRuns) {
            const rects = run.ids.map(id => oldRects.get(id)).filter((r): r is Rect => !!r);
            if (rects.length === 0) continue;
            const geometry = blockGeometry(rects, run.anchor ? oldRects.get(run.anchor) : undefined);
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

    // ── Update refs for next render ──
    prevSections.current = currentSections;
    prevOrder.current = currentOrder;

    useLayoutEffect(() => {
        const el = containerRef.current;
        const portal = portalRef.current;
        if (!el || !portal || !hasChanges || oldRects.size === 0) return;

        if (isReducedMotion()) return;

        // Cancel previous animations and restore hidden elements
        for (const a of runningAnims.current) {
            try { a.cancel(); } catch { /* already finished */ }
        }
        for (const r of animatedEls.current) {
            r.classList.remove('flip-animating');
        }
        for (const h of hiddenEls.current) {
            h.style.opacity = '';
        }
        for (const c of portalClones.current) {
            c.remove();
        }
        runningAnims.current = [];
        animatedEls.current = [];
        hiddenEls.current = [];
        portalClones.current = [];
        standIns.current.clear();

        const mobile = isMobile();
        // The shared slider scale (store/store.ts), the same one every CSS
        // --duration-* token folds in. Phones get the full length: a row that
        // leaves in 150ms is gone before a reader sees it go.
        const scale = durationScale.value;
        const easing = mobile ? EASING_MOBILE : EASING_DESKTOP;

        const rows = el.querySelectorAll<HTMLElement>('[data-flip-id]');
        const rowById = new Map<string, HTMLElement>();
        for (const row of rows) rowById.set(row.dataset.flipId!, row);
        const newAnims: Animation[] = [];
        const touched: HTMLElement[] = [];
        const hidden: HTMLElement[] = [];
        const clones: HTMLElement[] = [];

        // The anchor's own slide, if the commit moved it (a scroll clamp, say).
        const anchorDrift = (anchor: string | null) => {
            const oldRect = anchor ? oldRects.get(anchor) : undefined;
            const anchorEl = anchor ? rowById.get(anchor) : undefined;
            return oldRect && anchorEl ? oldRect.top - anchorEl.getBoundingClientRect().top : 0;
        };

        // A section's rows fade as they roll, so their hairlines cannot carry
        // its closing line. This line does, at full strength, on the rolling
        // part's bottom edge: it lands on the collapsed divider, or leaves
        // from it. At the row end it narrows to the closing row's own indent.
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
            clones.push(hairline);
            const box = hairline.getBoundingClientRect();
            const atRow = rowEndScale(box, rowLeft);
            const was = seen.get(key);
            standIns.current.set(key, hairline);
            const fromScale = was && box.width > 0 ? was.rect.width / box.width : reveal ? 1 : atRow;
            const at = (y: number, s: number) => ({ transform: `translateY(${y}px) scaleX(${s})` });
            return hairline.animate(
                [at(was ? was.rect.top - top : fromY, fromScale), at(toY, reveal ? atRow : 1)], timing);
        };

        // ── Disclosure: copies slide in a mask under the anchor. ──
        // Showing rows wait invisible in their new boxes. Hiding rows leave
        // from their old boxes, as copies taken before the commit. A block
        // rolls at most about one screen (`rollDistance`), and the rows below
        // it move that same distance, in lockstep.
        const disclosureBlock = (
            run: DisclosureRun, entries: DisclosureEntry[], geometry: BlockGeometry, reveal: boolean,
            rowLeft: number | null,
        ) => {
            const rows = entries.filter((e): e is DisclosureEntry & DisclosureRow => !!e.ghost);
            if (rows.length === 0) return null;
            // Shown rows past the cut wait hidden too: a scroll mid-motion
            // must not bring them up under the rows below, held one cut up.
            const waiting = entries.filter(e => e.real && e.rect.top >= geometry.rollEnd).map(e => e.real!);
            return { run, rows, waiting, ...geometry, drift: anchorDrift(run.anchor), reveal, rowLeft };
        };
        const blocks = [
            ...revealRuns.map(run => {
                const reals = run.ids.map(id => rowById.get(id)).filter((r): r is HTMLElement => !!r);
                if (reals.length === 0) return null;
                const rects = reals.map(real => real.getBoundingClientRect());
                const anchorRect = run.anchor ? rowById.get(run.anchor)?.getBoundingClientRect() : undefined;
                const geometry = blockGeometry(rects, anchorRect);
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
                    blockGeometry(rects, run.anchor ? oldRects.get(run.anchor) : undefined), false,
                    run.anchor ? hidingLineLefts.get(run.anchor) ?? null : null,
                );
            }),
        ].filter(b => b !== null);
        // No phone shortening here. A tall section already races past, and
        // cut further its rows' hairlines strobe by.
        const disclosureDuration = blocks.length > 0
            ? disclosureDurationMs(Math.max(...blocks.map(b => b.roll))) * durationScale.value
            : 0;

        for (const { run, rows: blockRows, waiting, line, roll, rollEnd, drift, reveal, rowLeft } of blocks) {
            // The mask joins the rows' own section, so the drawer's selectors
            // still style the copies and they scroll with the list.
            const host = (run.anchor ? rowById.get(run.anchor)?.parentElement : null) ?? el;
            const mask = mountDisclosureMask(host, line, blockRows);
            clones.push(mask);
            for (const real of [...blockRows.map(r => r.real), ...waiting]) {
                if (!real) continue;
                real.style.opacity = '0';
                hidden.push(real);
            }
            const timing = { duration: disclosureDuration, easing: EASING_DISCLOSURE, fill: 'forwards' as const };
            // The mask rides the anchor, and the copies slide the roll inside
            // it, in lockstep with the rows below.
            const atAnchor = { transform: 'translateY(0)' };
            const drifted = { transform: `translateY(${reveal ? drift : -drift}px)` };
            newAnims.push(mask.animate(reveal ? [drifted, atAnchor] : [atAnchor, drifted], timing));
            const open = { transform: 'translateY(0)', opacity: '1' };
            const shut = { transform: `translateY(${-roll}px)`, opacity: '0' };
            for (const { id, ghost, rect } of blockRows) {
                // A toggle that interrupts another starts where its copy was.
                const was = seen.get(id);
                const from = !was ? (reveal ? shut : open) : reveal
                    ? { transform: `translateY(${was.rect.top - rect.top - drift}px)`, opacity: String(was.opacity) }
                    : { transform: 'translateY(0)', opacity: String(was.opacity) };
                newAnims.push(ghost.animate([from, reveal ? open : shut], timing));
                standIns.current.set(id, ghost);
            }
            if (run.anchor && !isThreadRow(run.anchor)) {
                newAnims.push(carryHairline(host, run.anchor, rollEnd, rowLeft,
                    reveal ? drift - roll : 0, reveal ? 0 : -(roll + drift), reveal, timing));
            }
        }

        // A capped roll leaves the rest of a long block out of the motion, so
        // every row below it moves only the roll too. Collapsing, such a row
        // starts the cut above its layout box, unless a running batch already
        // holds it higher. Expanding, it moves the roll and settles off screen.
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
        // A block whose line has scrolled above the screen does not roll, so
        // the rows below it do not slide either: that change lands at once. A
        // rolling block nearer above a row still moves it.
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

        // ── Departures: copies of rows that left the list. ──
        // A thread tucked into a collapsed section flies onto its header, over
        // the rows, and fades as it lands. Any other row folds up from its
        // bottom edge on the same curve as the row below, which closes the gap.
        // This runs before the rows below start sliding: a sliding header
        // would report its old box, not the one the copy must land on.
        if (departures.size > 0) {
            const leaving = [...departures].map(([id, ghost]) => ({ id, ghost, rect: oldRects.get(id)! }));
            clones.push(mountDepartureLayer(el, leaving));
            for (const { id, ghost, rect } of leaving) {
                const header = tuckTarget(id, sections);
                const target = header ? rowById.get(header)?.getBoundingClientRect() : undefined;
                if (target) {
                    const dy = target.top - rect.top;
                    ghost.style.background = 'var(--bg-primary)';
                    ghost.style.zIndex = '1';
                    newAnims.push(ghost.animate([
                        { transform: 'translateY(0) scale(1)', opacity: '1' },
                        LINGER,
                        { transform: `translateY(${dy}px) scale(0.92)`, opacity: '0' },
                    ], { duration: flightDurationMs(Math.abs(dy)) * scale, easing, fill: 'forwards' }));
                } else {
                    newAnims.push(ghost.animate([
                        { transform: 'scaleY(1)', transformOrigin: 'top', opacity: '1' },
                        LINGER,
                        { transform: 'scaleY(0)', transformOrigin: 'top', opacity: '0' },
                    ], { duration: MIN_MS * scale, easing, fill: 'forwards' }));
                }
            }
        }

        for (const row of rows) {
            const id = row.dataset.flipId!;
            if (revealed.has(id)) continue;
            const isNew = newItems.has(id);
            const isMoved = transitioned.has(id);
            const isAffected = affectedSiblings.has(id);

            if (isNew) {
                row.classList.add('flip-animating');
                touched.push(row);
                const isSection = id.startsWith('__section_');
                // On mobile: simpler fade-in without scale bounce
                const newItemKeyframes = (isSection || mobile)
                    ? [{ opacity: '0' }, { opacity: '1' }]
                    : [
                        { opacity: '0', transform: 'scale(0.95)', transformOrigin: 'center' },
                        { opacity: '1', transform: 'scale(1.03)', transformOrigin: 'center', offset: 0.6 },
                        { opacity: '1', transform: 'scale(1)', transformOrigin: 'center' },
                    ];
                const anim = row.animate(
                    newItemKeyframes,
                    { duration: (isSection ? 350 : 450) * scale, easing, fill: 'none' },
                );
                newAnims.push(anim);
                continue;
            }

            if (!isMoved && !isAffected) continue;
            if (!isMoved && belowStill(id)) continue;

            const newRect = row.getBoundingClientRect();
            const oldRect = oldRects.get(id);
            if (!oldRect) continue;

            const deltaX = oldRect.left - newRect.left;
            const deltaY = oldRect.top - newRect.top;
            const noMovement = Math.abs(deltaX) < 1 && Math.abs(deltaY) < 1;

            // Displaced siblings with no pixel movement: skip
            if (noMovement && !isMoved) continue;

            const dist = Math.sqrt(deltaX * deltaX + deltaY * deltaY);
            const baseDuration = noMovement
                ? MIN_MS  // Section-transitioned but same position: use min duration for scale-settle
                : flightDurationMs(dist);
            // A row a disclosure pushed moves in lockstep with the block.
            const pushed = disclosureDuration > 0 && !isMoved;
            const duration = pushed ? disclosureDuration : baseDuration * scale;

            if (isMoved) {
                // ── Portal-based flight for moved threads ──
                // 1. Clone the element into the portal (fixed position, above everything)
                const clone = row.cloneNode(true) as HTMLElement;
                clone.style.position = 'fixed';
                clone.style.top = `${oldRect.top}px`;
                clone.style.left = `${oldRect.left}px`;
                clone.style.width = `${oldRect.width}px`;
                clone.style.height = `${oldRect.height}px`;
                clone.style.margin = '0';
                clone.style.pointerEvents = 'none';
                clone.style.boxSizing = 'border-box';
                clone.style.borderBottom = '1px solid var(--border-color)';
                clone.style.background = 'var(--bg-primary)';
                clone.style.willChange = 'transform';
                portal.appendChild(clone);
                clones.push(clone);

                // 2. Hide the real element at its destination
                row.style.opacity = '0';
                hidden.push(row);

                // 3. Animate the clone from old position to new position
                // Use transform instead of top/left to avoid layout thrashing (GPU-composited)
                const flyDeltaX = newRect.left - oldRect.left;
                const flyDeltaY = newRect.top - oldRect.top;
                // On mobile: skip intermediate scale bounce for smoother animation
                const flyKeyframes = mobile
                    ? [
                        { transform: 'translate(0, 0)' },
                        { transform: `translate(${flyDeltaX}px, ${flyDeltaY}px)` },
                    ]
                    : [
                        { transform: 'translate(0, 0) scale(1)' },
                        { transform: `translate(${flyDeltaX}px, ${flyDeltaY}px) scale(1.03)`, offset: 0.7 },
                        { transform: `translate(${flyDeltaX}px, ${flyDeltaY}px) scale(1)` },
                    ];
                const anim = clone.animate(flyKeyframes, { duration, easing, fill: 'forwards' });
                newAnims.push(anim);
            } else {
                // Displaced sibling: slide to new position (stays in DOM)
                row.classList.add('flip-animating');
                touched.push(row);
                const { start, end } = cutBelow(id);
                const anim = row.animate([
                    { transform: `translate(${deltaX}px, ${deltaY + start}px)` },
                    { transform: `translate(0, ${end}px)` },
                ], { duration, easing: pushed ? EASING_DISCLOSURE : easing, fill: 'none' });
                newAnims.push(anim);
            }
        }

        runningAnims.current = newAnims;
        startTogetherOnNextFrame(newAnims, () => runningAnims.current === newAnims);
        animatedEls.current = touched;
        hiddenEls.current = hidden;
        portalClones.current = clones;

        // Cleanup after all animations finish
        if (newAnims.length > 0) {
            Promise.allSettled(newAnims.map(a => a.finished)).then(() => {
                // A newer batch cancelled these, which is what settled us, and
                // its takeover above already did this batch's cleanup. Running
                // it again would un-hide a row whose new clone is still flying,
                // and would wipe the refs the NEXT batch cancels through.
                if (runningAnims.current !== newAnims) return;
                for (const r of touched) {
                    r.classList.remove('flip-animating');
                }
                for (const h of hidden) {
                    h.style.opacity = '';
                }
                for (const c of clones) {
                    c.remove();
                }
                runningAnims.current = [];
                animatedEls.current = [];
                hiddenEls.current = [];
                portalClones.current = [];
                standIns.current.clear();
            });
        }
    });
}
