import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

function colorDistanceSquared(a: string, b: string): number {
    const parse = (s: string) => {
        const m = s.match(/(\d+)\D+(\d+)\D+(\d+)/);
        if (!m) throw new Error('not rgb(): ' + s);
        return [+m[1], +m[2], +m[3]];
    };
    const [r1, g1, b1] = parse(a);
    const [r2, g2, b2] = parse(b);
    return (r1 - r2) ** 2 + (g1 - g2) ** 2 + (b1 - b2) ** 2;
}

test.use({ viewport: { width: 1280, height: 900 }, colorScheme: 'light' });

test.describe('Drawer divider thickness', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        await context.clearCookies();
        await context.addInitScript(() => {
            localStorage.removeItem('lucidos-drawer-collapsed');
            localStorage.removeItem('lucidos-drawer-collapsed-families');
            localStorage.setItem('lucidos-theme-mode', 'light');
        });
        clearAllThreads();
    });

    test('hovered row bg is visibly distinct from the divider color', async ({ page }) => {
        const now = new Date().toISOString();
        const stamp = Date.now();
        const inserts = Array.from({ length: 4 }, (_, i) =>
            seedThreadRow({ id: randomUUID(), title: `archived-${stamp}-${i}`, now })
        );
        psql(inserts.join(';\n'));

        await navigateToApp(page);
        await openThreadDrawer(page);
        await page.waitForSelector('.thread-row-wrap');

        const targetRow = page.locator('.thread-drawer-list .thread-row-wrap .thread-row').nth(1);
        await targetRow.hover();
        await page.waitForTimeout(200);

        const colors = await targetRow.evaluate((row) => {
            return {
                hoveredBg: getComputedStyle(row).backgroundColor,
                dividerBg: getComputedStyle(row, '::after').borderBottomColor,
            };
        });

        const d = colorDistanceSquared(colors.hoveredBg, colors.dividerBg);
        if (d <= 2500) {
            console.log(`hover bg ${colors.hoveredBg} vs divider ${colors.dividerBg} — squared distance ${d}`);
        }
        expect(d).toBeGreaterThan(2500);
    });
});
