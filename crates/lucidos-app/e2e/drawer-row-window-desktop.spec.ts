import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';
import { assertHealthy, DRAWER_TOGGLE_LABEL } from './helpers';
import { clearAllThreads } from './db-helpers';
import {
    MOUNTED_ROW_BOUND, seedCurrent, startProbe, stopProbe, mountedRows,
    parkListAt, visibleRows, type SeededCurrent, openSeededList,
} from './drawer-row-window-probe';

/**
 * Navigation through the drawer's row window, on desktop: keyboard browsing,
 * Show in Folders and Search Everywhere land without a blank frame or a jump
 * (docs/plans/2026-10-11-drawer-row-window.md).
 */

let seeded: SeededCurrent;

/** The highlighted row: wholly inside the list's viewport, and named by the
 *  tree's `aria-activedescendant`. */
async function highlightState(page: Page) {
    return page.evaluate(() => {
        const tree = document.querySelector('.thread-drawer-tree');
        const activeId = tree?.getAttribute('aria-activedescendant') ?? null;
        const active = activeId ? document.querySelector(`[id="${activeId}"]`) : null;
        const row = document.querySelector<HTMLElement>('.thread-drawer-list .thread-row-highlighted');
        const list = document.querySelector<HTMLElement>('.thread-drawer-list')!;
        const r = row?.getBoundingClientRect();
        const l = list.getBoundingClientRect();
        return {
            id: row?.dataset.threadNav ?? null,
            activeExists: activeId === null || active !== null,
            inView: !!r && r.top >= l.top - 0.5 && r.bottom <= l.bottom + 0.5,
            scrollTop: list.scrollTop,
        };
    });
}

test.describe('Drawer row window navigation', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        await context.addInitScript(() => {
            localStorage.setItem('lucidos-thread-drawer-open', 'true');
            localStorage.removeItem('lucidos-drawer-collapsed');
            localStorage.removeItem('lucidos-drawer-grouping');
            localStorage.removeItem('lucidos-scroll-thread-drawer');
        });
        clearAllThreads();
        seeded = seedCurrent();
    });

    test('browsing down with the keyboard lands every row in view, with no blank frame', async ({ page }) => {
        await openSeededList(page, seeded);
        await page.locator('.thread-drawer-tree').focus();
        await startProbe(page);
        // The Current header, then 150 rows.
        let lastScroll = 0;
        for (let step = 0; step < 151; step++) {
            await page.keyboard.press('ArrowDown');
            const state = await highlightState(page);
            expect(state.activeExists, `step ${step}`).toBe(true);
            if (step === 0) continue;
            expect(state.id, `step ${step}`).toBe(seeded.ids[step - 1]);
            expect(state.inView, `step ${step}`).toBe(true);
            // Down only ever scrolls down: never a jump back.
            expect(state.scrollTop, `step ${step}`).toBeGreaterThanOrEqual(lastScroll);
            lastScroll = state.scrollTop;
        }
        const probe = await stopProbe(page);
        expect(probe.blanks).toEqual([]);
        expect(probe.peakRows).toBeLessThan(MOUNTED_ROW_BOUND);
    });

    test('a highlight scrolled far out of view stays in the DOM, and the next row lands in view', async ({ page }) => {
        await openSeededList(page, seeded);
        await page.locator('.thread-drawer-tree').focus();
        await page.keyboard.press('ArrowDown');
        await page.keyboard.press('ArrowDown');
        expect((await highlightState(page)).id).toBe(seeded.ids[0]);

        await parkListAt(page, 1);
        // The highlighted row is pinned, far above the window.
        expect((await highlightState(page)).activeExists).toBe(true);
        expect(await page.locator(`.thread-drawer-list .thread-row[data-thread-nav="${seeded.ids[0]}"]`).count()).toBe(1);

        await startProbe(page);
        await page.keyboard.press('ArrowDown');
        await expect.poll(async () => (await highlightState(page)).inView).toBe(true);
        await page.waitForTimeout(300);
        const probe = await stopProbe(page);
        const state = await highlightState(page);
        expect(state.id).toBe(seeded.ids[1]);
        expect(state.activeExists).toBe(true);
        expect(probe.blanks).toEqual([]);
    });

    test('Show in Folders lands a row far down the list centred, with no blank frame', async ({ page, context }) => {
        const target = seeded.ids[350];
        await context.addInitScript((id) => localStorage.setItem('lucidos-focused-thread', id), target);
        await openSeededList(page, seeded);
        await expect(page.locator('.thread-view-header')).toContainText(seeded.titles[350]);

        await startProbe(page);
        await page.locator('.thread-view-header .thread-title-menu').click();
        await page.getByRole('menuitem', { name: 'Show in Folders' }).click();
        const centred = () => page.evaluate((id) => {
            const row = document.querySelector(`.thread-drawer-list .thread-row[data-thread-nav="${id}"]`);
            const list = document.querySelector('.thread-drawer-list')!;
            if (!row) return Infinity;
            const r = row.getBoundingClientRect();
            const l = list.getBoundingClientRect();
            return Math.abs((r.top + r.bottom) / 2 - (l.top + l.bottom) / 2);
        }, target);
        await expect.poll(centred, { timeout: 5_000 }).toBeLessThan(2);
        await page.waitForTimeout(300);
        const probe = await stopProbe(page);
        expect(probe.blanks).toEqual([]);
        expect(await mountedRows(page)).toBeLessThan(MOUNTED_ROW_BOUND);
    });

    test('shutting and reopening the drawer lands back on the same row', async ({ page }) => {
        await openSeededList(page, seeded);
        await parkListAt(page, 0.6);
        const topRow = (await visibleRows(page))[0];
        // Past the scroll memory's save debounce.
        await page.waitForTimeout(1000);

        const toggle = page.locator(`.thread-toggle-slot button[aria-label^="${DRAWER_TOGGLE_LABEL}"]`);
        await toggle.click();
        await expect(page.locator('.thread-drawer.thread-drawer-collapsed')).toHaveCount(1);
        await page.waitForTimeout(600);
        await toggle.click();
        await startProbe(page);
        await expect.poll(async () => (await visibleRows(page).catch(() => []))[0]?.id, { timeout: 15_000 }).toBe(topRow.id);
        await page.waitForTimeout(300);
        const probe = await stopProbe(page);
        expect(probe.blanks).toEqual([]);
        await expect.poll(() => mountedRows(page)).toBeLessThan(MOUNTED_ROW_BOUND);
    });

    test('opening a far thread from Search Everywhere leaves the drawer still', async ({ page }) => {
        await openSeededList(page, seeded);
        await parkListAt(page, 0.2);
        const before = await visibleRows(page);

        await startProbe(page);
        await page.keyboard.press('ControlOrMeta+k');
        await page.locator('.search-everywhere-input').fill(seeded.titles[350]);
        await page.locator('.search-everywhere-result', { hasText: seeded.titles[350] }).first().click();
        await expect(page.locator('.thread-view-header')).toContainText(seeded.titles[350]);
        await page.waitForTimeout(500);
        const probe = await stopProbe(page);

        expect(probe.blanks).toEqual([]);
        const after = new Map((await visibleRows(page)).map(r => [r.id, r.top]));
        for (const row of before) expect(after.get(row.id), row.id).toBeCloseTo(row.top, 0);
    });
});
