/**
 * Perf debug tooling — locate the main-thread blocker behind any "button/drawer
 * clicks slow to register" interaction lag. Three observers:
 *  - Event Timing: each slow interaction split into input / handler / render,
 *    plus the target element.
 *  - Long Animation Frames (Chrome): names the script (invoker + source) that
 *    caused a long frame and splits script-time vs style/layout-time — the
 *    decisive "is it JS re-render or DOM layout?" signal.
 *  - Long tasks: fallback duration when LoAF is unavailable.
 *
 * Quiet by default: past one activation banner at startup, it logs only
 * at/above the thresholds, so a snappy workspace produces no further lines. No
 * `import.meta.env.DEV` gate: `web-dev.sh` serves a production-style
 * `vite build` where DEV is false.
 *
 * PERMANENT debug tooling (not a temporary measure). Built for the `click-lag`
 * investigation (resolved 2026-06-30 — see docs/temporary-measures.md), but
 * RETAINED rather than deleted: it is fully general perf instrumentation that
 * surfaces any future interaction lag, registered always and silent below its
 * thresholds, so no reactivation is needed.
 */

const INTERACTION_LOG_MS = 100;
const LOAF_LOG_MS = 120;
const LONGTASK_LOG_MS = 120;

/** Continuous pointer/hover/move events — noisy and not "clicks". Skipped in the
 *  interaction log so the signal (click / keydown / pointerdown→up) stands out. */
const SKIP_EVENTS = new Set([
    'pointerover', 'pointerout', 'pointerenter', 'pointerleave', 'pointermove',
    'mouseover', 'mouseout', 'mouseenter', 'mouseleave', 'mousemove',
]);

/** Compact, copy-pasteable description of an interaction's target element. */
function describeTarget(node: Node | null): string {
    const el = node && node.nodeType === 1 ? (node as Element) : null;
    if (!el) return '(no target)';
    const tag = el.tagName.toLowerCase();
    const id = el.id ? `#${el.id}` : '';
    const className = typeof el.className === 'string' ? el.className.trim() : '';
    const cls = className ? '.' + className.split(/\s+/).slice(0, 3).join('.') : '';
    const role = el.getAttribute('data-role');
    const roleAttr = role ? `[data-role=${role}]` : '';
    const txt = (el.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 24);
    return `${tag}${id}${cls}${roleAttr}${txt ? ` "${txt}"` : ''}`;
}

let started = false;

/** Register the observers. Idempotent; feature-detects so an unsupported browser
 *  (Safari has no longtask / LoAF) just skips that observer. */
export function startPerfProbe(): void {
    if (started || typeof PerformanceObserver === 'undefined') return;
    started = true;

    // Event Timing — split each interaction into input delay / handler / render.
    try {
        const obs = new PerformanceObserver((list) => {
            for (const entry of list.getEntries()) {
                const e = entry as PerformanceEventTiming;
                if (e.duration < INTERACTION_LOG_MS || SKIP_EVENTS.has(e.name)) continue;
                const inputDelay = e.processingStart - e.startTime;
                const handler = e.processingEnd - e.processingStart;
                const render = (e.startTime + e.duration) - e.processingEnd;
                const target = describeTarget((e as PerformanceEventTiming & { target?: Node | null }).target ?? null);
                // eslint-disable-next-line no-console
                console.warn(
                    `[perf-probe] ${e.name} ${Math.round(e.duration)}ms`
                    + ` (input ${Math.round(inputDelay)}ms · handler ${Math.round(handler)}ms · render ${Math.round(render)}ms)`
                    + ` ← ${target}`,
                );
            }
        });
        obs.observe({ type: 'event', durationThreshold: INTERACTION_LOG_MS, buffered: true } as PerformanceObserverInit);
    } catch { /* Event Timing unsupported — skip */ }

    // Long Animation Frames (Chrome 123+) — the decisive observer. Each entry
    // attributes the frame to the scripts that ran in it (invoker + source +
    // duration) and reports styleAndLayoutDuration separately, so we can tell a
    // JS re-render (script-heavy) from DOM layout/paint (style/layout-heavy).
    try {
        const obs = new PerformanceObserver((list) => {
            for (const entry of list.getEntries()) {
                const loaf = entry as PerformanceEntry & {
                    blockingDuration?: number;
                    styleAndLayoutDuration?: number;
                    scripts?: Array<{
                        invoker?: string; invokerType?: string;
                        sourceURL?: string; sourceFunctionName?: string;
                        sourceCharPosition?: number; duration?: number;
                        forcedStyleAndLayoutDuration?: number;
                    }>;
                };
                if (loaf.duration < LOAF_LOG_MS) continue;
                const block = Math.round(loaf.blockingDuration ?? 0);
                const sl = Math.round(loaf.styleAndLayoutDuration ?? 0);
                // eslint-disable-next-line no-console
                console.warn(`[perf-probe] LoAF ${Math.round(loaf.duration)}ms (blocking ${block}ms · style/layout ${sl}ms)`);
                const scripts = (loaf.scripts ?? [])
                    .slice()
                    .sort((a, b) => (b.duration ?? 0) - (a.duration ?? 0))
                    .slice(0, 3);
                for (const s of scripts) {
                    const src = s.sourceURL ? `${s.sourceURL}${s.sourceCharPosition != null ? `:${s.sourceCharPosition}` : ''}` : '(no source)';
                    // eslint-disable-next-line no-console
                    console.warn(
                        `[perf-probe]   script ${Math.round(s.duration ?? 0)}ms`
                        + ` ${s.invokerType ?? '?'}:${s.invoker ?? '?'}`
                        + ` ${s.sourceFunctionName || '(anon)'} @ ${src}`
                        + ` (forcedLayout ${Math.round(s.forcedStyleAndLayoutDuration ?? 0)}ms)`,
                    );
                }
            }
        });
        obs.observe({ type: 'long-animation-frame', buffered: true } as PerformanceObserverInit);
    } catch { /* LoAF unsupported — skip */ }

    // Long tasks — fallback duration when LoAF is unavailable.
    try {
        const obs = new PerformanceObserver((list) => {
            for (const e of list.getEntries()) {
                if (e.duration < LONGTASK_LOG_MS) continue;
                // eslint-disable-next-line no-console
                console.warn(`[perf-probe] longtask ${Math.round(e.duration)}ms`);
            }
        });
        obs.observe({ type: 'longtask', buffered: true } as PerformanceObserverInit);
    } catch { /* longtask unsupported (Safari) — skip */ }

    // eslint-disable-next-line no-console
    console.warn('[perf-probe] active (v2: LoAF) — reproduce a laggy click, then copy the [perf-probe] lines.');
}
