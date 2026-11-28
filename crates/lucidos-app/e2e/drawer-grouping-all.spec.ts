import { test, expect, type Page, type Locator } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy, isMobileViewport } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

const SHOOTING = process.env.DRAWER_SHOTS === '1';
const SHOTS_DIR = 'test-results/drawer-shots';

const BLOCKED_COUNT = 30;

const TILES = ['Blocked', 'Review', 'Drafts', 'In flight'];

function seed(stamp: number): void {
    const base = Date.now();
    const draftId = randomUUID();
    const runningId = randomUUID();
    const rows = [
        ...Array.from({ length: BLOCKED_COUNT }, (_, i) => seedThreadRow({
            id: randomUUID(), title: `blocked-${i}-${stamp}`, now: new Date(base - i * 60_000).toISOString(), status: 'failed', archiveState: 'inbox',
        })),
        seedThreadRow({ id: draftId, title: `draft-${stamp}`, now: new Date(base - 3_000_000).toISOString(), archiveState: 'inbox' }),
        `UPDATE thread_summaries SET compose_text = 'half a thought' WHERE thread_id = '${draftId}'`,
        seedThreadRow({ id: runningId, title: `running-${stamp}`, now: new Date(base - 3_300_000).toISOString(), archiveState: 'inbox' }),
        `UPDATE thread_summaries SET status = 'running' WHERE thread_id = '${runningId}'`,
        seedThreadRow({ id: randomUUID(), title: `archived-${stamp}`, now: new Date(base - 3_600_000).toISOString() }),
    ];
    psql(rows.join(';\n'));
}

function visible(page: Page, selector: string): Locator {
    return page.locator(`.thread-drawer-body > .thread-drawer-list ${selector}`).locator('visible=true');
}

function groupingButton(page: Page): Locator {
    return page.locator('.grouping-btn').locator('visible=true');
}

function paneTitle(page: Page): Locator {
    return page.locator('.threads-header-title, .mobile-threads-header .mobile-header-title').locator('visible=true');
}

function header(page: Page, label: string): Locator {
    return visible(page, '.list-section-title-collapsible')
        .filter({ has: page.locator('.section-label', { hasText: new RegExp(`^${label}$`) }) });
}

function tile(page: Page, label: string): Locator {
    return visible(page, '.drawer-ongoing-tiles .ongoing-tile')
        .filter({ has: page.locator('.ongoing-tile-label', { hasText: new RegExp(`^${label}$`) }) });
}

async function tileLabels(page: Page): Promise<string[]> {
    return visible(page, '.drawer-ongoing-tiles .ongoing-tile-label').allTextContents();
}

function rowTitles(page: Page): Locator {
    return visible(page, '.ongoing-grouping [data-thread-nav] .thread-row-title');
}

async function shoot(page: Page, name: string): Promise<void> {
    if (!SHOOTING) return;
    await page.waitForTimeout(800);
    const prefix = isMobileViewport(page) ? 'phone' : 'desktop';
    const box = isMobileViewport(page)
        ? undefined
        : await page.locator('.thread-drawer:not(.thread-drawer-collapsed)').boundingBox();
    await page.screenshot({
        path: `${SHOTS_DIR}/${prefix}-${name}.png`,
        clip: box ? { x: 0, y: 0, width: box.x + box.width, height: page.viewportSize()!.height } : undefined,
    });
}

test.describe('Drawer grouping', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        await context.addInitScript(() => {
            if (sessionStorage.getItem('drawer-grouping-reset')) return;
            sessionStorage.setItem('drawer-grouping-reset', '1');
            localStorage.removeItem('lucidos-drawer-grouping');
            localStorage.removeItem('lucidos-drawer-selected-ongoing-group');
            localStorage.removeItem('lucidos-drawer-collapsed');
            localStorage.removeItem('lucidos-thread-filter-panel-open');
        });
        clearAllThreads();
        seed(Date.now());
        await navigateToApp(page);
        await openThreadDrawer(page);
    });

    test('the header button swaps between Folders and Ongoing', async ({ page }) => {
        await expect(paneTitle(page)).toHaveText('Folders');
        await expect(header(page, 'Current')).toBeVisible();
        await expect(page.locator('.filter-slot[data-shown]').locator('visible=true')).toHaveCount(1);
        await expect(groupingButton(page)).toHaveAttribute('aria-label', `Show Ongoing (${BLOCKED_COUNT} blocked)`);
        await expect(groupingButton(page).locator('.badge')).toHaveText(String(BLOCKED_COUNT));
        await expect(page.locator('.thread-drawer [role="radiogroup"]')).toHaveCount(0);
        await shoot(page, 'folders');

        const before = await groupingButton(page).boundingBox();
        await groupingButton(page).click();
        await expect(paneTitle(page)).toHaveText('Ongoing');
        await expect(groupingButton(page)).toHaveAttribute('aria-label', 'Show Folders');
        await expect(groupingButton(page).locator('.badge')).toHaveCount(0);
        await expect.poll(() => tileLabels(page)).toEqual(TILES);
        await expect(header(page, 'Current')).toHaveCount(0);
        await expect(page.locator('.filter-slot:not([data-shown])')).not.toHaveCount(0);
        expect(await groupingButton(page).boundingBox()).toEqual(before);
        await shoot(page, 'ongoing-closed');

        await groupingButton(page).click();
        await expect(header(page, 'Current')).toBeVisible();
    });

    test('Ongoing lists only the selected tile\'s group', async ({ page }) => {
        await groupingButton(page).click();
        await expect(tile(page, 'Blocked')).toHaveAttribute('aria-current', 'true');
        await expect(tile(page, 'Blocked').locator('.ongoing-tile-count')).toHaveText(String(BLOCKED_COUNT));
        await expect(rowTitles(page)).toHaveCount(BLOCKED_COUNT);
        await expect(rowTitles(page).first()).toHaveText(/^blocked-/);
        await expect(tile(page, 'Review')).toHaveClass(/ongoing-tile-empty/);
        await shoot(page, 'ongoing-blocked');

        await tile(page, 'Drafts').click();
        await expect(tile(page, 'Drafts')).toHaveAttribute('aria-current', 'true');
        await expect(visible(page, '.ongoing-tile[aria-current="true"]')).toHaveCount(1);
        await expect(rowTitles(page)).toHaveCount(1);
        await expect(rowTitles(page)).toHaveText(/^draft-/);
        await expect(visible(page, '.ongoing-grouping [data-thread-nav].thread-row-focused')).toHaveCount(0);
        await shoot(page, 'ongoing-drafts');

        await tile(page, 'Review').click();
        await expect(rowTitles(page)).toHaveCount(0);
        await expect(visible(page, '.ongoing-grouping .empty-state')).toHaveText('Nothing to review');
    });

    test('desktop keeps the tiles in view while the rows scroll; the phone scrolls them', async ({ page }) => {
        await groupingButton(page).click();
        await expect(tile(page, 'Blocked')).toHaveAttribute('aria-current', 'true');
        const scroller = page.locator('.thread-drawer-list').locator('visible=true');
        await expect(rowTitles(page)).toHaveCount(BLOCKED_COUNT);
        await scroller.evaluate(el => { el.scrollTop = (el.scrollHeight - el.clientHeight) / 2; });
        await expect.poll(() => scroller.evaluate(el => el.scrollTop)).toBeGreaterThan(200);
        const view = (await scroller.boundingBox())!;
        const inView = (box: { y: number; height: number }) =>
            box.y >= view.y - 1 && box.y + box.height <= view.y + view.height + 1;
        const phone = isMobileViewport(page);
        for (const label of TILES) {
            const box = await tile(page, label).boundingBox();
            expect(box, label).not.toBeNull();
            expect(inView(box!), `${label} in view`).toBe(!phone);
        }
        await shoot(page, 'ongoing-scrolled');
    });

    test('the grouping and the selected group survive a reload', async ({ page }) => {
        await groupingButton(page).click();
        await tile(page, 'Drafts').click();
        await expect(tile(page, 'Drafts')).toHaveAttribute('aria-current', 'true');
        await page.reload();
        await openThreadDrawer(page);
        await expect(paneTitle(page)).toHaveText('Ongoing');
        await expect(tile(page, 'Drafts')).toHaveAttribute('aria-current', 'true');
    });
});
