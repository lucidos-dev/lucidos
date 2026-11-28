
import { expect, type Page } from '@playwright/test';
import { randomUUID } from 'crypto';
import { psql, seedThreadRow } from './db-helpers';
import { navigateToApp, openThreadDrawer } from './helpers';

export const CURRENT_ROWS = 400;

export const MOUNTED_ROW_BOUND = 120;

export interface SeededCurrent {
    ids: string[];
    titles: string[];
}

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
    blanks: { gapTop: number; gapBottom: number }[];
    peakRows: number;
}

export interface SweepReport {
    mismatches: { id: string; was: number; now: number; scrollTop: number }[];
    mismatchCount: number;
    rowsSeen: number;
    peakRows: number;
}

const PROBE_AUTOSTART_KEY = 'row-window-probe-autostart';

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
            const frame = () => new Promise<void>((resolve, reject) => {
                const stalled = setTimeout(() => reject(new Error(
                    `sweep: no frame in 5 s at scrollTop ${list.scrollTop} of ${max()} (scrollHeight ${list.scrollHeight}, ` +
                    `${mountedRows()} rows mounted, ${steps} steps taken, connected ${list.isConnected}, ` +
                    `visibility ${document.visibilityState})`,
                )), 5000);
                requestAnimationFrame(() => { clearTimeout(stalled); resolve(); });
            });
            const twoFrames = async () => { await frame(); await frame(); };
            let steps = 0;
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
            if (!(step > 0)) throw new Error(`sweep: the list has no height (clientHeight ${list.clientHeight})`);
            const max = () => list.scrollHeight - list.clientHeight;
            const stepLimit = Math.ceil(max() / step) * 4 + 10;
            const counted = () => {
                if (++steps > stepLimit) throw new Error(`sweep: over ${stepLimit} steps, the list keeps growing (scrollHeight ${list.scrollHeight})`);
            };
            const stillThere = (at: number) => {
                if (!list.isConnected) throw new Error(`sweep: the list was replaced while sweeping, at scrollTop ${at}`);
            };
            for (let top = 0; top <= max(); top += step) {
                counted();
                list.scrollTop = top;
                await twoFrames();
                stillThere(top);
                check();
            }
            for (let top = max(); top >= 0; top -= step) {
                counted();
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
    if (sessionStorage.getItem(autostartKey)) probe.start();
}

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

export async function sweepList(page: Page, stepFraction = 0.5): Promise<SweepReport> {
    return page.evaluate((f) => (window as unknown as { __rowWindowProbe: Probe }).__rowWindowProbe.sweep(f), stepFraction);
}

export async function openSeededList(page: Page, seeded: SeededCurrent): Promise<void> {
    await navigateToApp(page);
    await openThreadDrawer(page);
    await expect(page.locator('.thread-drawer-list .thread-row', { hasText: seeded.titles[0] }).first()).toBeVisible();
    await installRowWindowProbe(page);
}

export async function parkListAt(page: Page, fraction: number): Promise<void> {
    await page.evaluate(async (f) => {
        const list = (window as unknown as { __rowWindowProbe: Probe }).__rowWindowProbe.visibleList()!;
        list.scrollTop = (list.scrollHeight - list.clientHeight) * f;
        for (let i = 0; i < 4; i++) await new Promise(r => requestAnimationFrame(r));
    }, fraction);
}

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
