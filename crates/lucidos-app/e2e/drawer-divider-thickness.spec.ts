import { test, expect } from '@playwright/test';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

/** Squared Euclidean distance between two `rgb(r, g, b)` strings. The exact
 *  numeric scale doesn't matter — we just need a stable ordering: same color
 *  is 0, white-to-black is ~195000. The threshold below is calibrated for the
 *  border-color (#d0d7de) vs the hover bg color we want to assert distance from. */
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
            localStorage.setItem('lucidos-theme', 'light');
        });
        clearAllThreads();
    });

    // The bug: the global `.list-row:hover { background: var(--bg-hover) }`
    // uses #eaeef2 — only ~30 RGB units from the divider color #d0d7de. At
    // retina + browser zoom the hovered row's top edge of bg-hover bleeds
    // into the 1px divider above it and they read as one thick band. Fixed
    // by overriding the drawer's hovered-row bg to a color that's well
    // clear of the divider color in RGB space.
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
            const wrapper = row.closest('.thread-row-wrap')!;
            return {
                hoveredBg: getComputedStyle(row).backgroundColor,
                dividerBg: getComputedStyle(wrapper, '::before').backgroundColor,
            };
        });

        // Reference values: divider #d0d7de (208,215,222) vs `--bg-hover`
        // #eaeef2 (the buggy color) squared-distance = 1605; vs `--bg-secondary`
        // #f6f8fa (the fix) = 3317. Threshold 2500 sits cleanly between them.
        const d = colorDistanceSquared(colors.hoveredBg, colors.dividerBg);
        if (d <= 2500) {
            console.log(`hover bg ${colors.hoveredBg} vs divider ${colors.dividerBg} — squared distance ${d}`);
        }
        expect(d).toBeGreaterThan(2500);
    });
});
