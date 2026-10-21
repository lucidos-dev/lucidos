/**
 * Shared tools for the drawer's row window specs (`drawer-row-window*.spec.ts`).
 *
 * The probe runs in the page. Every animation frame, it looks for a gap inside
 * the list's viewport, which is a blank flash. It also records the peak number
 * of mounted rows. That runs in a MutationObserver, so a render undone before
 * paint still counts.
 */

import { expect, type Page } from '@playwright/test';
import { randomUUID } from 'crypto';
import { psql, seedThreadRow } from './db-helpers';
import { navigateToApp, openThreadDrawer } from './helpers';

/** Rows seeded into Current: the length the iPhone lagged at. */
export const CURRENT_ROWS = 400;

/** Mounted rows allowed at once: the viewport plus two viewports either side
 *  holds about 60 to 75 rows on every project, and pinned rows add one or
 *  two. Far under the 400 a full render mounts. */
export const MOUNTED_ROW_BOUND = 120;

export interface SeededCurrent {
    ids: string[];
    titles: string[];
}

/** Seed `count` Current threads, newest first. Every third title wraps onto a
 *  second line, so rows differ in height and a wrong gap shows. */
export function seedCurrent(count = CURRENT_ROWS): SeededCurrent {
    const stamp = Date.now();
    const ids: string[] = [];
    const titles: string[] = [];
    const inserts: string[] = [];
    for (let i = 0; i < count; i++) {
        const id = randomUUID();
        const title = i % 3 === 0
            ? `current-${i}-${stamp} with a title long enough to wrap onto a second line in the drawer`
            : `current-${i}-${stamp}`;
        ids.push(id);
        titles.push(title);
        const at = new Date(stamp - i * 60_000).toISOString();
        inserts.push(seedThreadRow({ id, title, now: at, archiveState: 'inbox' }));
    }
    psql(inserts.join(';\n'));
    return { ids, titles };
}

export interface ProbeReport {
    /** Frames that painted a gap inside the list's viewport. */
    blanks: { gapTop: number; gapBottom: number }[];
    peakRows: number;
}

export interface SweepReport {
    mismatches: { id: string; was: number; now: number; scrollTop: number }[];
    mismatchCount: number;
    rowsSeen: number;
    peakRows: number;
}

/** Session flag: a page that loads while it is set starts the probe at once. */
const PROBE_AUTOSTART_KEY = 'row-window-probe-autostart';

/** The in-page probe. Self-contained, so it can run as an init script too. */
function installProbe(autostartKey: string): void {
    const w = window as unknown as { __rowWindowProbe?: unknown };
    if (w.__rowWindowProbe) return;
    const visibleList = (): HTMLElement | null => {
        for (const el of document.querySelectorAll<HTMLElement>('.thread-drawer-list')) {
            const r = el.getBoundingClientRect();
            if (r.width > 0 && r.height > 0 && r.right > 1 && r.left < window.innerWidth - 1) return el;
        }
        return null;
    };
    const mountedRows = () =>
        visibleList()?.querySelectorAll('.drawer-section-rows > [data-flip-id]').length ?? 0;
    const gapsInView = () => {
        const list = visibleList();
        if (!list) return [];
        const l = list.getBoundingClientRect();
        const out: { gapTop: number; gapBottom: number }[] = [];
        for (const gap of list.querySelectorAll<HTMLElement>('.drawer-row-gap')) {
            const g = gap.getBoundingClientRect();
            if (g.bottom > l.top + 0.5 && g.top < l.bottom - 0.5) out.push({ gapTop: g.top - l.top, gapBottom: g.bottom - l.top });
        }
        return out;
    };
    const probe = {
        blanks: [] as { gapTop: number; gapBottom: number }[],
        peakRows: 0,
        running: false,
        observer: null as MutationObserver | null,
        start() {
            this.blanks = [];
            this.peakRows = mountedRows();
            this.running = true;
            this.observer?.disconnect();
            this.observer = new MutationObserver(() => { this.peakRows = Math.max(this.peakRows, mountedRows()); });
            this.observer.observe(document.documentElement, { childList: true, subtree: true });
            // Sampled in the task after each frame, which sees the DOM that
            // frame painted. A rAF callback runs before the paint instead.
            const tick = () => {
                if (!this.running) return;
                setTimeout(() => this.blanks.push(...gapsInView()), 0);
                requestAnimationFrame(tick);
            };
            requestAnimationFrame(tick);
        },
        stop() {
            this.running = false;
            this.observer?.disconnect();
            return { blanks: this.blanks.slice(0, 5), peakRows: this.peakRows };
        },
        mountedRows,
        visibleList,
        async sweep(stepFraction: number) {
            const list = visibleList()!;
            const twoFrames = () => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
            const seen = new Map<string, number>();
            const mismatches: { id: string; was: number; now: number; scrollTop: number }[] = [];
            let peakRows = 0;
            const check = () => {
                const l = list.getBoundingClientRect();
                for (const row of list.querySelectorAll<HTMLElement>('.drawer-section-rows > [data-flip-id]')) {
                    const id = row.dataset.flipId!;
                    const offset = row.getBoundingClientRect().top - l.top + list.scrollTop;
                    const was = seen.get(id);
                    if (was === undefined) seen.set(id, offset);
                    else if (Math.abs(was - offset) > 0.5) mismatches.push({ id, was, now: offset, scrollTop: list.scrollTop });
                }
                peakRows = Math.max(peakRows, mountedRows());
            };
            const step = list.clientHeight * stepFraction;
            // A list with no height steps by zero and never reaches its end. A
            // list the app replaced mid-sweep does the same. Either one hung the
            // page for the whole test timeout, so the sweep names it instead.
            if (!(step > 0)) throw new Error(`sweep: the list has no height (clientHeight ${list.clientHeight})`);
            const max = () => list.scrollHeight - list.clientHeight;
            const stillThere = (at: number) => {
                if (!list.isConnected) throw new Error(`sweep: the list was replaced while sweeping, at scrollTop ${at}`);
            };
            for (let top = 0; top <= max(); top += step) {
                list.scrollTop = top;
                await twoFrames();
                stillThere(top);
                check();
            }
            for (let top = max(); top >= 0; top -= step) {
                list.scrollTop = top;
                await twoFrames();
                stillThere(top);
                check();
            }
            list.scrollTop = 0;
            await twoFrames();
            check();
            return { mismatches: mismatches.slice(0, 5), mismatchCount: mismatches.length, rowsSeen: seen.size, peakRows };
        },
    };
    w.__rowWindowProbe = probe;
    // A reload under test starts sampling before the app's first paint.
    if (sessionStorage.getItem(autostartKey)) probe.start();
}

/** Start the probe in the next page load as early as its first paint. */
export async function autostartProbeOnNextLoad(page: Page): Promise<void> {
    await page.evaluate((key) => sessionStorage.setItem(key, '1'), PROBE_AUTOSTART_KEY);
}

type Probe = {
    start(): void;
    stop(): ProbeReport;
    mountedRows(): number;
    visibleList(): HTMLElement | null;
    sweep(stepFraction: number): Promise<SweepReport>;
};

/** Install the probe in the current page, and in every page this context
 *  loads after it (a reload). */
export async function installRowWindowProbe(page: Page): Promise<void> {
    await page.context().addInitScript(installProbe, PROBE_AUTOSTART_KEY);
    await page.evaluate(installProbe, PROBE_AUTOSTART_KEY);
}

export async function startProbe(page: Page): Promise<void> {
    await page.evaluate(() => (window as unknown as { __rowWindowProbe: Probe }).__rowWindowProbe.start());
}

export async function stopProbe(page: Page): Promise<ProbeReport> {
    return page.evaluate(() => (window as unknown as { __rowWindowProbe: Probe }).__rowWindowProbe.stop());
}

export async function mountedRows(page: Page): Promise<number> {
    return page.evaluate(() => (window as unknown as { __rowWindowProbe: Probe }).__rowWindowProbe.mountedRows());
}

/** Scroll the list down and back up in steps of `stepFraction` of a viewport.
 *  The report names every row seen at two content offsets. */
export async function sweepList(page: Page, stepFraction = 0.5): Promise<SweepReport> {
    return page.evaluate((f) => (window as unknown as { __rowWindowProbe: Probe }).__rowWindowProbe.sweep(f), stepFraction);
}

/** Open the drawer on the seeded list, with the probe installed. */
export async function openSeededList(page: Page, seeded: SeededCurrent): Promise<void> {
    await navigateToApp(page);
    await openThreadDrawer(page);
    await expect(page.locator('.thread-drawer-list .thread-row', { hasText: seeded.titles[0] }).first()).toBeVisible();
    await installRowWindowProbe(page);
}

/** Put the list's scroll position at `fraction` of its scroll range, and wait
 *  for the window to settle there. */
export async function parkListAt(page: Page, fraction: number): Promise<void> {
    await page.evaluate(async (f) => {
        const list = (window as unknown as { __rowWindowProbe: Probe }).__rowWindowProbe.visibleList()!;
        list.scrollTop = (list.scrollHeight - list.clientHeight) * f;
        for (let i = 0; i < 4; i++) await new Promise(r => requestAnimationFrame(r));
    }, fraction);
}

/** The rows whose box lies wholly inside the list's viewport: id and top
 *  edge, relative to the list. */
export async function visibleRows(page: Page): Promise<{ id: string; top: number }[]> {
    return page.evaluate(() => {
        const list = (window as unknown as { __rowWindowProbe: Probe }).__rowWindowProbe.visibleList()!;
        const l = list.getBoundingClientRect();
        return [...list.querySelectorAll<HTMLElement>('.drawer-section-rows > [data-flip-id]')]
            .map(row => ({ id: row.dataset.flipId!, r: row.getBoundingClientRect() }))
            .filter(({ r }) => r.top >= l.top && r.bottom <= l.bottom)
            .map(({ id, r }) => ({ id, top: r.top - l.top }));
    });
}
