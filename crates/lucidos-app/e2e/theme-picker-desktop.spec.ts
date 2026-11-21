/**
 * Settings > Appearance lists every theme inline, in a carousel two cards tall
 * filtered by family chips, with a chevron over each side. It pages in whole
 * cards. A pick paints at once and the carousel stays, so the next click on
 * anything else simply lands, with nothing to fold first.
 */
import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, openSettingsView, waitForEventStream } from './helpers';

test.use({ viewport: { width: 1280, height: 900 } });

test('the theme carousel picks in place and lets the next click land', async ({ page }) => {
  await assertHealthy(page);
  // Dark, so a light-only theme raises the mode-switch confirm below.
  await page.emulateMedia({ colorScheme: 'dark' });
  await navigateToApp(page);
  await waitForEventStream(page);
  await openSettingsView(page, 'appearance');

  const strip = page.locator('.theme-carousel');
  const radios = strip.locator('.theme-card[role="radio"]');
  await expect(radios.first()).toBeVisible();

  // Two cards tall, and wider than the column, so it scrolls sideways alone.
  const layout = await strip.evaluate(el => {
    const tops = new Set([...el.querySelectorAll('.theme-family:first-child .theme-card')].map(c => Math.round(c.getBoundingClientRect().top)));
    return { rows: tops.size, scrolls: el.scrollWidth > el.clientWidth };
  });
  expect(layout).toEqual({ rows: 2, scrolls: true });
  const pageScrollsSideways = await page.locator('.content-pane-body').evaluate(el => el.scrollWidth > el.clientWidth);
  expect(pageScrollsSideways, 'the carousel widened the page').toBe(false);

  // The chevrons page the strip, and the one at an end it has reached is off.
  const previous = page.getByRole('button', { name: 'Previous themes' });
  const next = page.getByRole('button', { name: 'Next themes' });
  await strip.evaluate(el => { el.scrollLeft = 0; el.dispatchEvent(new Event('scroll')); });
  await expect(previous).toBeDisabled();
  await next.click();
  await expect.poll(() => strip.evaluate(el => el.scrollLeft)).toBeGreaterThan(0);
  await expect(previous).toBeEnabled();
  // No scrollbar: the strip is as tall as its content.
  const barHeight = await strip.evaluate(el => el.offsetHeight - el.clientHeight);
  expect(barHeight, 'the strip draws a scrollbar').toBe(0);
  // A page shows whole cards only: none is cut by either side.
  await expect.poll(() => strip.evaluate(el => {
    const box = el.getBoundingClientRect();
    return [...el.querySelectorAll('.theme-card')].every(card => {
      const r = card.getBoundingClientRect();
      const showing = r.right > box.left + 1 && r.left < box.right - 1;
      return !showing || (r.left >= box.left - 1 && r.right <= box.right + 1);
    });
  }), { message: 'a page cut a card' }).toBe(true);

  // A family chip shows only that family, with no names row, and All brings
  // every theme back.
  const chips = page.locator('.pill-bar-btn');
  await expect(chips.first()).toHaveText('All');
  await expect(chips.first()).toHaveAttribute('aria-pressed', 'true');
  const all = await radios.count();
  const second = chips.nth(1);
  const family = await second.innerText();
  await second.click();
  await expect(second).toHaveAttribute('aria-pressed', 'true');
  await expect(strip.locator('.theme-family-name')).toHaveCount(0);
  await expect(strip.locator('.theme-family')).toHaveCount(1);
  await expect(strip.locator('.theme-family')).toHaveAttribute('aria-label', family);
  await chips.first().click();
  await expect(radios).toHaveCount(all);

  // A theme for both modes picks at once, with no confirm, and stays shown.
  const other = strip.locator('.theme-card[role="radio"][aria-checked="false"]:not(:has(.theme-card-modes))').first();
  const name = await other.locator('.theme-card-name').innerText();
  await other.click();
  await expect(strip.locator('.theme-card[role="radio"][aria-checked="true"]')).toContainText(name);

  // The next click elsewhere does what it says: Motion's Reduce presses.
  const reduce = page.getByRole('group', { name: 'Motion' }).getByRole('button', { name: 'Reduce' });
  await reduce.click();
  await expect(reduce).toHaveAttribute('aria-pressed', 'true');
  await expect(radios.first()).toBeVisible();

  // A light-only theme picked in dark asks first. Confirming picks it.
  await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'dark');
  const lightOnly = radios.filter({ hasText: 'Light only' }).first();
  const lightName = await lightOnly.locator('.theme-card-name').innerText();
  await lightOnly.click();
  await page.getByRole('button', { name: 'Switch to light mode' }).click();
  await expect(strip.locator('.theme-card[role="radio"][aria-checked="true"]')).toContainText(lightName);

  // Back on Appearance, the strip opens with the active theme inside it.
  await openSettingsView(page, 'system');
  await openSettingsView(page, 'appearance');
  const active = strip.locator('.theme-card[aria-checked="true"]');
  await expect(active).toContainText(lightName);
  const inside = await strip.evaluate(el => {
    const card = el.querySelector('.theme-card[aria-checked="true"]')!.getBoundingClientRect();
    const box = el.getBoundingClientRect();
    return card.left >= box.left && card.right <= box.right;
  });
  expect(inside, 'the active theme opened outside the strip').toBe(true);
});
