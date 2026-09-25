import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

/** One frame after the Archive tap: every copy standing in for the row. */
interface Frame { t: number; copies: { top: number; bottom: number; left: number; opacity: number }[] }

async function archiveAndRecord(page: import('@playwright/test').Page, id: string): Promise<Frame[]> {
    await page.locator(`.thread-row[data-thread-nav="${id}"]`).click({ button: 'right' });
    const item = page.getByRole('menuitem', { name: 'Archive' });
    await expect(item).toBeVisible();
    await item.evaluate(el => { (window as unknown as { __archiveItem: HTMLElement }).__archiveItem = el; });
    return page.evaluate(async () => {
        const selector = '.flip-departure-layer > *, .flip-portal > *';
        (window as unknown as { __archiveItem: HTMLElement }).__archiveItem.click();
        const frames: Frame[] = [];
        const start = performance.now();
        while (performance.now() - start < 800) {
            await new Promise(resolve => requestAnimationFrame(resolve));
            frames.push({
                t: Math.round(performance.now() - start),
                copies: [...document.querySelectorAll<HTMLElement>(selector)].map(el => {
                    const r = el.getBoundingClientRect();
                    return { top: r.top, bottom: r.bottom, left: r.left, opacity: Number(getComputedStyle(el).opacity) };
                }),
            });
        }
        return frames;
    });
}

test.describe('Drawer archive departure', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        await context.clearCookies();
        clearAllThreads();
    });

    for (const archiveCollapsed of [false, true]) {
        test(`an archived row visibly leaves (Archive ${archiveCollapsed ? 'collapsed' : 'open'})`, async ({ page, context }) => {
            await context.addInitScript((collapsed) => {
                localStorage.setItem('lucidos-drawer-collapsed', JSON.stringify(collapsed ? ['archive'] : []));
            }, archiveCollapsed);
            const stamp = Date.now();
            const current = Array.from({ length: 4 }, () => randomUUID());
            const archived = randomUUID();
            const now = new Date().toISOString();
            psql([
                ...current.map((id, i) => seedThreadRow({ id, title: `current-${i}-${stamp}`, now })),
                seedThreadRow({ id: archived, title: `archived-${stamp}`, now }),
                `UPDATE thread_summaries SET archive_state = 'inbox' WHERE thread_id IN ('${current.join("','")}')`,
            ].join(';\n'));

            await navigateToApp(page);
            await openThreadDrawer(page);
            const target = current[1];
            await expect(page.locator(`.thread-row[data-thread-nav="${target}"]`)).toBeVisible();

            const frames = await archiveAndRecord(page, target);
            const trace = JSON.stringify(frames.filter(f => f.copies.length > 0));
            // Archiving also opens the next thread, which can hold the first
            // paint back. The exit must start on the frame that paints it,
            // rather than being half over by then.
            const first = frames.find(f => f.copies.length > 0);
            expect(first, trace).toBeDefined();
            expect(Math.max(...first!.copies.map(c => c.opacity)), trace).toBeGreaterThan(0.9);
            // And it stays on screen long enough to read.
            const height = page.viewportSize()!.height;
            const visible = frames.filter(f => f.copies.some(c => c.opacity > 0.05 && c.bottom > 0 && c.top < height));
            expect(visible[visible.length - 1].t - visible[0].t, trace).toBeGreaterThanOrEqual(150);
        });
    }
});
