/**
 * Desktop: Settings > Intelligence > the Claude Code ladder keeps its columns,
 * and a rung reorders by dragging its grip. The arrows stay for the keyboard
 * only. A pane narrower than the breakpoint takes the phone layout, which the
 * mobile spec covers.
 */
import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, openSettingsView } from './helpers';

// Wide enough that the settings pane, one of three, clears the ladder's
// compact breakpoint (36rem).
test.use({ viewport: { width: 2400, height: 1000 } });

const LADDER_KEY = 'intelligence_ladder_claude_code';

test.afterEach(async ({ page }) => {
  await apiRequest(page).delete(`/api/v1/preferences?key=${LADDER_KEY}`);
});

test('the wide ladder shows column heads and moves a rung by its grip', async ({ page }) => {
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

  await expect(section.locator('.ladder-head > span')).toHaveText([
    '', 'Rung, least to most intelligent', 'Intelligence', 'Cost', 'Time', 'Default', 'On',
  ]);
  await expect(rows.first().locator('.ladder-cell-label').first()).toBeHidden();
  // The arrows shrink to a clipped 1px box: out of sight, still in the tab order.
  const arrowBox = await rows.first().locator('.ladder-move').first().boundingBox();
  expect(arrowBox?.width, 'the arrows show on desktop').toBeLessThanOrEqual(1);

  const names = () => rows.locator('.title').allTextContents();
  const before = await names();

  // A mouse press on the grip lifts at once, and the release saves once.
  const grip = (await rows.first().locator('.ladder-grip').boundingBox())!;
  const third = (await rows.nth(2).boundingBox())!;
  const write = page.waitForRequest((req) => req.method() === 'PUT' && req.url().includes(`key=${LADDER_KEY}`));
  await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
  await page.mouse.down();
  await page.mouse.move(grip.x + grip.width / 2, third.y + third.height / 2, { steps: 8 });
  await expect(rows.first()).toHaveClass(/ladder-row-lifted/);
  await page.mouse.up();
  await write;

  await expect.poll(names).not.toEqual(before);
  expect((await names())[0]).not.toBe(before[0]);
  expect(writes, 'one drag, one write').toHaveLength(1);
});
