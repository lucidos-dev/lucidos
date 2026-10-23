/**
 * Mobile: Settings > Intelligence > the Claude Code ladder fits a phone and
 * reorders by press and drag.
 *
 * A compact row shows the name with its star and switch, then three bands
 * under their own labels. The reorder arrows are gone, so a hold lifts a rung,
 * a drag moves it, and the release saves the new order once.
 */
import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, openSettingsView } from './helpers';

const LADDER_KEY = 'intelligence_ladder_claude_code';

test.afterEach(async ({ page }) => {
  await apiRequest(page).delete(`/api/v1/preferences?key=${LADDER_KEY}`);
});

test('the ladder fits the phone and a held rung drags to a new place', async ({ page }) => {
  const writes: string[] = [];
  page.on('request', (req) => {
    if (req.method() === 'PUT' && req.url().includes(`key=${LADDER_KEY}`)) writes.push(req.url());
  });
  await assertHealthy(page);
  await navigateToApp(page);
  await openSettingsView(page, 'intelligence');

  const section = page.locator('.ladder-section', {
    has: page.locator('[data-search-anchor="intelligence:ladder-claude-code"]'),
  });
  const rows = section.locator('.ladder-row');
  await expect(rows.first()).toBeVisible();
  const names = () => rows.locator('.title').allTextContents();

  // Nothing runs off the pane, and the compact cells carry their labels.
  const overflow = await section.evaluate((el) => el.scrollWidth - el.clientWidth);
  expect(overflow, 'the ladder scrolls sideways').toBeLessThanOrEqual(0);
  await expect(section.locator('.ladder-head')).toBeHidden();
  // The arrows shrink to a clipped 1px box: out of sight, still in the tab order.
  const arrowBox = await rows.first().locator('.ladder-move').first().boundingBox();
  expect(arrowBox?.width, 'the arrows show on a phone').toBeLessThanOrEqual(1);
  await expect(rows.first().locator('.ladder-cell-label').first()).toBeVisible();

  const before = await names();
  expect(before.length).toBeGreaterThan(2);

  // Bring the first rung to mid-screen, clear of the sticky header. Then wait
  // out whatever still moves the page, so each point below is read where it
  // holds. A swipe scrolls the page, so this runs again before the hold.
  const titleBox = () => rows.first().locator('.title').boundingBox();
  const settle = async () => {
    await rows.first().evaluate((el) => el.scrollIntoView({ block: 'center' }));
    let last: number | undefined;
    await expect.poll(async () => {
      const now = (await titleBox())?.y;
      const still = now === last;
      last = now;
      return still;
    }, { intervals: [300] }).toBe(true);
    return (await titleBox())!;
  };

  // A swipe is a scroll: nothing is lifted and nothing is written.
  // Press clear of the left edge, where the pane's own edge swipe listens.
  const swipeFrom = await settle();
  await page.mouse.move(swipeFrom.x + 60, swipeFrom.y + 4);
  await page.mouse.down();
  await page.mouse.move(swipeFrom.x + 60, swipeFrom.y + 80, { steps: 4 });
  await page.mouse.up();
  expect(await names()).toEqual(before);
  expect(writes, 'a swipe saved an order').toHaveLength(0);

  // A hold, a drag onto the third rung, and the release.
  const first = await settle();
  const third = (await rows.nth(2).boundingBox())!;
  const write = page.waitForRequest((req) => req.method() === 'PUT' && req.url().includes(`key=${LADDER_KEY}`));
  await page.mouse.move(first.x + 60, first.y + 4);
  await page.mouse.down();
  await page.waitForTimeout(700);
  await expect(rows.first()).toHaveClass(/ladder-row-lifted/);
  await page.mouse.move(first.x + 60, third.y + third.height / 2, { steps: 8 });
  await page.mouse.up();
  await write;

  await expect.poll(names).not.toEqual(before);
  expect((await names())[0]).not.toBe(before[0]);
  expect(writes, 'one drag, one write').toHaveLength(1);
});
