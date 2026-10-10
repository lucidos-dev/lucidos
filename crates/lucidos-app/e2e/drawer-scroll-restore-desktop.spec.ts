import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import type { Page } from '@playwright/test';
import { navigateToApp, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

/** Archived rows seeded. The initial window carries 30, so most of these are
 *  reachable only through `/threads/older` pagination. */
const ARCHIVED_ROWS = 150;

/** Rows that must be loaded before the test parks the list: well past the
 *  first window, so the saved position lies beyond what a reload loads. */
const LOADED_BEFORE_PARKING = 100;

function seedArchive(): void {
    const stamp = Date.now();
    const inserts: string[] = [];
    for (let i = 0; i < ARCHIVED_ROWS; i++) {
        const at = new Date(stamp - i * 60_000).toISOString();
        inserts.push(seedThreadRow({ id: randomUUID(), title: `archived-${i}-${stamp}`, now: at }));
    }
    psql(inserts.join(';\n'));
}

/** Title of the first row whose top edge sits inside the list's viewport. */
function topRowTitle(page: Page): Promise<string | null> {
    return page.evaluate(() => {
        const list = document.querySelector('.thread-drawer-list');
        if (!list) return null;
        const listTop = list.getBoundingClientRect().top;
        const rows = [...document.querySelectorAll<HTMLElement>('.thread-drawer-list .thread-row')];
        const first = rows.find(row => row.getBoundingClientRect().top >= listTop);
        return first?.querySelector('.thread-row-title')?.textContent ?? null;
    });
}

test.describe('Drawer scroll position across a reload', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        // Runs on the reload too, so it leaves the saved scroll key alone.
        await context.addInitScript(() => {
            localStorage.setItem('lucidos-thread-drawer-open', 'true');
            localStorage.removeItem('lucidos-drawer-collapsed');
            localStorage.removeItem('lucidos-drawer-grouping');
        });
        clearAllThreads();
        seedArchive();
    });

    test('lands back deep in Archive, past the rows a reload first loads', async ({ page }) => {
        await navigateToApp(page);
        const list = page.locator('.thread-drawer-list');
        const rows = page.locator('.thread-drawer-list .thread-row');
        await expect(rows.first()).toBeVisible();

        // Page older threads in by scrolling to the bottom until enough landed.
        await expect.poll(async () => {
            await list.evaluate(el => { el.scrollTop = el.scrollHeight; });
            return rows.count();
        }, { timeout: 30_000 }).toBeGreaterThanOrEqual(LOADED_BEFORE_PARKING);

        // Park the list deep in the loaded rows, with a real wheel so the
        // scroll memory records it.
        await list.hover();
        await list.evaluate(el => { el.scrollTop = el.scrollHeight - el.clientHeight - 1200; });
        await page.mouse.wheel(0, 40);
        await expect.poll(() => page.evaluate(() => localStorage.getItem('lucidos-scroll-thread-drawer')))
            .not.toBeNull();
        await page.waitForTimeout(400);
        const saved = Number(await page.evaluate(() => localStorage.getItem('lucidos-scroll-thread-drawer')));
        const before = await topRowTitle(page);
        expect(before).not.toBeNull();

        await page.reload({ waitUntil: 'load' });
        await expect(rows.first()).toBeVisible();

        await expect.poll(() => list.evaluate(el => Math.round(el.scrollTop)), { timeout: 20_000 })
            .toBeGreaterThan(saved - 3);
        expect(Math.abs(await list.evaluate(el => el.scrollTop) - saved)).toBeLessThanOrEqual(3);
        expect(await topRowTitle(page)).toBe(before);
    });
});
