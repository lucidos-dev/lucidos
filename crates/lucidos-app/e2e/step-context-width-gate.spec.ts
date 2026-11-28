import { test, expect, type Locator, type Page } from './fixtures';
import { assertHealthy, hideDashboardRail, minThreadPane, navigateToApp, newThread, revealSteps, sendMessage, waitForResponse } from './helpers';
import { clearAllThreads } from './db-helpers';


test.use({ viewport: { width: 1280, height: 800 } });

const WIDE_PANE = 860;
const NARROW_PANE = 424;

async function dragDividerTo(page: Page, toX: number): Promise<void> {
  const box = await page.locator('.split-divider').boundingBox();
  if (!box) throw new Error('.split-divider not visible');
  await page.mouse.move(box.x + box.width / 2, 400);
  await page.mouse.down();
  await page.mouse.move(toX, 400, { steps: 5 });
  await page.mouse.up();
  await expect
    .poll(() => page.evaluate(() => document.querySelector('.pane-thread')!.getBoundingClientRect().width))
    .toBeCloseTo(toX, -1);
}

async function counterOfFirstStep(page: Page): Promise<Locator> {
  await newThread(page);
  await sendMessage(page, 'Say "hello world" and nothing else.');
  await waitForResponse(page);

  await revealSteps(page);

  const counter = page
    .locator('[data-role="inline-step"]:visible [data-role="step-context"]')
    .first();
  await expect(counter).toBeVisible({ timeout: 30_000 });
  return counter;
}

test.describe('Step context counter width gate', () => {
  test('a narrow thread pane collapses the counter on a desktop viewport', async ({ page, context }) => {
    await assertHealthy(page);
    clearAllThreads();
    await context.addInitScript(() => {
      localStorage.setItem('lucidos-split-ratio', '0.4');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
    });
    await hideDashboardRail(page);
    await navigateToApp(page);

    const counter = await counterOfFirstStep(page);
    expect(await minThreadPane(page), 'the floor sits under the narrow pane').toBeLessThan(NARROW_PANE);
    const full = counter.locator('.step-context-full');
    const compact = counter.locator('.step-context-compact');

    await dragDividerTo(page, WIDE_PANE);
    await expect(full).toBeVisible();
    await expect(compact).toBeHidden();
    await expect(full).toContainText('/');

    await dragDividerTo(page, NARROW_PANE);
    await expect(compact).toBeVisible();
    await expect(full).toBeHidden();
    await expect(compact).toContainText('%');

    await dragDividerTo(page, WIDE_PANE);
    await expect(full).toBeVisible();
    await expect(compact).toBeHidden();
  });
});

test.describe('Step context counter at a phone width', () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test('collapses to the percentage with no divider in the layout at all', async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);

    const counter = await counterOfFirstStep(page);
    await expect(counter.locator('.step-context-compact')).toBeVisible();
    await expect(counter.locator('.step-context-full')).toBeHidden();
  });
});
