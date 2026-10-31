import { test, expect, type Page } from './fixtures';
import {
  assertHealthy, navigateToApp, ensureOnThreadPane, newThread, waitForVisibleInput, hideDashboardRail,
} from './helpers';

// A composer row narrower than 36rem draws the compact faces: the badge alone
// and a round action button. That holds on a desktop viewport too (ADR 0576).
// Desktop-only (`-desktop.spec.ts`): the phone layout is always compact.

const ROW = '.prompt-actions-row:visible';

/** Cap the composer row at `rem` of content, its own padding added. */
async function capRow(page: Page, rem: number): Promise<void> {
  await page.evaluate((contentRem) => {
    const row = Array.from(document.querySelectorAll<HTMLElement>('.prompt-actions-row'))
      .find((r) => r.getBoundingClientRect().width > 0)!;
    const cs = getComputedStyle(row);
    const remPx = parseFloat(getComputedStyle(document.documentElement).fontSize);
    const padding = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight);
    row.style.maxWidth = `${contentRem * remPx + padding}px`;
  }, rem);
}

async function uncapRow(page: Page): Promise<void> {
  await page.evaluate(() => {
    document.querySelectorAll<HTMLElement>('.prompt-actions-row').forEach((r) => { r.style.maxWidth = ''; });
  });
}

test.describe('The composer row picks its faces by its own width', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await page.setViewportSize({ width: 2200, height: 1000 });
    await hideDashboardRail(page);
    await navigateToApp(page);
    await newThread(page);
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);
    await expect(page.locator(`${ROW} .rung-pill:not([data-pending])`)).toBeVisible();
  });

  test('a wide row keeps the desktop faces, a narrow one the compact faces', async ({ page }) => {
    const row = page.locator(ROW);
    const pill = row.locator('.rung-pill');
    const send = row.locator('.send-cancel-morph');
    await expect(row).not.toHaveAttribute('data-composer-compact', /.*/);
    const wide = { pill: (await pill.boundingBox())!.width, send: (await send.boundingBox())!.width };
    await expect(row.locator('.rung-step').first()).toBeVisible();

    await capRow(page, 30);
    await expect(row).toHaveAttribute('data-composer-compact', 'true');
    await expect(row.locator('.rung-step').first()).toBeHidden();
    await expect.poll(async () => (await send.boundingBox())!.width).toBeLessThan(wide.send);
    await expect.poll(async () => (await pill.boundingBox())!.width).toBeLessThan(wide.pill);
    // Faces move and sizing stays: the row keeps its desktop height.
    expect((await pill.boundingBox())!.height).toBeCloseTo(28, 0);

    await uncapRow(page);
    await expect(row).not.toHaveAttribute('data-composer-compact', /.*/);
    await expect.poll(async () => (await send.boundingBox())!.width).toBeCloseTo(wide.send, 0);
    await expect.poll(async () => (await pill.boundingBox())!.width).toBeCloseTo(wide.pill, 0);
  });
});
