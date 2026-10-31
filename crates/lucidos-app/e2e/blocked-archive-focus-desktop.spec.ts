import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

test.describe('Archiving in Blocked', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        await context.clearCookies();
        clearAllThreads();
    });

    for (const via of ['row menu', 'thread'] as const) {
        test(`opens the next Blocked row (archived from the ${via})`, async ({ page, context }) => {
            await context.addInitScript(() => {
                localStorage.setItem('lucidos-drawer-grouping', 'ongoing');
                localStorage.setItem('lucidos-drawer-selected-ongoing-group', 'blocked');
            });
            const stamp = Date.now();
            const ids = [randomUUID(), randomUUID(), randomUUID()];
            const base = Date.now();
            // The list sorts on last_user_action, which the seed leaves at one
            // shared now(). Pin it so the rows run ids[0], ids[1], ids[2].
            psql([
                ...ids.map((id, i) => seedThreadRow({
                    id, title: `attention-${i}-${stamp}`, now: new Date(base - i * 60_000).toISOString(),
                    status: 'failed', archiveState: 'inbox',
                })),
                `UPDATE thread_summaries SET last_user_action = last_activity WHERE thread_id IN ('${ids.join("','")}')`,
            ].join(';\n'));

            await navigateToApp(page);
            await openThreadDrawer(page);
            const row = (id: string) => page.locator(`.thread-row[data-thread-nav="${id}"]`);
            await expect(row(ids[2])).toBeVisible();

            await row(ids[1]).click();
            await expect(row(ids[1])).toHaveClass(/thread-row-focused/);

            if (via === 'row menu') {
                await row(ids[1]).click({ button: 'right' });
                await page.getByRole('menuitem', { name: 'Archive' }).click();
            } else {
                await page.locator('button[aria-label="Archive thread"]:visible').first().click();
            }

            await expect(row(ids[1])).toHaveCount(0);
            await expect(row(ids[2])).toHaveClass(/thread-row-focused/);
        });
    }
});
