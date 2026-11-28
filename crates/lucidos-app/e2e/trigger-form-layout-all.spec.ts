import { test, expect, Page } from './fixtures';
import { navigateToApp, openTriggersPanel, addTriggerCard, isMobileViewport } from './helpers';


async function openTriggerForm(page: Page): Promise<void> {
  await navigateToApp(page);
  await openTriggersPanel(page);
  await addTriggerCard(page).click();
  await expect(page.locator('.inline-form:visible').first()).toBeVisible({ timeout: 10_000 });
}

test('Group dropdown chevron is right-aligned in the trigger form', async ({ page }) => {
  await openTriggerForm(page);

  const gap = await page.evaluate(() => {
    const vis = (sel: string) =>
      Array.from(document.querySelectorAll(sel)).find(e => e.getBoundingClientRect().width > 0) as HTMLElement | undefined;
    const trigger = vis('.trigger-group-select .dropdown-trigger');
    const chevron = trigger?.querySelector('.dropdown-chevron') as HTMLElement | undefined;
    if (!trigger || !chevron) return null;
    return trigger.getBoundingClientRect().right - chevron.getBoundingClientRect().right;
  });
  expect(gap, 'chevron should resolve').not.toBeNull();
  expect(gap!).toBeLessThanOrEqual(20);
});

test('Cancel/Save stay reachable on mobile', async ({ page }) => {
  test.skip(!isMobileViewport(page), 'desktop form fits without scrolling');
  await openTriggerForm(page);

  await page.evaluate(() => {
    const body = Array.from(document.querySelectorAll('.content-pane-body'))
      .find(e => e.getBoundingClientRect().width > 0) as HTMLElement | undefined;
    if (body) body.scrollTop = body.scrollHeight;
  });

  const save = page.locator('.inline-form .btn-save:visible').first();
  await expect(save).toBeVisible();
  const fits = await save.evaluate((el) => {
    const r = el.getBoundingClientRect();
    return r.top >= 0 && r.bottom <= window.innerHeight;
  });
  expect(fits, 'Save button fully within the viewport after scrolling').toBe(true);
});
