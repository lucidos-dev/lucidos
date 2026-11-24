import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import type { Page } from '@playwright/test';
import { navigateToApp, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

const ARCHIVED_ROWS = 150;

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

        await expect.poll(() => list.evaluate((el) => {
            el.scrollTop = el.scrollHeight;
            const w = window as unknown as { __rowsSeen?: Set<string> };
            w.__rowsSeen ??= new Set();
            for (const row of el.querySelectorAll<HTMLElement>('.thread-row[data-thread-nav]')) w.__rowsSeen.add(row.dataset.threadNav!);
            return w.__rowsSeen.size;
        }), { timeout: 30_000 }).toBeGreaterThanOrEqual(LOADED_BEFORE_PARKING);

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
