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
 * TWO SINKS, and the second is the one a phone has. Every line goes to
 * `console.warn` AND to `recordPerfSample`, which reaches the engine's log as a
 * `[Client/perf]` line (`docs/client-logs.md` says which file). An iOS PWA offers no console at all, so console-only
 * output made this module unreadable on the device that most needed it. The
 * queue's own default-off gate decides whether a sample is kept. The console
 * half is what a desktop session gets free; the queue half is what a
 * switched-on phone reports. See
 * docs/plans/2026-09-20-the-phone-can-report-what-blocks-a-navigation.md.
 *
 * On WebKit only Event Timing may register: LoAF and longtask are Chrome-only.
 * `main-thread-stall` in utils/mainThreadStall.ts is the substitute, and
 * `perf-probe-support` below says which observers this browser actually took.
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

import { onPerfEnabledChange, recordPerfSample } from './perfQueue';

const INTERACTION_LOG_MS = 100;
const LOAF_LOG_MS = 120;
const LONGTASK_LOG_MS = 120;

const SKIP_EVENTS = new Set([
    'pointerover', 'pointerout', 'pointerenter', 'pointerleave', 'pointermove',
    'mouseover', 'mouseout', 'mouseenter', 'mouseleave', 'mousemove',
]);

export function targetStructure(node: Node | null): string {
    const el = node && node.nodeType === 1 ? (node as Element) : null;
    if (!el) return '(no target)';
    const tag = el.tagName.toLowerCase();
    const className = typeof el.className === 'string' ? el.className.trim() : '';
    const cls = className ? '.' + className.split(/\s+/).slice(0, 3).join('.') : '';
    const role = el.getAttribute('data-role');
    const roleAttr = role ? `[data-role=${role}]` : '';
    return `${tag}${cls}${roleAttr}`;
}

function targetText(node: Node | null): string {
    const el = node && node.nodeType === 1 ? (node as Element) : null;
    if (!el) return '';
    return (el.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 24);
}

function describeTarget(node: Node | null): string {
    const txt = targetText(node);
    return `${targetStructure(node)}${txt ? ` "${txt}"` : ''}`;
}

export interface InteractionSample {
    name: string;
    durationMs: number;
    inputMs: number;
    handlerMs: number;
    renderMs: number;
    target: string;
}

export function interactionSampleOf(e: {
    name: string;
    duration: number;
    startTime: number;
    processingStart: number;
    processingEnd: number;
    target?: Node | null;
}): InteractionSample {
    return {
        name: e.name,
        durationMs: Math.round(e.duration),
        inputMs: Math.round(e.processingStart - e.startTime),
        handlerMs: Math.round(e.processingEnd - e.processingStart),
        renderMs: Math.round((e.startTime + e.duration) - e.processingEnd),
        target: targetStructure(e.target ?? null),
    };
}

let started = false;
let unsubscribeGate: (() => void) | null = null;

function observeType(
    type: string,
    init: PerformanceObserverInit & { durationThreshold?: number },
    handle: (list: PerformanceObserverEntryList) => void,
): boolean {
    try {
        const obs = new PerformanceObserver(handle);
        obs.observe({ type, ...init } as PerformanceObserverInit);
        return true;
    } catch {
        return false;
    }
}

export function startPerfProbe(): void {
    if (started || typeof PerformanceObserver === 'undefined') return;
    started = true;

    const armed: string[] = [];

    const gotEvent = observeType('event', { durationThreshold: INTERACTION_LOG_MS, buffered: true }, (list) => {
        for (const entry of list.getEntries()) {
            const e = entry as PerformanceEventTiming;
            if (e.duration < INTERACTION_LOG_MS || SKIP_EVENTS.has(e.name)) continue;
            const target = (e as PerformanceEventTiming & { target?: Node | null }).target ?? null;
            const s = interactionSampleOf(e as PerformanceEventTiming & { target?: Node | null });
            // eslint-disable-next-line no-console
            console.warn(
                `[perf-probe] ${s.name} ${s.durationMs}ms`
                + ` (input ${s.inputMs}ms · handler ${s.handlerMs}ms · render ${s.renderMs}ms)`
                + ` ← ${describeTarget(target)}`,
            );
            recordPerfSample('interaction', { ...s });
        }
    });
    if (gotEvent) armed.push('event');

    const gotLoaf = observeType('long-animation-frame', { buffered: true }, (list) => {
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
            const worst = scripts[0];
            recordPerfSample('loaf', {
                durationMs: Math.round(loaf.duration),
                blockingMs: block,
                styleLayoutMs: sl,
                script: worst && {
                    ms: Math.round(worst.duration ?? 0),
                    invoker: `${worst.invokerType ?? '?'}:${worst.invoker ?? '?'}`,
                    fn: worst.sourceFunctionName || '(anon)',
                    forcedLayoutMs: Math.round(worst.forcedStyleAndLayoutDuration ?? 0),
                },
            });
        }
    });
    if (gotLoaf) armed.push('long-animation-frame');

    const gotLongtask = observeType('longtask', { buffered: true }, (list) => {
        for (const e of list.getEntries()) {
            if (e.duration < LONGTASK_LOG_MS) continue;
            // eslint-disable-next-line no-console
            console.warn(`[perf-probe] longtask ${Math.round(e.duration)}ms`);
            recordPerfSample('longtask', { durationMs: Math.round(e.duration) });
        }
    });
    if (gotLongtask) armed.push('longtask');

    recordSupport(armed);
    unsubscribeGate = onPerfEnabledChange((on) => { if (on) recordSupport(armed); });

    // eslint-disable-next-line no-console
    console.warn(`[perf-probe] active (v3: engine-log). Armed: ${armed.join(', ') || 'none'}`);
}

function recordSupport(armed: string[]): void {
    recordPerfSample('perf-probe-support', {
        armed,
        supported: supportedEntryTypes(),
    });
}

function supportedEntryTypes(): string[] {
    const observer = PerformanceObserver as unknown as { supportedEntryTypes?: string[] };
    return Array.isArray(observer.supportedEntryTypes) ? observer.supportedEntryTypes : [];
}

export function _resetPerfProbeForTesting(): void {
    started = false;
    unsubscribeGate?.();
    unsubscribeGate = null;
}
