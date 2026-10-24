/**
 * The DESKTOP header review loop, the counterpart to `header-shots-mobile`.
 *
 * Not an assertion test: it drives the real app and writes PNGs, so a header
 * change can be LOOKED AT before it is applied.
 *
 *   HEADER_SHOTS=1 ./scripts/e2e-browser.sh --no-reset -f header-shots-desktop.spec.ts
 *
 * The PNGs land in `test-results/header-shots/`, from where they are published
 * to the workspace's `data/artifacts/design/` and open in the app with no Apply.
 * Skipped unless `HEADER_SHOTS=1`: it asserts nothing, so it has no business in
 * the gate every other spec here belongs to.
 *
 * The `--no-reset` trap from the mobile spec applies here word for word: it
 * reuses a RUNNING engine, and the e2e workspace is shared across worktrees, so
 * `Engine already running on port …` means the shots may describe someone
 * else's branch. Drop the flag once to restart it against this worktree.
 *
 * THE CONNECTION STATES ARE SHOWN TWO WAYS, deliberately.
 * `5-conn-*` is the state induced FOR REAL, by taking the context offline and
 * letting the EventSource drop: that is the honest picture, but it can only
 * reach the two settled states, and `connecting` lives in the gap between them.
 * `4-conn-strip` splices three copies of the mark into the live header with the
 * three `data-conn` values on them, so all three read side by side under the
 * same stylesheet and the same blue bar. It is a probe of the real cascade, not
 * a mock: the clones ARE the header's own markup, and nothing but the attribute
 * differs between them. The one thing a still cannot show is that `connecting`
 * breathes rather than sitting at the dim value it was caught at.
 */
import { test, expect, Page } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';

const SHOOTING = process.env.HEADER_SHOTS === '1';
const DIR = 'test-results/header-shots';

test.use({ viewport: { width: 1280, height: 800 } });

function markConn(page: Page): Promise<string | null> {
  return page.evaluate(() =>
    document.querySelector('.desktop-header [data-role="brand-menu-toggle"]')?.getAttribute('data-conn') ?? null);
}

/** The boot splash is an opaque `inset: 0` overlay that fades out once the app
 *  has painted. A screenshot taken under it is a picture of the splash. */
async function waitForSplashGone(page: Page): Promise<void> {
  await expect.poll(
    () => page.evaluate(() => document.querySelectorAll('.boot-splash').length),
    { timeout: 30_000 },
  ).toBe(0);
}

async function shootHeader(page: Page, name: string): Promise<void> {
  await page.locator('.app-header').screenshot({ path: `${DIR}/${name}.png` });
}

test.describe('Desktop header shots', () => {
  test.skip(!SHOOTING, 'set HEADER_SHOTS=1 to take screenshots');

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('the bar, the menu, and every connection state', async ({ page, context }) => {
    await navigateToApp(page);
    await waitForSplashGone(page);
    await expect.poll(() => markConn(page)).toBe('connected');

    await shootHeader(page, 'desktop-1-bar');

    await page.locator('.desktop-header [data-role="brand-menu-toggle"]').click();
    await expect(page.locator('.brand-menu')).toBeVisible();
    await page.screenshot({ path: `${DIR}/desktop-2-menu.png` });
    await page.keyboard.press('Escape');
    await expect(page.locator('.brand-menu')).toHaveCount(0);

    // The three states side by side, spliced into the live bar so they render
    // through the real cascade. Labelled, because two of the three differ only
    // in strength and a caption is the only way a still says which is which.
    await page.evaluate(() => {
      const label = document.querySelector('.desktop-header .pane-header-brand-label') as HTMLElement;
      const mark = label.querySelector('.brand-mark') as HTMLElement;
      const strip = document.createElement('div');
      strip.className = 'conn-strip';
      strip.style.cssText = 'display:flex;align-items:center;gap:2rem;margin-left:2rem';
      for (const state of ['connected', 'connecting', 'disconnected']) {
        const cell = document.createElement('div');
        cell.style.cssText = 'display:flex;align-items:center;gap:0.4rem;color:#fff;font:600 12px system-ui';
        const clone = mark.cloneNode(true) as HTMLElement;
        clone.setAttribute('data-conn', state);
        const caption = document.createElement('span');
        caption.textContent = state;
        cell.append(clone, caption);
        strip.append(cell);
      }
      label.append(strip);
      // The label is a fixed span with its content pinned to the ends; let the
      // strip have the room it needs for the shot.
      label.style.width = 'auto';
      label.style.justifyContent = 'flex-start';
    });
    await page.waitForTimeout(300);
    await shootHeader(page, 'desktop-4-conn-strip');
    await page.reload();
    await waitForSplashGone(page);

    // ...and the real thing, for the state that can be induced honestly.
    await context.setOffline(true);
    await expect.poll(() => markConn(page), { timeout: 30_000 }).toBe('disconnected');
    await page.waitForTimeout(300);
    await shootHeader(page, 'desktop-5-conn-disconnected');

    await context.setOffline(false);
    await expect.poll(() => markConn(page), { timeout: 30_000 }).toBe('connected');
    await shootHeader(page, 'desktop-6-conn-connected');
  });
});
