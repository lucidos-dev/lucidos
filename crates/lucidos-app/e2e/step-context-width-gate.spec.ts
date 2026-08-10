import { test, expect, type Locator, type Page } from './fixtures';
import { assertHealthy, navigateToApp, newThread, revealSteps, sendMessage, waitForResponse } from './helpers';

/** The step row's context counter collapses to the bare percentage when the ROW
 *  is narrow, not when the DEVICE is a phone (`@container step-row` in
 *  steps.css). A phone is just the commonest way to get a narrow row: dragging
 *  the split divider in on a 1280px desktop produces exactly the same crowding,
 *  and the old `viewportIsMobile` gate could not see it.
 *
 *  The first case runs at a desktop viewport on purpose, and that IS the
 *  assertion: the window stays far above the mobile breakpoint throughout, so a
 *  device-shaped gate fails on the narrow pane while a width-shaped one passes.
 *  The second holds the phone case the device gate used to own.
 *
 *  Browser-only by construction: a container query is resolved by layout, so no
 *  unit test can evaluate it. The CSS scan in `inline-step-layout.test.ts` pins
 *  the rules; this pins that they actually swap the counter. */

test.use({ viewport: { width: 1280, height: 800 } });

/** Well clear of the 26rem gate (416px at the desktop 16px root): the row is
 *  the pane minus the transcript gutters, the turn inset and the scrollbar. */
const WIDE_PANE = 860;
/** Well under it, and still above the thread pane's own floor (300px at this
 *  16px root, `minThreadPanePx`), so the clamp lets the drag land here. */
const NARROW_PANE = 330;

async function dragDividerTo(page: Page, toX: number): Promise<void> {
  const box = await page.locator('.split-divider').boundingBox();
  if (!box) throw new Error('.split-divider not visible');
  await page.mouse.move(box.x + box.width / 2, 400);
  await page.mouse.down();
  await page.mouse.move(toX, 400, { steps: 5 });
  await page.mouse.up();
  // Nothing corrects a drag on release any more (ADR 0056), and both targets
  // are inside the pane minimums, so the divider is already where it lands. The
  // poll is for the geometry transition, not for a correction.
  await expect
    .poll(() => page.evaluate(() => document.querySelector('.pane-thread')!.getBoundingClientRect().width))
    .toBeCloseTo(toX, -1);
}

/** A turn with its step rows revealed, resolved to the first row's counter. */
async function counterOfFirstStep(page: Page): Promise<Locator> {
  await newThread(page);
  await sendMessage(page, 'Say "hello world" and nothing else.');
  await waitForResponse(page);

  // Inline steps are hidden by default (`stepsExpanded` in localStorage).
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
    await context.addInitScript(() => {
      localStorage.setItem('lucidos-split-ratio', '0.4');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
    });
    await navigateToApp(page);

    const counter = await counterOfFirstStep(page);
    const full = counter.locator('.step-context-full');
    const compact = counter.locator('.step-context-compact');

    await dragDividerTo(page, WIDE_PANE);
    await expect(full).toBeVisible();
    await expect(compact).toBeHidden();
    // The forms are two different sentences about the same number, so the wide
    // one must actually be the "178k / 1000k (18%)" shape rather than a second
    // copy of the percentage.
    await expect(full).toContainText('/');

    await dragDividerTo(page, NARROW_PANE);
    await expect(compact).toBeVisible();
    await expect(full).toBeHidden();
    await expect(compact).toContainText('%');

    // Back out again: the gate is a live query, not a one-way collapse latched
    // at first layout.
    await dragDividerTo(page, WIDE_PANE);
    await expect(full).toBeVisible();
    await expect(compact).toBeHidden();
  });
});

/** The case the old device gate owned, kept honest by the width one: a phone
 *  has no divider to drag, and its row is under the gate at any pane state. */
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
