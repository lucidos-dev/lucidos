/**
 * The header review loop.
 *
 * Not an assertion test: it drives the real app to each pane and each
 * connection state and writes PNGs, so a header change can be LOOKED AT before
 * it is applied. On the `mobile-webkit` project that is WebKit at 390x844 with
 * `deviceScaleFactor: 3` and an iPhone UA, the same engine and viewport class
 * as the iOS PWA:
 *
 *   HEADER_SHOTS=1 ./scripts/e2e-browser.sh --no-reset --webkit -f header-shots-mobile.spec.ts
 *
 * `--no-reset` leaves the workspace running, so only the first invocation pays
 * the boot and a retune is seconds. The PNGs land in `test-results/header-shots/`,
 * from where they are published to the workspace's `data/artifacts/design/` and
 * open on the phone with no Apply.
 *
 * ONE TRAP, and it cost a round of wrong conclusions on 2026-08-08. `--no-reset`
 * reuses a RUNNING engine, and the e2e workspace is shared across worktrees, so
 * if another coding-agent thread started it first, that engine's
 * `LUCIDOS_STATIC_DIR` points at ITS `dist/`. Your rebuild then changes nothing
 * the browser sees, and the shots (and every spec) quietly describe someone
 * else's branch. `Engine already running on port …` in the output is the tell.
 * Drop `--no-reset` once to restart it against this worktree.
 *
 * Skipped unless `HEADER_SHOTS=1`: it asserts nothing, so it has no business in
 * the gate every other spec here belongs to, and taking pictures on every run
 * is pure cost.
 *
 * The disconnected state is induced for real, by taking the context offline and
 * letting the EventSource drop, rather than by forcing the signal through a
 * test hook. A hook that lets any script forge the connection light would have
 * to ship to production (the e2e bundle IS the production build, see `__openApp`
 * in main.tsx), and a screenshot is not worth that.
 */
import { test, expect, Page } from './fixtures';
import { assertHealthy, navigateToApp, ensureMobileView, enableMobileHeaderSticky } from './helpers';

const SHOOTING = process.env.HEADER_SHOTS === '1';
const DIR = 'test-results/header-shots';

/** Scoped to the THREAD header on purpose. Both mobile headers carry a menu
 *  toggle, they are all mounted at once, and the threads header comes first in
 *  DOM order, so an unscoped query returns that one. It is deliberately not the
 *  connection light (it is a member of an icon run, see HeaderMark), so it has
 *  no `data-conn` at all and this read would answer null forever. */
function markConn(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    const el = document.querySelector('.mobile-thread-header [data-role="brand-menu-toggle"]');
    return el?.getAttribute('data-conn') ?? null;
  });
}

/** The boot splash is an opaque `inset: 0` overlay that fades out once the app
 *  has painted. A screenshot taken under it is a picture of the splash, whatever
 *  the DOM says: an element screenshot captures the page's rendering at that
 *  box, so `.app-header` came out as a bare blue bar and the full-page shot came
 *  out as "Opening your workspace…". Wait for it to be gone before shooting. */
async function waitForSplashGone(page: Page): Promise<void> {
  await expect.poll(
    () => page.evaluate(() => document.querySelectorAll('.boot-splash').length),
    { timeout: 30_000 },
  ).toBe(0);
}

async function shootHeader(page: Page, name: string): Promise<void> {
  await page.locator('.app-header').screenshot({ path: `${DIR}/${name}.png` });
}

test.describe('Header shots', () => {
  test.skip(!SHOOTING, 'set HEADER_SHOTS=1 to take screenshots');

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await enableMobileHeaderSticky(page);
  });

  test('every pane and both connection states', async ({ page, context }) => {
    await navigateToApp(page);

    await waitForSplashGone(page);
    await ensureMobileView(page, 'thread');
    await expect.poll(() => markConn(page)).toBe('connected');
    await shootHeader(page, '1-thread-connected');

    await page.locator('[data-role="brand-menu-toggle"]:visible').first().tap();
    await expect(page.locator('.brand-menu')).toBeVisible();
    await page.screenshot({ path: `${DIR}/2-menu-open.png` });
    await page.keyboard.press('Escape');
    await expect(page.locator('.brand-menu')).toHaveCount(0);

    await ensureMobileView(page, 'threads');
    await shootHeader(page, '3-threads-drawer-mark');

    await ensureMobileView(page, 'content');
    await shootHeader(page, '4-content-chevrons');

    // Pull the plug for real. The pulse is an animation, so give it a beat past
    // the state flip or the shot can land on the ring's invisible end frame.
    await ensureMobileView(page, 'thread');
    await context.setOffline(true);
    await expect.poll(() => markConn(page), { timeout: 30_000 }).toBe('disconnected');
    await page.waitForTimeout(300);
    await shootHeader(page, '5-disconnected');
    await page.screenshot({ path: `${DIR}/6-disconnected-full.png` });

    await context.setOffline(false);
    await expect.poll(() => markConn(page), { timeout: 30_000 }).toBe('connected');
  });
});
