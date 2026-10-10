import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy } from './helpers';

test.use({ viewport: { width: 1280, height: 900 }, colorScheme: 'light' });

/**
 * Regression: numbered lists rendered with BOTH a number and a bullet
 * ("1. • text"). marked emits clean `<ol><li>` for a nested numbered list, so
 * the extra '•' came from CSS: a custom `.markdown-content ul li::before`
 * bullet that leaked onto the <li>s of an <ol> nested inside a <ul>.
 *
 * `.markdown-content` was consolidated to ONE definition (shared-components.css)
 * that uses NATIVE list markers — no `::before` bullet at all. Native markers
 * make the double-marking structurally impossible: each list level owns its own
 * marker, so a nested <ol> shows decimal only. This spec pins that property at
 * the CSS-cascade level — a unit test on rendered HTML can't catch it (the HTML
 * is correct; only the marker rendering leaked).
 *
 * Two things are asserted, because both are load-bearing:
 *   1. list-style-type is per-level (ul→disc, nested ol→decimal), and
 *   2. no element carries a custom `::before` bullet (guards against anyone
 *      reintroducing the `::before` approach that caused the original bug).
 */
test.describe('Markdown nested list markers', () => {
    test.beforeEach(async ({ page }) => {
        await assertHealthy(page);
    });

    test('a nested <ol> inside a <ul> shows numbers only, never a bullet too', async ({ page }) => {
        await navigateToApp(page);

        // Inject the exact HTML marked produces for a numbered list nested under
        // a bullet item, into a real `.response-content.markdown-content`
        // container so the live app cascade applies.
        const markers = await page.evaluate(() => {
            const host = document.createElement('div');
            host.className = 'response-content markdown-content';
            host.innerHTML = `
                <ul>
                    <li id="t-outer-bullet">mobile_beat block
                        <ol>
                            <li id="t-nested-num-1">Desktop 175%</li>
                            <li id="t-nested-num-2">App content clipped</li>
                        </ol>
                    </li>
                </ul>`;
            document.body.appendChild(host);

            const probe = (id: string) => {
                const el = document.getElementById(id)!;
                return {
                    listStyleType: getComputedStyle(el).listStyleType,
                    beforeContent: getComputedStyle(el, '::before').content,
                };
            };
            const result = {
                outerBullet: probe('t-outer-bullet'),
                nestedNum1: probe('t-nested-num-1'),
                nestedNum2: probe('t-nested-num-2'),
            };
            host.remove();
            return result;
        });

        // The outer <ul> item gets a native disc; the nested <ol> items get
        // native decimal — each level owns ITS OWN marker, so the nested
        // numbered items can never also carry a bullet.
        expect(markers.outerBullet.listStyleType).toBe('disc');
        expect(markers.nestedNum1.listStyleType).toBe('decimal');
        expect(markers.nestedNum2.listStyleType).toBe('decimal');

        // No element may carry a custom `::before` bullet — that pseudo-element
        // was exactly what leaked the '•' onto nested <ol> items. 'none' is the
        // computed value when no ::before content applies.
        expect(markers.outerBullet.beforeContent).toBe('none');
        expect(markers.nestedNum1.beforeContent).toBe('none');
        expect(markers.nestedNum2.beforeContent).toBe('none');
    });
});
