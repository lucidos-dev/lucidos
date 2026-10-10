import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import type { Page } from '@playwright/test';
import { navigateToApp, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

/** Seed a parent and child between 40 newer and 40 older threads. The child's
 *  row then sits mid-list, where reaching it takes a scroll that is not pinned
 *  to either end. */
function seedFamilyAmongFillers(): { parentId: string; childId: string; stamp: number } {
    const stamp = Date.now();
    const parentId = randomUUID();
    const childId = randomUUID();
    // Current sorts newest first, so the family lands between the two runs.
    const old = new Date(stamp - 86_400_000).toISOString();
    const rows = [
        seedThreadRow({ id: parentId, title: `reveal-parent-${stamp}`, totalChildren: 1, now: old, archiveState: 'inbox' }),
        seedThreadRow({ id: childId, title: `reveal-child-${stamp}`, parentId, now: old, archiveState: 'inbox' }),
    ];
    for (let i = 0; i < 40; i++) {
        const newer = new Date(stamp - i * 60_000).toISOString();
        const older = new Date(stamp - 2 * 86_400_000 - i * 60_000).toISOString();
        rows.push(seedThreadRow({ id: randomUUID(), title: `newer-${i}-${stamp}`, now: newer, archiveState: 'inbox' }));
        rows.push(seedThreadRow({ id: randomUUID(), title: `older-${i}-${stamp}`, now: older, archiveState: 'inbox' }));
    }
    psql(rows.join(';\n'));
    return { parentId, childId, stamp };
}

/** Pick Show in Folders from the title bar's thread menu. Then require the
 *  child row to be the focused row, inside the list's own viewport. */
async function showInFoldersAndExpectRowInView(page: Page, childId: string, stamp: number): Promise<void> {
    const header = page.locator('.thread-view-header');
    await expect(header).toContainText(`reveal-child-${stamp}`);

    await header.locator('.thread-title-menu').click();
    await page.getByRole('menuitem', { name: 'Show in Folders' }).click();

    const childRow = page.locator(`.thread-drawer-list .thread-row[data-thread-nav="${childId}"]`);
    await expect(childRow).toBeVisible();
    await expect(childRow).toHaveClass(/thread-row-focused/);
    // Inside the list's own viewport, not merely rendered below the fold.
    // Polled past any late scroll restore, then checked once it has settled.
    const rowInList = () => page.evaluate((id) => {
        const row = document.querySelector(`.thread-drawer-list .thread-row[data-thread-nav="${id}"]`);
        const list = document.querySelector('.thread-drawer-list');
        if (!row || !list) return false;
        const r = row.getBoundingClientRect();
        const l = list.getBoundingClientRect();
        return r.top >= l.top && r.bottom <= l.bottom;
    }, childId);
    await expect.poll(rowInList).toBe(true);
    await page.waitForTimeout(500);
    expect(await rowInList()).toBe(true);
}

/** Show in Folders, from the thread title bar's thread menu. One menu pick has
 *  to bring the open thread's row into view, whatever hid it. */
test.describe('Show in Folders', () => {
    test.beforeEach(async ({ page }) => {
        await assertHealthy(page);
        clearAllThreads();
    });

    test('opens the drawer, expands the family and scrolls the row into view', async ({ page, context }) => {
        const { parentId, childId, stamp } = seedFamilyAmongFillers();
        await context.addInitScript(({ childId, parentId }) => {
            localStorage.setItem('lucidos-focused-thread', childId);
            localStorage.setItem('lucidos-thread-drawer-open', 'false');
            localStorage.setItem('lucidos-drawer-collapsed-families', JSON.stringify([parentId]));
            localStorage.removeItem('lucidos-drawer-collapsed');
        }, { childId, parentId });

        await navigateToApp(page);
        await expect(page.locator(`.thread-row[data-thread-nav="${childId}"]`)).toHaveCount(0);
        await showInFoldersAndExpectRowInView(page, childId, stamp);

        const families = await page.evaluate(() => localStorage.getItem('lucidos-drawer-collapsed-families'));
        expect(JSON.parse(families ?? '[]')).not.toContain(parentId);
    });

    // Leaving the Ongoing grouping re-arms the list's scroll memory. A restore of
    // the saved offset must not land after the reveal and undo its scroll. The
    // drawer starts shut, so the reveal opens it as well.
    test('scrolls to the row when leaving the Ongoing grouping with nothing to expand', async ({ page, context }) => {
        const { childId, stamp } = seedFamilyAmongFillers();
        await context.addInitScript((childId) => {
            localStorage.setItem('lucidos-focused-thread', childId);
            localStorage.setItem('lucidos-thread-drawer-open', 'false');
            localStorage.setItem('lucidos-drawer-grouping', 'ongoing');
            localStorage.setItem('lucidos-scroll-thread-drawer', '0');
            localStorage.removeItem('lucidos-drawer-collapsed');
            localStorage.removeItem('lucidos-drawer-collapsed-families');
        }, childId);

        await navigateToApp(page);
        await showInFoldersAndExpectRowInView(page, childId, stamp);
    });

    // WebKit has no scroll anchoring, so the drawer's opening width animation
    // moves the row after one scroll. Turning anchoring off reproduces it here.
    test('keeps the row in view while an opening drawer settles, without scroll anchoring', async ({ page, context }) => {
        const { childId, stamp } = seedFamilyAmongFillers();
        await context.addInitScript((childId) => {
            localStorage.setItem('lucidos-focused-thread', childId);
            localStorage.setItem('lucidos-thread-drawer-open', 'false');
            localStorage.removeItem('lucidos-drawer-collapsed');
            localStorage.removeItem('lucidos-drawer-collapsed-families');
        }, childId);

        await navigateToApp(page);
        await page.addStyleTag({ content: '.thread-drawer-list { overflow-anchor: none !important; }' });
        await showInFoldersAndExpectRowInView(page, childId, stamp);
    });
});
