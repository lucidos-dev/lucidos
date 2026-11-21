import { test, expect } from './fixtures';
import { assertHealthy, openThreadDrawer } from './helpers';

test.describe('openThreadDrawer on a page that restores the drawer open', () => {
  test('leaves a restored drawer open when called before the app has drawn', async ({ page, context }) => {
    await assertHealthy(page);
    await context.addInitScript(() => localStorage.setItem('lucidos-thread-drawer-open', 'true'));
    // `commit` returns before the app renders, the state a reload leaves.
    await page.goto('/', { waitUntil: 'commit' });
    await openThreadDrawer(page);
    await expect(page.locator('.thread-drawer:not(.thread-drawer-collapsed):visible').first()).toBeVisible();
  });
});
