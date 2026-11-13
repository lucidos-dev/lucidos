import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import type { Page } from '@playwright/test';
import { openThreadDrawer, assertHealthy, apiRequest, ensureMobileView, isMobileViewport, waitForPaneAtRest } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';
import {
    CURRENT_ROWS, MOUNTED_ROW_BOUND, seedCurrent, startProbe, stopProbe, mountedRows,
    sweepList, parkListAt, visibleRows, autostartProbeOnNextLoad, type SeededCurrent, openSeededList,
} from './drawer-row-window-probe';

/**
 * The drawer windows its Folders rows: only the rows near the viewport are in
 * the DOM, and a gap of their exact measured height stands in for the rest
 * (docs/plans/2026-10-11-drawer-row-window.md). These specs pin the two hard
 * requirements: nothing jumps, and nothing paints blank.
 */

const PREFS_SEEDED = 'row-window-prefs-seeded';

let seeded: SeededCurrent;

/** The Current header's text, which carries the section's thread count. */
function currentHeader(page: Page) {
    return page.locator('.thread-drawer-list [data-flip-id="__section_current"]');
}

test.describe('Drawer row window', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        // Once per test, not per load: the reload spec keeps its saved scroll.
        await context.addInitScript((flag) => {
            if (sessionStorage.getItem(flag)) return;
            sessionStorage.setItem(flag, '1');
            localStorage.setItem('lucidos-thread-drawer-open', 'true');
            localStorage.removeItem('lucidos-drawer-collapsed');
            localStorage.removeItem('lucidos-drawer-grouping');
            localStorage.removeItem('lucidos-scroll-thread-drawer');
            localStorage.removeItem('lucidos-focused-thread');
        }, PREFS_SEEDED);
        clearAllThreads();
        seeded = seedCurrent();
    });

    test('a long Current keeps only the rows near the viewport, and moving the window moves no row', async ({ page }) => {
        await openSeededList(page, seeded);
        expect(await mountedRows(page)).toBeLessThan(MOUNTED_ROW_BOUND);

        await startProbe(page);
        const sweep = await sweepList(page);
        const probe = await stopProbe(page);

        expect(sweep.mismatches).toEqual([]);
        expect(sweep.rowsSeen).toBe(CURRENT_ROWS);
        expect(sweep.peakRows).toBeLessThan(MOUNTED_ROW_BOUND);
        expect(probe.peakRows).toBeLessThan(MOUNTED_ROW_BOUND);
        expect(probe.blanks).toEqual([]);
    });

    test('threads arriving, leaving and changing far above hold the list as a full list would', async ({ page }) => {
        // Newer than every seeded row, so it lands at the top of Current.
        const arrivalId = randomUUID();
        psql(seedThreadRow({ id: arrivalId, title: `arrival-${Date.now()}`, now: new Date(Date.now() + 60_000).toISOString() }));
        await openSeededList(page, seeded);
        await parkListAt(page, 0.5);

        /** Run `change`, then require every visible row to have moved by one
         *  shared amount. A full list moves them too, by the height that
         *  arrived or left above, so only a spread between rows is a jump. */
        const expectListHeld = async (change: () => Promise<void>) => {
            const before = await visibleRows(page);
            expect(before.length).toBeGreaterThan(3);
            await startProbe(page);
            await change();
            // Past any FLIP slide the change set off.
            await page.waitForTimeout(800);
            const probe = await stopProbe(page);
            expect(probe.blanks).toEqual([]);
            const after = new Map((await visibleRows(page)).map(r => [r.id, r.top]));
            const shifts = before.filter(r => after.has(r.id)).map(r => after.get(r.id)! - r.top);
            expect(shifts.length).toBeGreaterThan(3);
            const spread = Math.max(...shifts) - Math.min(...shifts);
            expect(spread).toBeLessThan(0.5);
        };

        await expectListHeld(async () => {
            const res = await apiRequest(page).post('/api/v1/threads/unarchive', { data: { thread_ids: [arrivalId], with_sub_threads: false } });
            expect(res.ok()).toBeTruthy();
            await expect(currentHeader(page)).toContainText(String(CURRENT_ROWS + 1));
        });
        await expectListHeld(async () => {
            const res = await apiRequest(page).post('/api/v1/threads/archive', { data: { thread_id: seeded.ids[5] } });
            expect(res.ok()).toBeTruthy();
            await expect(currentHeader(page)).toContainText(String(CURRENT_ROWS));
        });
        const renamed = `renamed far above, long enough to wrap onto more lines than it had before ${Date.now()}`;
        await expectListHeld(async () => {
            const res = await apiRequest(page).post('/api/v1/threads/rename', { data: { thread_id: seeded.ids[10], title: renamed } });
            expect(res.ok()).toBeTruthy();
        });

        // Every gap still matches the rows it stands for, the renamed one too.
        const sweep = await sweepList(page);
        expect(sweep.mismatches).toEqual([]);
        await expect(page.locator('.thread-drawer-list .thread-row', { hasText: renamed })).toHaveCount(1);
    });

    test('collapsing and expanding a long Current rolls without mounting it whole', async ({ page }) => {
        await openSeededList(page, seeded);
        await currentHeader(page).click();
        await expect(currentHeader(page)).toHaveAttribute('aria-expanded', 'false');
        await page.waitForTimeout(800);

        await startProbe(page);
        await currentHeader(page).click();
        await expect(currentHeader(page)).toHaveAttribute('aria-expanded', 'true');
        await page.waitForTimeout(1000);
        const probe = await stopProbe(page);

        expect(probe.peakRows).toBeLessThan(MOUNTED_ROW_BOUND);
        expect(probe.blanks).toEqual([]);
        await expect(page.locator('.thread-drawer-list .thread-row', { hasText: seeded.titles[0] }).first()).toBeVisible();
    });

    test.describe('across a reload', () => {
        // `sw.js` answers every API GET, and `page.route` cannot see a request
        // the worker makes. Blocking it lets the spec hold the preference reads.
        test.use({ serviceWorkers: 'block' });

        test('a reload lands back on the same row deep in Current, without a blank frame', async ({ page }) => {
            // Preferences decide the backup reminder, which sits in the phone's
            // header and so in the list's spacer. Undismissed, it shows on every
            // load, and the reload below holds them until the rows are already in.
            let preferencesHeld: Promise<void> = Promise.resolve();
            await page.route('**/api/v1/preferences**', async (route) => {
                if (route.request().method() !== 'GET') return route.fallback();
                await preferencesHeld;
                const response = await route.fetch();
                const body = await response.json();
                delete body.preferences.backup_reminder_dismissed;
                await route.fulfill({ response, json: body });
            });
            await openSeededList(page, seeded);
            await expect(page.locator('.backup-reminder')).toBeVisible();
            await parkListAt(page, 0.6);
            const topRow = (await visibleRows(page))[0];
            // Past the scroll memory's save debounce.
            await page.waitForTimeout(1000);

            const saved = Number(await page.evaluate(() => localStorage.getItem('lucidos-scroll-thread-drawer')));
            expect(saved).toBeGreaterThan(0);

            let releasePreferences!: () => void;
            preferencesHeld = new Promise(resolve => { releasePreferences = resolve; });
            await autostartProbeOnNextLoad(page);
            await page.reload();
            // The list grows tall enough to hold the saved place before the
            // preferences land. A restore that lands now lands under a header still
            // short of its banner.
            await expect.poll(() => page.evaluate(() => {
                const list = document.querySelector('.thread-drawer-list');
                return list ? list.scrollHeight - list.clientHeight : -1;
            }), { timeout: 15_000 }).toBeGreaterThanOrEqual(saved);
            // Two frames, so a restore that takes the first chance has taken it.
            await page.evaluate(async () => {
                for (let i = 0; i < 2; i++) await new Promise(r => requestAnimationFrame(r));
            });
            releasePreferences();
            // Let the restore land before the phone's swipe back to the list. A
            // swipe is the reader's own action, which retires a restore in flight.
            // A banner arriving after the restore pushes the offset past `saved`
            // while the row stays put, so the row is what must match.
            await expect.poll(() => page.evaluate(() => document.querySelector('.thread-drawer-list')?.scrollTop ?? -1), { timeout: 15_000 })
                .toBeGreaterThanOrEqual(saved - 1);
            await openThreadDrawer(page);
            await expect.poll(async () => (await visibleRows(page).catch(() => []))[0]?.id, { timeout: 15_000 }).toBe(topRow.id);
            const probe = await stopProbe(page);
            expect(probe.blanks).toEqual([]);
            // Restoring renders the rows above the saved place once, then the
            // window folds them away again.
            await expect.poll(() => mountedRows(page)).toBeLessThan(MOUNTED_ROW_BOUND);
        });
    });

    test('a round trip through the Ongoing grouping lands back on the same row', async ({ page }) => {
        await openSeededList(page, seeded);
        await parkListAt(page, 0.6);
        const topRow = (await visibleRows(page))[0];
        // Past the scroll memory's save debounce.
        await page.waitForTimeout(1000);

        const groupingButton = page.locator('.grouping-btn').locator('visible=true');
        await groupingButton.click();
        await expect(page.locator('.ongoing-grouping')).toBeVisible();
        await startProbe(page);
        await groupingButton.click();
        await expect.poll(async () => (await visibleRows(page).catch(() => []))[0]?.id, { timeout: 15_000 }).toBe(topRow.id);
        const probe = await stopProbe(page);
        expect(probe.blanks).toEqual([]);
        await expect.poll(() => mountedRows(page)).toBeLessThan(MOUNTED_ROW_BOUND);
    });

    test('tapping a row leaves the list where it was', async ({ page }) => {
        await openSeededList(page, seeded);
        await parkListAt(page, 0.5);
        const before = await visibleRows(page);
        const target = before[Math.floor(before.length / 2)];

        await startProbe(page);
        await page.locator(`.thread-drawer-list .thread-row[data-thread-nav="${target.id}"]`).click();
        if (isMobileViewport(page)) {
            // The tap swipes to the thread; come back to the list.
            await waitForPaneAtRest(page);
            await ensureMobileView(page, 'threads');
            await waitForPaneAtRest(page);
        } else {
            await page.waitForTimeout(600);
        }
        const probe = await stopProbe(page);

        expect(probe.blanks).toEqual([]);
        const after = new Map((await visibleRows(page)).map(r => [r.id, r.top]));
        for (const row of before) expect(after.get(row.id), row.id).toBeCloseTo(row.top, 0);
    });

    test('a thread focused far down the list leaves the drawer at the top', async ({ page, context }) => {
        await context.addInitScript((id) => localStorage.setItem('lucidos-focused-thread', id), seeded.ids[350]);
        await openSeededList(page, seeded);
        await page.waitForTimeout(500);
        expect((await visibleRows(page))[0]?.id).toBe(seeded.ids[0]);
        expect(await mountedRows(page)).toBeLessThan(MOUNTED_ROW_BOUND);
    });
});
