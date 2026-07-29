import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer } from './helpers';

/** Free divider drags with a deferred snap (SplitLayout / DrawerDivider +
 *  splitHelpers): a drag lands exactly where the pointer drops it; ~400ms
 *  after release a pane below its minimum width animates to the minimum, or
 *  collapses entirely below half of it. While the drag is live the header
 *  regions track the panes 1:1 (data-pane-resizing disables their geometry
 *  transitions) — the old always-on eases left the header visibly
 *  disconnected from the panels mid-drag.
 *
 *  Desktop-only: the split layout and its header regions only exist on
 *  desktop (mobile swipes between full-screen panes). */

// Generous settle wait: SNAP_DELAY_MS (400) + var(--duration-slow) (300) + margin.
const SETTLE_MS = 1200;
// MIN_THREAD_PANE_PX in splitHelpers.ts / MIN_DRAWER_WIDTH in store.ts.
const MIN_THREAD_PANE = 300;
const MIN_DRAWER = 260;

test.use({ viewport: { width: 1280, height: 800 } });

async function dragDivider(page: Page, selector: string, toX: number, opts: { release?: boolean } = {}) {
  const divider = page.locator(selector);
  const box = await divider.boundingBox();
  if (!box) throw new Error(`${selector} not visible`);
  await page.mouse.move(box.x + box.width / 2, 400);
  await page.mouse.down();
  await page.mouse.move(toX, 400, { steps: 5 });
  if (opts.release !== false) await page.mouse.up();
}

async function threadPaneWidth(page: Page): Promise<number> {
  return page.evaluate(() => document.querySelector('.pane-thread')!.getBoundingClientRect().width);
}

async function drawerWidth(page: Page): Promise<number> {
  return page.evaluate(() => document.querySelector('.thread-drawer')!.getBoundingClientRect().width);
}

/** Viewport x of a point on the thread side of the desktop header that
 *  `AppHeader.isInteractive()` treats as a plain GAP — the empty run between the
 *  leading icon cluster and the centered brand label's visible children.
 *
 *  Derived, not hardcoded, and that is the point. This used to be a literal
 *  `x: 200`, which silently stopped being a gap once the brand label's text grew
 *  left over it: `isInteractive` gates out the label's visible children (they
 *  open the control panel), so the dblclick was ignored, the pane never moved,
 *  and the failure surfaced as an *animation* timeout in a test about animation
 *  — pointing at the wrong subsystem entirely. Reading the live geometry keeps
 *  the point valid as the header evolves, and if the gap ever really does close
 *  the explicit width check below says so instead.
 *
 *  Scoped to `.desktop-header`: `MobileAppHeader` renders FIRST and carries
 *  copies of these classes, so an unscoped `querySelector` returns the hidden
 *  mobile one (the 0x0-rect trap in .claude/rules/frontend.md). */
async function threadHeaderGapX(page: Page): Promise<number> {
  const gap = await page.evaluate(() => {
    const header = document.querySelector('.desktop-header');
    if (!header) return null;
    // Whichever leading icon host is currently the visible one (the pair
    // crossfades on data-thread-drawer-open, so the other has no box).
    const leadingRight = ['.collapsed-thread-actions', '.thread-nav-group']
      .map((sel) => header.querySelector(sel)?.getBoundingClientRect())
      .reduce((max, r) => (r && r.width > 0 ? Math.max(max, r.right) : max), 0);
    const brandText = header.querySelector('.pane-header-title')?.getBoundingClientRect();
    if (!brandText || brandText.width === 0) return null;
    return { from: leadingRight, to: brandText.left };
  });
  expect(gap, 'desktop header: leading cluster / brand label not laid out').not.toBeNull();
  expect(
    gap!.to - gap!.from,
    `no non-interactive gap left on the thread header (leading cluster ends at `
      + `${gap!.from.toFixed(1)}, brand text starts at ${gap!.to.toFixed(1)}) — `
      + `header dblclick has nowhere left to land`,
  ).toBeGreaterThan(16);
  return (gap!.from + gap!.to) / 2;
}

/** openThreadDrawer returns as soon as the drawer has ANY width, but the
 *  open animates over var(--duration-slow) — grabbing the divider mid-slide
 *  reads a stale bounding box and the drag silently misses it. Wait for the
 *  default width (300, no persisted width in these tests) to settle. */
async function openDrawerAndSettle(page: Page): Promise<void> {
  await openThreadDrawer(page);
  await page.waitForFunction(() => {
    const drawer = document.querySelector('.thread-drawer');
    return drawer && Math.abs(drawer.getBoundingClientRect().width - 300) < 1;
  }, undefined, { timeout: 5_000 });
}

test.describe('Split layout — free drag with deferred snap', () => {
  test.beforeEach(async ({ page, context }) => {
    await assertHealthy(page);
    await context.addInitScript(() => {
      localStorage.setItem('lucidos-split-ratio', '0.4');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
      localStorage.removeItem('lucidos-thread-drawer-width');
    });
    await navigateToApp(page);
  });

  test('drag above the minimum lands exactly where released and stays', async ({ page }) => {
    await dragDivider(page, '.split-divider', 560);
    expect(Math.abs(await threadPaneWidth(page) - 560)).toBeLessThanOrEqual(2);

    // No snap should fire — both sides are above their minimums.
    await page.waitForTimeout(SETTLE_MS);
    expect(Math.abs(await threadPaneWidth(page) - 560)).toBeLessThanOrEqual(2);
  });

  test('release between half-min and min snaps the thread pane to the minimum', async ({ page }) => {
    await dragDivider(page, '.split-divider', 200);
    // Free landing first…
    expect(Math.abs(await threadPaneWidth(page) - 200)).toBeLessThanOrEqual(2);
    // …then the deferred snap brings it to the minimum.
    await expect.poll(() => threadPaneWidth(page), { timeout: 5_000 }).toBeGreaterThanOrEqual(MIN_THREAD_PANE - 1);
    expect(Math.abs(await threadPaneWidth(page) - MIN_THREAD_PANE)).toBeLessThanOrEqual(2);
  });

  test('release below half-min collapses the thread pane — but never mid-drag', async ({ page }) => {
    // Mid-drag, even hard against the left edge, the collapse state must not
    // flip: collapse attributes swap header icon groups between hosts, and
    // flipping them while the pointer wiggles across the edge makes the icons
    // dance between the headers. Collapse belongs to the post-release snap.
    await dragDivider(page, '.split-divider', 2, { release: false });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    expect(await page.evaluate(() =>
      document.documentElement.hasAttribute('data-thread-collapsed'))).toBe(false);
    await expect(page.locator('.pane-thread')).not.toHaveClass(/pane-collapsed/);

    await page.mouse.up();
    await expect(page.locator('.pane-thread')).toHaveClass(/pane-collapsed/, { timeout: 5_000 });
  });

  test('header regions track the panes 1:1 while dragging', async ({ page }) => {
    await dragDivider(page, '.split-divider', 500, { release: false });

    const measure = () => page.evaluate(() => {
      const region = document.querySelector('.content-header-elements')!.getBoundingClientRect();
      const pane = document.querySelector('.pane-content')!.getBoundingClientRect();
      return Math.abs(region.left - pane.left);
    });

    // Mid-drag, after a paint: the header region's left edge must sit on the
    // content pane's left edge. With the old always-on 300ms ease the region
    // would still be far from the pane right after a move.
    await page.evaluate(() => new Promise(requestAnimationFrame));
    expect(await measure()).toBeLessThanOrEqual(1.5);

    await page.mouse.move(820, 400, { steps: 2 });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    expect(await measure()).toBeLessThanOrEqual(1.5);

    await page.mouse.up();
  });

  test('drawer release between half-min and min snaps to the drawer minimum', async ({ page }) => {
    await openDrawerAndSettle(page);
    await dragDivider(page, '.drawer-divider', 180);
    // Free landing first…
    expect(Math.abs(await drawerWidth(page) - 180)).toBeLessThanOrEqual(2);
    // …then the deferred snap. Two-sided assertion: landing back at the
    // default 300 would mean the drag never happened.
    await expect.poll(() => drawerWidth(page), { timeout: 5_000 }).toBeGreaterThanOrEqual(MIN_DRAWER - 1);
    expect(Math.abs(await drawerWidth(page) - MIN_DRAWER)).toBeLessThanOrEqual(2);
  });

  test('drawer release below half-min closes the drawer; reopening restores a usable width', async ({ page }) => {
    await openDrawerAndSettle(page);
    await dragDivider(page, '.drawer-divider', 80);
    await expect(page.locator('.thread-drawer')).toHaveClass(/thread-drawer-collapsed/, { timeout: 5_000 });

    await openThreadDrawer(page);
    await expect.poll(() => drawerWidth(page), { timeout: 5_000 }).toBeGreaterThanOrEqual(MIN_DRAWER - 1);
  });

  test('widening the drawer below the thread-pane minimum snaps the thread pane too', async ({ page }) => {
    await openDrawerAndSettle(page);
    // The drawer drag preserves the content pane, so widening the drawer
    // eats the thread pane (~390px at ratio 0.4): dragging the drawer to
    // ~490px lands the thread pane around 200px — below its minimum but
    // above half of it. The release snap must restore it to the minimum.
    await dragDivider(page, '.drawer-divider', 490);
    expect(await threadPaneWidth(page)).toBeLessThan(250);
    await expect.poll(() => threadPaneWidth(page), { timeout: 5_000 }).toBeGreaterThanOrEqual(MIN_THREAD_PANE - 1);
  });

  test('drawer toggle slides the drawer header shut with the drawer — no pop', async ({ page }) => {
    await openDrawerAndSettle(page);

    // openDrawerAndSettle opened the drawer; the toggle is a plain show/hide, so
    // this second click simply hides it (no focus stage in between).
    await page.locator('button[aria-label="Show or hide thread drawer"]:visible').first().click();
    // The drawer header must pass through intermediate widths: it stays
    // mounted and its width rides --content-offset through the transition.
    // The old conditional render unmounted it at toggle time — width would
    // jump straight to 0 and this wait would time out at an intermediate
    // sample... by never seeing one (it polls every frame).
    await page.waitForFunction(() => {
      const header = document.querySelector('.threads-header');
      if (!header) return false; // unmounting is exactly the pop we're pinning against
      const w = header.getBoundingClientRect().width;
      return w > 20 && w < 280;
    }, undefined, { timeout: 2_000 });

    // After the transition: still mounted, slid to zero, out of the tab order.
    await expect.poll(
      () => page.evaluate(() => {
        const header = document.querySelector('.threads-header');
        if (!header) return -1;
        return header.getBoundingClientRect().width;
      }),
      { timeout: 5_000 },
    ).toBeLessThanOrEqual(1);
    // Polled: the visibility transition flips to hidden right as the width
    // lands — a single sample could race it by a frame.
    await expect.poll(
      () => page.evaluate(() =>
        getComputedStyle(document.querySelector('.threads-header')!).visibility),
      { timeout: 2_000 },
    ).toBe('hidden');
  });

  test('maximizing the content pane fades the thread-side header instead of popping it', async ({ page }) => {
    // Double-click the divider → thread pane collapses (content maximized).
    const divider = page.locator('.split-divider');
    await divider.dblclick();

    // The brand must fade through intermediate opacities — display:none or
    // unmount would make it vanish at the start of the pane animation.
    // .desktop-header scoping matters: MobileAppHeader renders an earlier
    // .pane-header-brand copy that is display:none on desktop, where
    // transitions don't run and opacity flips instantly.
    await page.waitForFunction(() => {
      const brand = document.querySelector('.desktop-header .pane-header-brand');
      if (!brand) return false;
      const opacity = parseFloat(getComputedStyle(brand).opacity);
      return opacity > 0.05 && opacity < 0.95;
    }, undefined, { timeout: 2_000 });

    // Settled: still mounted, fully faded, hidden from interaction.
    await expect.poll(
      () => page.evaluate(() => {
        const brand = document.querySelector('.desktop-header .pane-header-brand');
        if (!brand) return -1;
        return parseFloat(getComputedStyle(brand).opacity);
      }),
      { timeout: 5_000 },
    ).toBe(0);
    // Polled: visibility flips at the opacity transition's end — a single
    // sample could race it by a frame.
    await expect.poll(
      () => page.evaluate(() =>
        getComputedStyle(document.querySelector('.desktop-header .pane-header-brand')!).visibility),
      { timeout: 2_000 },
    ).toBe('hidden');
  });

  test('maximizing the thread pane animates the pane body, not just the header', async ({ page }) => {
    // Double-click the thread side of the header → content pane collapses, the
    // thread pane maximizes to full width. The pane body must animate to full
    // width over var(--duration-slow) like its header does — the bug was that
    // the pane jumped instantly (its flex switched from basis-driven to
    // grow-driven, and .snap-animate only transitioned flex-basis) while the
    // header slid smoothly, so they read as disconnected.
    const startWidth = await threadPaneWidth(page); // ~512 at ratio 0.4 / 1280
    expect(startWidth).toBeLessThan(700);

    // Land on a real non-interactive gap on the thread side, so
    // onHeaderDblClick actually maximizes the thread pane (see
    // threadHeaderGapX — a hardcoded x silently drifted onto the brand text).
    const header = page.locator('.app-header');
    const headerBox = await header.boundingBox();
    if (!headerBox) throw new Error('.app-header not visible');
    const gapX = await threadHeaderGapX(page);
    await header.dblclick({ position: { x: gapX - headerBox.x, y: 20 } });

    // The pane width must pass through an intermediate value during the
    // transition. With the bug it snaps straight to full width and this never
    // samples an in-between value (waitForFunction would time out).
    await page.waitForFunction(() => {
      const w = document.querySelector('.pane-thread')!.getBoundingClientRect().width;
      return w > 650 && w < 1150;
    }, undefined, { timeout: 2_000 });

    // Settled: thread fills the row (only the divider + its margin remain).
    await expect.poll(() => threadPaneWidth(page), { timeout: 5_000 }).toBeGreaterThan(1150);
  });
});
