import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp, newThread, waitForVisibleInput, isMobileViewport } from './helpers';

/** The composer's height is only ever right for the width it was measured at.
 *  Narrow the Conversation pane and the same paragraph needs more lines; widen
 *  it and it needs fewer. `resizeTextarea` reads the VALUE, which a resize never
 *  touches, so without `useWidthRemeasure` the box keeps the height it had:
 *  clipped after a narrowing, and stuck tall after a widening.
 *
 *  The zero-width stand-down that rides with it is unit-tested
 *  (`components/chat/__tests__/prompt-resize.test.ts`). What only a browser can
 *  say is that the height really does follow the pane, so that is what this
 *  asserts.
 *
 *  Desktop-only: the split layout exists only here. Mobile swipes between
 *  full-screen panes, so the composer has one width. */

test.use({ viewport: { width: 1280, height: 800 } });

// Two divider positions far enough apart that the paragraph below takes a
// different number of lines at each. The narrow one is past the Conversation
// pane's floor on purpose: a drag there clamps, so it lands on the wall itself.
const WIDE_X = 700;
const NARROW_X = 150;
// What the pane must actually measure at each end. A drag that silently missed
// the divider would leave the pane put, and the height assertions would then
// pass for the wrong reason.
const WIDE_FLOOR = 600;
const NARROW_CEILING = 400;

// One paragraph, no newlines, so every line break is the box's own wrapping
// decision and the line count is a pure function of the width.
const PARAGRAPH =
  'This single paragraph carries no newline of its own, so every line break in '
  + 'the composer is one the box chose for the width it currently has.';

// The pane geometry settles within var(--duration-slow) (300ms).
const SETTLE_MS = 600;

async function dragDivider(page: Page, toX: number) {
  const box = await page.locator('.split-divider').boundingBox();
  if (!box) throw new Error('.split-divider not visible');
  await page.mouse.move(box.x + box.width / 2, 400);
  await page.mouse.down();
  await page.mouse.move(toX, 400, { steps: 5 });
  await page.mouse.up();
  await page.waitForTimeout(SETTLE_MS);
}

/** The box's own height, the height its content wants, and the pane it sits in.
 *  `ch >= sh` means the whole draft is visible; a taller `sh` means the bottom
 *  lines are clipped. `pane` is carried so a drag that went nowhere says so. */
function metrics(page: Page) {
  return page.locator('[data-role="prompt-input"]:visible').first()
    .evaluate((el: HTMLTextAreaElement) => ({
      ch: el.clientHeight,
      sh: el.scrollHeight,
      pane: Math.round(document.querySelector('.pane-thread')!.getBoundingClientRect().width),
    }));
}

test.describe('the composer re-measures when its pane changes width', () => {
  test.beforeEach(async ({ page, context }) => {
    await assertHealthy(page);
    await context.addInitScript(() => {
      localStorage.setItem('lucidos-split-ratio', '0.4');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
    });
    await navigateToApp(page);
  });

  test('a draft is neither clipped when narrowed nor left tall when widened', async ({ page }) => {
    test.skip(isMobileViewport(page), 'the split layout is desktop-only');

    await newThread(page);
    const input = await waitForVisibleInput(page);
    await input.fill(PARAGRAPH);

    await dragDivider(page, WIDE_X);
    const wide = await metrics(page);
    expect(wide.pane, 'the widening drag moved the divider').toBeGreaterThan(WIDE_FLOOR);
    expect(wide.ch, 'the draft starts un-clipped in the wide pane').toBeGreaterThanOrEqual(wide.sh - 4);

    await dragDivider(page, NARROW_X);
    const narrow = await metrics(page);
    expect(narrow.pane, 'the narrowing drag reached the pane floor').toBeLessThan(NARROW_CEILING);
    expect(narrow.ch, 'the paragraph takes more lines in the narrow pane').toBeGreaterThan(wide.ch);
    expect(narrow.ch, 'and the box grew to show them all').toBeGreaterThanOrEqual(narrow.sh - 4);

    await dragDivider(page, WIDE_X);
    const again = await metrics(page);
    expect(again.pane, 'the pane is wide again').toBeGreaterThan(WIDE_FLOOR);
    expect(again.ch, 'the box came back down with the width').toBeLessThanOrEqual(wide.ch + 4);
    expect(again.ch, 'still showing the whole draft').toBeGreaterThanOrEqual(again.sh - 4);
  });

  // Zero is a width the composer really reaches: a collapsed pane keeps it in
  // layout rather than unmounting it, and a textarea's `overflow-wrap:
  // break-word` puts every character on its own line there. So the round trip
  // has to end at the composer's own height, never at the `max-height: 40vh`
  // cap a per-character measurement lands on.
  test('collapsing the pane and reopening it leaves the composer its own height', async ({ page }) => {
    test.skip(isMobileViewport(page), 'the split layout is desktop-only');

    await newThread(page);
    const input = await waitForVisibleInput(page);
    await input.fill(PARAGRAPH);
    await dragDivider(page, WIDE_X);
    const before = await metrics(page);
    expect(before.pane, 'the widening drag moved the divider').toBeGreaterThan(WIDE_FLOOR);

    // Double-click collapses the Conversation pane; a second one restores it.
    await page.locator('.split-divider').dblclick();
    await expect
      .poll(() => page.evaluate(() => document.querySelector('.pane-thread')!.getBoundingClientRect().width))
      .toBe(0);
    await page.locator('.split-divider').dblclick();
    await page.waitForTimeout(SETTLE_MS);
    await dragDivider(page, WIDE_X);

    const after = await metrics(page);
    expect(after.pane, 'the pane is wide again').toBeGreaterThan(WIDE_FLOOR);
    expect(after.ch, 'no cap-height box after the round trip').toBeLessThanOrEqual(before.ch + 4);
    expect(after.ch, 'and the draft is still whole').toBeGreaterThanOrEqual(after.sh - 4);
  });
});
