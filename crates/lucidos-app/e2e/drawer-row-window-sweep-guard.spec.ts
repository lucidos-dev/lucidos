import { test, expect } from './fixtures';
import { assertHealthy } from './helpers';
import { clearAllThreads } from './db-helpers';
import { seedCurrent, sweepList, openSeededList, type SeededCurrent } from './drawer-row-window-probe';


let seeded: SeededCurrent;

test.describe('Drawer row window sweep guard', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        await context.addInitScript(() => localStorage.setItem('lucidos-thread-drawer-open', 'true'));
        clearAllThreads();
        seeded = seedCurrent(60);
    });

    test('a page that paints no frame fails the sweep with its state', async ({ page }) => {
        await openSeededList(page, seeded);
        await page.evaluate(() => { window.requestAnimationFrame = () => 0; });
        await expect(sweepList(page)).rejects.toThrow(/sweep: no frame in 5 s at scrollTop/);
    });
});
