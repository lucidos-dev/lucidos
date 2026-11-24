import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

test.use({ viewport: { width: 1280, height: 900 }, colorScheme: 'dark', reducedMotion: 'reduce' });

test.describe('Drawer section-header focus band alignment', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        await context.clearCookies();
        await context.addInitScript(() => {
            localStorage.removeItem('lucidos-drawer-collapsed');
            localStorage.removeItem('lucidos-drawer-collapsed-families');
            localStorage.setItem('lucidos-theme-mode', 'dark');
        });
        clearAllThreads();
    });

    test('section-header box right edge lines up with a thread-row box', async ({ page }) => {
        const now = new Date().toISOString();
        const stamp = Date.now();
        const inserts = Array.from({ length: 4 }, (_, i) =>
            seedThreadRow({ id: randomUUID(), title: `archived-${stamp}-${i}`, now })
        );
        psql(inserts.join(';\n'));

        await navigateToApp(page);
        await openThreadDrawer(page);
        await page.waitForSelector('.thread-drawer-list .thread-row-wrap .thread-row');
        await page.waitForSelector('.thread-drawer-list .list-section-title');

        const edges = await page.evaluate(() => {
            const list = document.querySelector('.thread-drawer-list');
            if (!list) throw new Error('drawer list not found');
            for (const anim of list.getAnimations({ subtree: true })) {
                const timing = anim.effect?.getComputedTiming();
                if (!timing || timing.iterations === Infinity || timing.duration === Infinity) continue;
                anim.finish();
            }
            const header = list.querySelector('.list-section-title');
            const row = list.querySelector('.thread-row-wrap .thread-row');
            if (!header || !row) throw new Error('drawer header or row not found');
            const h = header.getBoundingClientRect();
            const r = row.getBoundingClientRect();
            return { headerLeft: h.left, headerRight: h.right, rowLeft: r.left, rowRight: r.right };
        });

        if (Math.abs(edges.headerRight - edges.rowRight) > 1) {
            console.log(`header.right ${edges.headerRight} vs row.right ${edges.rowRight}`);
        }
        expect(Math.abs(edges.headerRight - edges.rowRight)).toBeLessThanOrEqual(1);
        expect(Math.abs(edges.headerLeft - edges.rowLeft)).toBeLessThanOrEqual(1);
    });
});
