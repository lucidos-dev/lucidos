import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy } from './helpers';

test.use({ viewport: { width: 1280, height: 900 }, colorScheme: 'light' });

test.describe('Markdown nested list markers', () => {
    test.beforeEach(async ({ page }) => {
        await assertHealthy(page);
    });

    test('a nested <ol> inside a <ul> shows numbers only, never a bullet too', async ({ page }) => {
        await navigateToApp(page);

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

        expect(markers.outerBullet.listStyleType).toBe('disc');
        expect(markers.nestedNum1.listStyleType).toBe('decimal');
        expect(markers.nestedNum2.listStyleType).toBe('decimal');

        expect(markers.outerBullet.beforeContent).toBe('none');
        expect(markers.nestedNum1.beforeContent).toBe('none');
        expect(markers.nestedNum2.beforeContent).toBe('none');
    });
});
