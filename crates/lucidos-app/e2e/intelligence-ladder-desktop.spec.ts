import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, openSettingsView } from './helpers';

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
  await section.scrollIntoViewIfNeeded();

  await expect(section.locator('.ladder-head > span')).toHaveText([
    '', 'Rung, least to most intelligent', 'Intelligence', 'Cost', 'Time', 'Default', 'On',
  ]);
  await expect(rows.first().locator('.ladder-cell-label').first()).toBeHidden();
  const arrowBox = await rows.first().locator('.ladder-move').first().boundingBox();
  expect(arrowBox?.width, 'the arrows show on desktop').toBeLessThanOrEqual(1);

  const names = () => rows.locator('.title').allTextContents();
  const before = await names();

  await rows.nth(2).scrollIntoViewIfNeeded();
  await rows.nth(2).scrollIntoViewIfNeeded();
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

test('the default moves on tap and a refused save restores it', async ({ page }) => {
  await assertHealthy(page);
  await navigateToApp(page);
  await openSettingsView(page, 'intelligence');

  const section = page.locator('.ladder-section', {
    has: page.locator('[data-search-anchor="intelligence:ladder-claude-code"]'),
  });
  const oldDefault = section.locator('.ladder-row').filter({
    has: page.getByRole('button', { name: /is the default/ }),
  });
  await expect(oldDefault.getByRole('checkbox')).toBeDisabled();
  const previousName = await oldDefault.locator('.title').textContent();

  await section.getByRole('checkbox', { name: /Show rungs/ }).check();
  const off = section.locator('.ladder-row').filter({ has: page.locator('input:not(:checked)') }).first();
  const nextName = await off.locator('.title').textContent();
  const pick = section.locator('.ladder-row').filter({ hasText: nextName! });
  let refuse = () => {};
  const reply = new Promise<void>((resolve) => { refuse = resolve; });
  await page.route('**/api/v1/intelligence-ladder/default', async (route) => {
    await reply;
    await route.fulfill({ status: 400, json: { error: 'Default refused' } });
  });

  try {
    await pick.getByRole('button', { name: /Make .* the default/ }).click();
    await expect(pick.getByRole('button', { name: /is the default/ })).toHaveAttribute('aria-pressed', 'true');
    await expect(pick.getByRole('checkbox')).toBeChecked();
    await expect(pick.getByRole('checkbox')).toBeDisabled();
  } finally {
    refuse();
  }
  await expect(oldDefault.locator('.title')).toHaveText(previousName!);
  await expect(pick.getByRole('checkbox')).not.toBeChecked();
});
