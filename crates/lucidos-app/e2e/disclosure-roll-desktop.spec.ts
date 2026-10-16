/**
 * A `<Disclosure>` rolls in a real browser: its height grows under the row
 * above it while the body slides down, and the clip lifts once it lands.
 * Closing rolls it back up, inert, and then unmounts it. The component's
 * contract is unit-tested; this pins what jsdom cannot, real Web Animations
 * driving real layout. Plan: `docs/plans/2026-09-26-one-disclosure-roll.md`.
 *
 * What's New is the surface because every build has releases to unfold. The
 * slider sits at its slowest, so a roll lasts seconds and frames land mid-way.
 * Desktop-only: it pins a 1280x900 viewport, as `type-scale-settings-desktop`.
 */
import { test, expect, type Page } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';

const SLOWEST = '-10';

test.use({ viewport: { width: 1280, height: 900 } });

async function openWhatsNew(page: Page) {
  await navigateToApp(page);
  await waitForEventStream(page);
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'settings', params: { settings_view: 'whats-new' } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
}

/** One frame of the release's disclosure: the rolling box, its body's
 *  natural height, the body's lift, and whether the body takes input. */
function sample(page: Page, version: string) {
  return page.evaluate((v) => {
    const box = document.querySelector<HTMLElement>(`[data-release="${v}"] .disclosure`);
    if (!box) return null;
    const body = box.querySelector<HTMLElement>('.disclosure-body')!;
    return {
      height: box.getBoundingClientRect().height,
      full: body.offsetHeight,
      lift: new DOMMatrixReadOnly(getComputedStyle(body).transform).m42,
      rolling: box.classList.contains('is-rolling'),
      inert: body.inert,
    };
  }, version);
}

test.describe('the disclosure roll', () => {
  test.beforeEach(async ({ page }) => { await assertHealthy(page); });

  test('a release unfolds under its row and folds back up', async ({ page }) => {
    await page.addInitScript((pos) => localStorage.setItem('lucidos-animation-speed-slider', pos), SLOWEST);
    await openWhatsNew(page);

    const firstShut = page
      .locator('.whats-new-release:visible:has(> .whats-new-release-row > .whats-new-release-header[aria-expanded="false"])')
      .first();
    await expect(firstShut).toBeVisible({ timeout: 15_000 });
    // Pinned by version: the shut-row query stops matching once this one opens.
    const version = (await firstShut.getAttribute('data-release'))!;
    const release = page.locator(`.whats-new-release[data-release="${version}"]:visible`);
    const header = release.locator('.whats-new-release-header');

    await header.click();
    await expect.poll(async () => {
      const f = await sample(page, version);
      return f !== null && f.rolling && f.height > 1 && f.height < f.full - 1;
    }, { message: 'the release never showed a frame mid-roll' }).toBe(true);
    const mid = (await sample(page, version))!;
    expect(mid.lift, 'the body did not slide down from under its row').toBeLessThan(0);
    expect(mid.inert, 'an opening body refused input').toBe(false);

    await expect.poll(async () => (await sample(page, version))?.rolling, { timeout: 15_000 }).toBe(false);
    const landed = (await sample(page, version))!;
    expect(Math.abs(landed.height - landed.full), 'the box did not land on its content').toBeLessThan(1);
    expect(landed.lift).toBe(0);

    await header.click();
    await expect.poll(async () => {
      const f = await sample(page, version);
      return f !== null && f.rolling && f.height < f.full - 1;
    }, { message: 'the release never showed a frame mid-fold' }).toBe(true);
    expect((await sample(page, version))!.inert, 'a leaving body took input').toBe(true);

    await expect(release.locator('.disclosure')).toHaveCount(0, { timeout: 15_000 });
  });
});
