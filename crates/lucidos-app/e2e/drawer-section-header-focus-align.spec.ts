import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

test.use({ viewport: { width: 1280, height: 900 }, colorScheme: 'dark' });

test.describe('Drawer section-header focus band alignment', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        await context.clearCookies();
        await context.addInitScript(() => {
            localStorage.removeItem('lucidos-drawer-collapsed');
            localStorage.removeItem('lucidos-drawer-collapsed-families');
            localStorage.setItem('lucidos-theme', 'dark');
        });
        clearAllThreads();
    });

    // The bug: a focused/keyboard-highlighted SECTION HEADER painted its band on
    // the full-width `.list-section-title` box (no right margin), so it ran past
    // the content edge — toward/under the scrollbar — while a focused THREAD ROW
    // insets its band 0.5rem from the drawer's right edge (`.thread-row`'s
    // `margin-right: 0.5rem`). Fix mirrors that inset onto the section-header box.
    //
    // The band is painted on the element's own border box (`.list-section-title`
    // background for headers, `.thread-row` background for rows), so the element
    // bounding rects ARE the band rects — we assert the header's left/right edges
    // line up with a thread row's, no need to drive the highlight to measure it.
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
            const header = document.querySelector('.thread-drawer-list .list-section-title');
            const row = document.querySelector('.thread-drawer-list .thread-row-wrap .thread-row');
            if (!header || !row) throw new Error('drawer header or row not found');
            const h = header.getBoundingClientRect();
            const r = row.getBoundingClientRect();
            return { headerLeft: h.left, headerRight: h.right, rowLeft: r.left, rowRight: r.right };
        });

        // Right edges must match: before the fix the header overshot the row by
        // 0.5rem (8px), running full-width to the drawer edge.
        if (Math.abs(edges.headerRight - edges.rowRight) > 1) {
            console.log(`header.right ${edges.headerRight} vs row.right ${edges.rowRight}`);
        }
        expect(Math.abs(edges.headerRight - edges.rowRight)).toBeLessThanOrEqual(1);
        // Left edges already match (neither carries a left margin) — pin it so a
        // future left-margin change can't reintroduce the misalignment.
        expect(Math.abs(edges.headerLeft - edges.rowLeft)).toBeLessThanOrEqual(1);
    });
});
