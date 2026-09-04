import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, openTriggersPanel } from './helpers';

/** The trigger group heading's delete icon must be what a touch at its own outer
 *  edge actually hits.
 *
 *  Reported as "the trashcan is near impossible to hit on mobile". The cause is
 *  not its size: `.edge-swipe-right` is a transparent 1.25rem strip pinned to
 *  the right edge of every mobile pane at `z-index: 1` (EdgeSwipeZones, rendered
 *  inside each `.mobile-swipe-pane`), and the heading's actions sit in exactly
 *  that column, so the strip covered the outer third of the button. A touch
 *  there resolves to the strip: it never reaches the button, AND, the strip not
 *  being an interactive target, `shouldSuppressEdgeNavigation` reads it as an
 *  iOS edge-navigation gesture and preventDefault()s it.
 *
 *  Hit-tested rather than tapped, deliberately. Playwright drives an element's
 *  CENTRE, which was always clear of the strip, so `.tap()` passed both before
 *  and after the fix and proves nothing. The outer edge is where a thumb reaching
 *  for the last control in a row actually lands, so that is the point to assert.
 *
 *  `-mobile.spec.ts` because the strip only exists under the mobile layout. */

const PREFIX = 'e2e-edge';

/** What `document.elementFromPoint` resolves to at `(x, y)`, described in the
 *  two terms this test cares about: the delete button (possibly via its glyph),
 *  the edge strip, or whatever else got in the way. */
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
      /* best-effort cleanup */
    }
  });

  test('a touch on the delete icon reaches the button, not the swipe strip', async ({ page }) => {
    await assertHealthy(page);
    // Created over HTTP so the panel simply loads it: this test is about where
    // the button IS, not about the create flow the lifecycle spec covers.
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
    // An empty group's delete is enabled. A disabled one is `pointer-events:
    // none` by design and would hit-test straight through, which would make
    // this assertion meaningless.
    await expect(del).toBeEnabled();

    const box = (await del.boundingBox())!;
    expect(box, 'the delete button has no box').toBeTruthy();

    const midY = box.y + box.height / 2;
    // Just inside the button's right edge: the part the strip used to own.
    expect(await hitAt(page, box.x + box.width - 2, midY)).toBe('delete-button');
    // And its centre, which was always reachable, so a regression here would
    // mean something bigger moved.
    expect(await hitAt(page, box.x + box.width / 2, midY)).toBe('delete-button');

    // The strip is still there doing its job just beyond the button, which is
    // what keeps this a lift rather than a removal.
    const stripWidth = await page.evaluate(() => {
      const strip = document.querySelector('.edge-swipe-right');
      return strip ? strip.getBoundingClientRect().width : 0;
    });
    expect(stripWidth, 'the right edge-swipe strip is gone, not merely cleared').toBeGreaterThan(0);
  });
});
