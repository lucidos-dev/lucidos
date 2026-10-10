/**
 * Mobile: the theme carousel opens on its left edge and pages one card at a
 * time, the first press centring the third theme. Its chips and chevrons stay
 * inside the settings column and above the pane's edge-swipe strips, so a tap
 * reaches them. The strip scrolls alone, so the page never scrolls sideways.
 */
import { test, expect } from './fixtures';
import { assertHealthy, isMobileViewport, navigateToApp, openSettingsView, waitForEventStream, waitForPaneAtRest } from './helpers';

test('the theme carousel fits a phone and pages on a tap', async ({ page }) => {
  test.skip(!isMobileViewport(page), 'Phone layout only, so the desktop project skips it');
  await assertHealthy(page);
  await navigateToApp(page);
  await waitForEventStream(page);
  await openSettingsView(page, 'appearance');

  const strip = page.locator('.theme-carousel');
  const next = page.getByRole('button', { name: 'Next themes' });
  const previous = page.getByRole('button', { name: 'Previous themes' });
  await expect(next).toBeVisible({ timeout: 15_000 });
  // Opening Settings swipes the panes: measure once the content pane settles.
  await waitForPaneAtRest(page);

  const column = await page.locator('.settings-panel').boundingBox();
  for (const chevron of [previous, next]) {
    const box = await chevron.boundingBox();
    expect(box && column && box.x >= column.x && box.x + box.width <= column.x + column.width, 'a chevron sits outside the column').toBe(true);
  }
  const pageScrollsSideways = await page.locator('.content-pane-body').evaluate(el => el.scrollWidth > el.clientWidth);
  expect(pageScrollsSideways, 'the carousel widened the page').toBe(false);

  await strip.evaluate(el => { el.scrollLeft = 0; el.dispatchEvent(new Event('scroll')); });
  await expect(previous).toBeDisabled();
  // Opens with the first card on the strip's left edge, not centred.
  const firstLeft = await strip.evaluate(el => {
    const card = el.querySelector('.theme-card')!.getBoundingClientRect();
    return Math.abs(card.left - el.getBoundingClientRect().left) < 12;
  });
  expect(firstLeft, 'the strip opened with an empty left side').toBe(true);

  const centred = () => strip.evaluate(el => {
    const box = el.getBoundingClientRect();
    const middle = box.left + box.width / 2;
    const cards = [...el.querySelectorAll('.theme-card')];
    const distance = (card: Element) => {
      const r = card.getBoundingClientRect();
      return Math.abs(r.left + r.width / 2 - middle);
    };
    return cards.reduce((best, card, i) => (distance(card) < distance(cards[best]) ? i : best), 0);
  });
  await next.tap();
  await expect.poll(centred, { message: 'the first press did not centre the third theme' }).toBe(2);
  await expect(previous).toBeEnabled();
  await previous.tap();
  await expect.poll(centred, { message: 'the left chevron did not page back' }).toBe(1);

  // The first chip sits at the edge-swipe strip and still takes a tap.
  const chips = page.locator('.pill-bar-btn');
  await chips.nth(1).tap();
  await expect(chips.nth(1)).toHaveAttribute('aria-pressed', 'true');
  await chips.first().tap();
  await expect(chips.first()).toHaveAttribute('aria-pressed', 'true');
});
