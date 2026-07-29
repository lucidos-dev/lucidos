import { useRef, useLayoutEffect } from 'preact/hooks';
import { speedMultiplier } from '../store/store';
import { prefersReducedMotion } from '../utils/platform';
import { isMobile } from '../utils/viewport';

interface Rect { top: number; left: number; width: number; height: number }

const EASING_DESKTOP = 'cubic-bezier(0.22, 0, 0, 1)';
const EASING_MOBILE = 'cubic-bezier(0.25, 0.1, 0.25, 1)'; // simpler ease for mobile perf
const PX_PER_SEC = 600;
const MIN_MS = 250;
const MAX_MS = 450;
const MOBILE_DURATION_SCALE = 0.6; // shorter animations on mobile

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
 * Speed setting: slider 0 = normal (1x), +10 = 10x faster, -10 = 10x slower.
 */
export function useFlipTransitions(
    containerRef: { current: HTMLElement | null },
    portalRef: { current: HTMLElement | null },
    sections: { name: string; ids: string[] }[],
    resetKey?: unknown,
) {
    const prevResetKey = useRef<unknown>(resetKey);
    const resetted = prevResetKey.current !== resetKey;
    prevResetKey.current = resetKey;
    const prevSections = useRef<Map<string, string>>(new Map());
    const prevOrder = useRef<string[]>([]);
    const runningAnims = useRef<Animation[]>([]);
    const animatedEls = useRef<HTMLElement[]>([]);
    const hiddenEls = useRef<HTMLElement[]>([]);
    const portalClones = useRef<HTMLElement[]>([]);

    // ── Build flat ordered list and section map ──
    const currentOrder: string[] = [];
    const currentSections = new Map<string, string>();
    for (const section of sections) {
        for (const id of section.ids) {
            currentOrder.push(id);
            currentSections.set(id, section.name);
        }
    }

    // ── Detect transitions and new items ──
    const transitioned = new Set<string>();
    const newItems = new Set<string>();
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
    }

    // ── Build affected siblings: any existing item that isn't moved/new ──
    // When sections appear/disappear, index-range heuristics miss items outside
    // the source-dest range (e.g. a section header below the moved thread).
    // Instead, mark ALL surviving items as potential siblings — the pixel-delta
    // check later (line ~177) skips items that didn't actually move.
    const affectedSiblings = new Set<string>();
    const hasChanges = transitioned.size > 0 || newItems.size > 0;
    if (hasChanges && prevOrder.current.length > 0) {
        for (const id of currentOrder) {
            if (!transitioned.has(id) && !newItems.has(id)) {
                affectedSiblings.add(id);
            }
        }
    }

    // ── Capture OLD positions from the still-current DOM ──
    const oldRects = new Map<string, Rect>();
    const container = containerRef.current;
    if (container && hasChanges) {
        const rows = container.querySelectorAll<HTMLElement>('[data-flip-id]');
        for (const row of rows) {
            const id = row.dataset.flipId!;
            const r = row.getBoundingClientRect();
            oldRects.set(id, { top: r.top, left: r.left, width: r.width, height: r.height });
        }
    }

    // ── Update refs for next render ──
    prevSections.current = currentSections;
    prevOrder.current = currentOrder;

    useLayoutEffect(() => {
        const el = containerRef.current;
        const portal = portalRef.current;
        if (!el || !portal || !hasChanges || oldRects.size === 0) return;

        if (prefersReducedMotion()) return;

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

        const speed = speedMultiplier.value;
        const mobile = isMobile();
        const durationScale = (1 / speed) * (mobile ? MOBILE_DURATION_SCALE : 1);
        const easing = mobile ? EASING_MOBILE : EASING_DESKTOP;

        const rows = el.querySelectorAll<HTMLElement>('[data-flip-id]');
        const newAnims: Animation[] = [];
        const touched: HTMLElement[] = [];
        const hidden: HTMLElement[] = [];
        const clones: HTMLElement[] = [];

        for (const row of rows) {
            const id = row.dataset.flipId!;
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
                    { duration: (isSection ? 350 : 450) * durationScale, easing, fill: 'none' },
                );
                newAnims.push(anim);
                continue;
            }

            if (!isMoved && !isAffected) continue;

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
                : Math.max(MIN_MS, Math.min(MAX_MS, dist / PX_PER_SEC * 1000));
            const duration = baseDuration * durationScale;

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
                const anim = row.animate([
                    { transform: `translate(${deltaX}px, ${deltaY}px)` },
                    { transform: 'translate(0, 0)' },
                ], { duration, easing, fill: 'none' });
                newAnims.push(anim);
            }
        }

        runningAnims.current = newAnims;
        animatedEls.current = touched;
        hiddenEls.current = hidden;
        portalClones.current = clones;

        // Cleanup after all animations finish
        if (newAnims.length > 0) {
            Promise.allSettled(newAnims.map(a => a.finished)).then(() => {
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
            });
        }
    });
}
