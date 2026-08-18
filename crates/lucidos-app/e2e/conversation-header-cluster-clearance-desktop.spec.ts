import { test, expect, type Page } from './fixtures';
import { assertHealthy, gotoWithRetry, navigateToApp, openThreadDrawer } from './helpers';

/**
 * The Conversation header's centred brand cluster never paints on a flanking
 * control, at any legal pane width and any ui-scale.
 *
 * The bug. On the packaged macOS build the drawer toggle rests at the
 * traffic-lights reserve, not at the row's own padding. The leading end of the
 * row is therefore 80px plus a button, which neither `--brand-side-reserve` nor
 * the Conversation pane's floor knew. At 125% ui-scale a divider dragged to the
 * floor put the back chevron's box 17.5px on the toggle and its badge.
 *
 * The build is stamped rather than run: no WebDriver reaches WKWebView (ADR
 * 0016), so `data-titlebar-overlay` is set on `<html>` the way
 * `titlebar_inset_script` does pre-paint. That is the whole of what moves the
 * leading control sideways. `--titlebar-inset` is deliberately left unset,
 * since it only lifts the row vertically.
 *
 * Desktop-only, hence `-desktop`: the split layout, its divider and this row
 * exist only there. The arithmetic behind the clearance is
 * `src/store/__tests__/conversation-pane-floor.test.ts`.
 */

test.use({ viewport: { width: 1280, height: 800 } });

/** Every ui-scale the sweep covers. 75 is UI_SCALE_MIN, and it matters: there
 *  the px lights reserve is at its widest against a rem-sized row. The old
 *  reserve was narrower than the leading end on both builds at that scale. */
const SCALES = [75, 100, 125, 150];

/** Seeded so a narrowing drag has somewhere to travel from. Clear of the
 *  drawer's own floor at every scale swept (388px at 150%). */
const OPEN_DRAWER_WIDTH = 420;

/** Subpixel tolerance. Boxes are compared, not glyphs: a shared edge is legal
 *  and is exactly what the floor is defined to produce. */
const EPS = 0.6;

interface RowMetrics {
  /** The live root font size. Carried because every box below is rem-sized, and
   *  a scale that has not landed yet is a header measured at the wrong one. */
  rootFontPx: number;
  paneWidth: number;
  toggleRight: number;
  backLeft: number;
  forwardRight: number;
  actionsLeft: number;
}

/** Measure the desktop Conversation row: the centred cluster's two chevrons
 *  against the controls flanking them. Null until everything has a box.
 *
 *  Scoped to `.desktop-header`, because `MobileAppHeader` renders first and
 *  carries copies of these classes at 0x0 under a desktop viewport. */
async function measureRow(page: Page): Promise<RowMetrics | null> {
  return page.evaluate(() => {
    const header = document.querySelector('.desktop-header');
    if (!header) return null;
    const toggle = header.querySelector('.thread-toggle-slot');
    const label = header.querySelector('.pane-header-brand-label');
    const actions = header.querySelector('.pane-header-brand-actions');
    const pane = document.querySelector('.pane-thread');
    if (!toggle || !label || !actions || !pane) return null;
    const chevrons = label.querySelectorAll('.thread-nav-btn');
    if (chevrons.length !== 2) return null;
    const t = toggle.getBoundingClientRect();
    const back = chevrons[0].getBoundingClientRect();
    const forward = chevrons[1].getBoundingClientRect();
    const a = actions.getBoundingClientRect();
    if (t.width === 0 || back.width === 0 || a.width === 0) return null;
    return {
      rootFontPx: parseFloat(getComputedStyle(document.documentElement).fontSize),
      paneWidth: pane.getBoundingClientRect().width,
      toggleRight: t.right,
      backLeft: back.left,
      forwardRight: forward.right,
      actionsLeft: a.left,
    };
  });
}

/** Drag the split divider to `toX` and release. What persists is what the clamp
 *  allowed, so an x past the wall parks the pane on its floor. */
async function dragDividerTo(page: Page, toX: number): Promise<void> {
  const box = await page.locator('.split-divider').boundingBox();
  if (!box) throw new Error('.split-divider not visible');
  await page.mouse.move(box.x + box.width / 2, 400);
  await page.mouse.down();
  await page.mouse.move(toX, 400, { steps: 5 });
  await page.mouse.up();
}

/** Both clearances, named so a failure says which end and in which regime. */
function expectClear(m: RowMetrics, where: string): void {
  expect(
    m.backLeft,
    `${where}: back chevron starts at ${m.backLeft.toFixed(1)}, `
      + `on a drawer toggle ending at ${m.toggleRight.toFixed(1)} `
      + `(Conversation pane ${m.paneWidth.toFixed(0)}px)`,
  ).toBeGreaterThanOrEqual(m.toggleRight - EPS);
  expect(
    m.forwardRight,
    `${where}: forward chevron ends at ${m.forwardRight.toFixed(1)}, `
      + `on an actions cluster starting at ${m.actionsLeft.toFixed(1)} `
      + `(Conversation pane ${m.paneWidth.toFixed(0)}px)`,
  ).toBeLessThanOrEqual(m.actionsLeft + EPS);
}

/** Set the scale, park the divider, and wait for the row to stop moving. Every
 *  control in the row is rem-sized, so a scale change relays out all of them,
 *  and the pane geometry eases for var(--duration-slow) after release.
 *
 *  The root font size is part of what has to settle, not a precondition of it.
 *  `loadPreferences` re-applies the account's own scale over this write when it
 *  lands. A run that measured before waiting for it read a pane clamped at one
 *  scale and a header laid out at another. `waitForScaleApplied` rules that out
 *  once per test; this holds the invariant per sample. */
async function settleRow(page: Page, wantRootPx: number | null, where: string): Promise<RowMetrics> {
  let last: RowMetrics | null = null;
  await expect
    .poll(async () => {
      const m = await measureRow(page);
      const stable = !!m && !!last && Math.abs(m.backLeft - last.backLeft) < 0.5
        && Math.abs(m.paneWidth - last.paneWidth) < 0.5
        && (wantRootPx === null || Math.abs(m.rootFontPx - wantRootPx) < 0.1);
      last = m;
      return stable;
    }, { timeout: 10_000, message: `the row never settled: ${where}` })
    .toBe(true);
  return last!;
}

async function settleAt(page: Page, scale: number, toX: number): Promise<RowMetrics> {
  await page.evaluate(
    (s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`),
    scale,
  );
  await dragDividerTo(page, toX);
  return settleRow(page, 16 * scale / 100, `ui-scale ${scale}`);
}

/** Wait for the app's own preference load to have applied ITS ui-scale.
 *
 *  `loadPreferences` ends in `applyUiScale`, which writes `--user-ui-scale`
 *  inline and persists the scale to this key. Until that has happened, a scale
 *  the sweep writes is one write away from being replaced. */
async function waitForScaleApplied(page: Page): Promise<void> {
  await page.waitForFunction(
    () => localStorage.getItem('lucidos-ui-scale') !== null,
    undefined,
    { timeout: 10_000 },
  );
}

/** Stamp the packaged build's one horizontal difference. */
async function stampOverlayBuild(page: Page): Promise<void> {
  await page.evaluate(() =>
    document.documentElement.setAttribute('data-titlebar-overlay', ''));
}

// The web build is swept alongside the packaged one as a control rather than as
// padding. It never reproduced the bug, its toggle resting at 0.5rem. What it
// says is that the fix did not move the collision to the other client.
test.describe('the Conversation header cluster clears both flanking controls', () => {
  test.beforeEach(async ({ page, context }) => {
    await assertHealthy(page);
    await context.addInitScript((width) => {
      localStorage.setItem('lucidos-split-ratio', '0.4');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
      localStorage.setItem('lucidos-thread-drawer-width', String(width));
    }, OPEN_DRAWER_WIDTH);
    await navigateToApp(page);
    await waitForScaleApplied(page);
  });

  for (const packaged of [true, false]) {
    const build = packaged ? 'packaged macOS' : 'web';

    test(`${build}: at the pane's own floor, drawer shut`, async ({ page }) => {
      // The reported configuration. Dragging hard left parks the pane on its
      // floor. That is the narrowest the clamp allows, and so the worst case
      // the header ever has to lay out in.
      if (packaged) await stampOverlayBuild(page);
      for (const scale of SCALES) {
        expectClear(await settleAt(page, scale, 2), `${build}, drawer shut, ui-scale ${scale}`);
      }
    });

    test(`${build}: at the pane's own floor, drawer open`, async ({ page }) => {
      // The toggle moves to the Conversation pane's leading edge here, so the
      // leading end is one button with no inset at all. Narrower than the shut
      // state on both builds, and swept anyway: the drawer takes its width off
      // the split, so the pane reaches its floor from a different direction.
      if (packaged) await stampOverlayBuild(page);
      await openThreadDrawer(page);
      await page.waitForFunction((want) => {
        const drawer = document.querySelector('.thread-drawer');
        return !!drawer && Math.abs(drawer.getBoundingClientRect().width - want) < 1;
      }, OPEN_DRAWER_WIDTH, { timeout: 5_000 });

      for (const scale of SCALES) {
        expectClear(await settleAt(page, scale, 2), `${build}, drawer open, ui-scale ${scale}`);
      }
    });

    test(`${build}: a layout persisted UNDER the new floor is migrated on load`, async ({ page }) => {
      // The upgrade path, and the only one the reporter actually walks. A
      // stored ratio is a fraction while the floors are px, and `splitRatio` is
      // read straight out of localStorage with no clamp. So raising the floor
      // fixes nothing for anyone already parked below it until they happen to
      // drag the divider. 0.234 of this 1280 viewport is 300px, the old floor,
      // which is where the overlap was reported from.
      await page.evaluate(() => {
        localStorage.setItem('lucidos-split-ratio', '0.234');
      });
      await gotoWithRetry(page, '/');
      await waitForScaleApplied(page);
      if (packaged) await stampOverlayBuild(page);

      const m = await settleRow(page, null, 'after the sub-floor migration');
      expect(
        m.paneWidth,
        `the split stayed at the stored ${m.paneWidth.toFixed(0)}px, under its own floor`,
      ).toBeGreaterThan(300);
      expectClear(m, `${build}, migrated from a sub-floor persisted ratio`);
    });

    test(`${build}: and everywhere between the floor and a wide pane`, async ({ page }) => {
      // The floor is one arm of the clamp and the reserve is the other. The
      // handover between them is where a guessed constant would show: the box
      // stops growing at the reserve and starts being held at its natural
      // width. Sweep the divider across it rather than sampling the two ends.
      if (packaged) await stampOverlayBuild(page);
      for (const toX of [2, 340, 400, 460, 520, 640, 800]) {
        expectClear(await settleAt(page, 125, toX), `${build}, divider at x=${toX}`);
      }
    });
  }
});
