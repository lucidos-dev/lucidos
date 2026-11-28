import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, openTriggersPanel } from './helpers';


const PREFIX = 'e2e-edge';

async function hitAt(page: import('./fixtures').Page, x: number, y: number): Promise<string> {
  return page.evaluate(([px, py]) => {
    const el = document.elementFromPoint(px, py);
    if (!el) return 'nothing';
    if (el.closest('.trigger-group-delete')) return 'delete-button';
    if (el.closest('.edge-swipe-zone')) return 'edge-swipe-zone';
    return `other: ${el.tagName.toLowerCase()}.${el.className}`;
  }, [x, y]);
}

test.describe('Trigger group actions clear the edge-swipe strip', () => {
  test.afterEach(async ({ page }) => {
    try {
      const res = await page.request.get('/api/v1/trigger-groups');
      const body = await res.json();
      for (const g of (body.groups ?? []) as Array<{ id: string; name: string }>) {
        if (g.name?.startsWith(PREFIX)) await apiRequest(page).delete(`/api/v1/trigger-groups?id=${g.id}`);
      }
    } catch {
    }
  });

  test('a touch on the delete icon reaches the button, not the swipe strip', async ({ page }) => {
    await assertHealthy(page);
    const groupName = `${PREFIX}-${Date.now()}`;
    const created = await apiRequest(page).post('/api/v1/trigger-groups', { data: { name: groupName } });
    expect(created.ok(), 'failed to create the group over the API').toBe(true);

    await navigateToApp(page);
    await openTriggersPanel(page);

    const section = page.locator('.trigger-group-section').filter({
      has: page.locator('.trigger-group-name', { hasText: groupName }),
    });
    const del = section.locator('.trigger-group-delete');
    await expect(del).toBeVisible({ timeout: 10_000 });
    await expect(del).toBeEnabled();

    const box = (await del.boundingBox())!;
    expect(box, 'the delete button has no box').toBeTruthy();

    const midY = box.y + box.height / 2;
    expect(await hitAt(page, box.x + box.width - 2, midY)).toBe('delete-button');
    expect(await hitAt(page, box.x + box.width / 2, midY)).toBe('delete-button');

    const stripWidth = await page.evaluate(() => {
      const strip = document.querySelector('.edge-swipe-right');
      return strip ? strip.getBoundingClientRect().width : 0;
    });
    expect(stripWidth, 'the right edge-swipe strip is gone, not merely cleared').toBeGreaterThan(0);
  });
});
