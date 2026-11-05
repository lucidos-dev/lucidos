import { test, expect, type Page } from './fixtures';
import { assertHealthy, minThreadPane, navigateToApp, DRAWER_TOGGLE_LABEL } from './helpers';
import { computeMinContentPaneWidth } from '../src/store/paneMinimums';
import { THREAD_DRAWER_WIDTH_KEY } from '../src/store/drawerKeys';
import { DASHBOARD_RAIL_SHOWN_KEY, DASHBOARD_RAIL_WIDTH_KEY } from '../src/store/dashboardRailKeys';

/** No two visible header controls overlap, at any window width, UI scale,
 *  drawer state and rail state (ADR 0495), and the Conversation is never drawn
 *  below its floor.
 *
 *  The reported cases: the split could not hold both pane floors, so it
 *  squeezed the Canvas to a sliver, and the dashboard rail toggle landed on Search
 *  everywhere. The Canvas now yields instead. The drawer and the rail squeezed
 *  the Conversation the same way, and now yield too, rail first. A yield is
 *  drawn only: the stored layout never changes for lack of room. */

const SETTLE_MS = 800;
const STORED_RATIO = '0.5';
const WIDTHS = [900, 1100, 1280, 1600];
const SCALES = ['100%', '125%'];

test.beforeEach(async ({ page }) => {
  await page.addInitScript((ratio) => {
    localStorage.setItem('lucidos-split-ratio', ratio);
  }, STORED_RATIO);
});

test.afterEach(async ({ page }) => {
  await assertHealthy(page);
});

interface Drawn {
  label: string;
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** Every header control a reader can see, as the box it paints after its
 *  ancestors' clips. A control faded out, hidden or clipped to nothing is not
 *  drawn, so it cannot overlap anything. */
async function drawnHeaderControls(page: Page): Promise<Drawn[]> {
  return page.evaluate(() => {
    const selector = '.desktop-header .icon-btn, .dashboard-rail-toggle-slot .icon-btn, .dashboard-rail .dashboard-head .icon-btn';
    // An `inset()` clip as edge offsets in px: 1 to 4 lengths or percentages.
    const insets = (clip: string, w: number, h: number): number[] | null => {
      const m = /^inset\(([^)]*)\)/.exec(clip);
      if (!m) return null;
      const parts = m[1].trim().split(/\s+/);
      const [t, r = t, b = t, l = r] = parts;
      const px = (v: string, size: number) => (v.endsWith('%') ? (parseFloat(v) / 100) * size : parseFloat(v) || 0);
      return [px(t, h), px(r, w), px(b, h), px(l, w)];
    };
    const drawn: Drawn[] = [];
    for (const el of document.querySelectorAll<HTMLElement>(selector)) {
      let { left, top, right, bottom } = el.getBoundingClientRect();
      let shown = true;
      for (let node: HTMLElement | null = el; node && shown; node = node.parentElement) {
        const style = getComputedStyle(node);
        if (style.visibility === 'hidden' || style.display === 'none' || parseFloat(style.opacity) === 0) shown = false;
        if (node === el) continue;
        const r = node.getBoundingClientRect();
        const clip = insets(style.clipPath, r.width, r.height);
        if (clip) {
          top = Math.max(top, r.top + clip[0]);
          right = Math.min(right, r.right - clip[1]);
          bottom = Math.min(bottom, r.bottom - clip[2]);
          left = Math.max(left, r.left + clip[3]);
        } else if (style.overflowX !== 'visible') {
          left = Math.max(left, r.left);
          right = Math.min(right, r.right);
        }
      }
      if (!shown || right - left < 2 || bottom - top < 2) continue;
      drawn.push({
        label: el.getAttribute('aria-label') ?? el.className,
        left, top, right, bottom,
      });
    }
    return drawn;
  });
}

function overlaps(a: Drawn, b: Drawn): boolean {
  const w = Math.min(a.right, b.right) - Math.max(a.left, b.left);
  const h = Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top);
  return w > 0.5 && h > 0.5;
}

async function setFlag(page: Page, attr: string, want: boolean, toggle: () => Promise<void>): Promise<void> {
  const has = () => page.evaluate((a) => document.documentElement.hasAttribute(a), attr);
  if ((await has()) !== want) await toggle();
  await expect.poll(has).toBe(want);
}

const DRAWER_OPEN_KEY = 'lucidos-thread-drawer-open';
const toggleDrawer = (page: Page) => page.getByRole('button', { name: DRAWER_TOGGLE_LABEL }).first().click();
const drawerToggle = (page: Page) => page.locator(`.thread-toggle-slot button[aria-label^="${DRAWER_TOGGLE_LABEL}"]`);
const railToggle = (page: Page) => page.locator('.dashboard-rail-toggle');

/** Every key a yield must never write: the split ratio, both widths, and the
 *  two flags the user's own toggles write. */
const STORED_KEYS = ['lucidos-split-ratio', THREAD_DRAWER_WIDTH_KEY, DASHBOARD_RAIL_WIDTH_KEY, DRAWER_OPEN_KEY, DASHBOARD_RAIL_SHOWN_KEY];

const storedLayout = (page: Page) =>
  page.evaluate((keys) => keys.map((key) => localStorage.getItem(key)), STORED_KEYS);

/** Start the app from a stored drawer and rail state. A reload is what a
 *  returning user does, and it keeps the toggles out of the sweep: a yielded
 *  pane's toggle swaps or is disabled, so it cannot set a state the room
 *  forbids. */
async function startFrom(page: Page, drawerOpen: boolean, railShown: boolean): Promise<void> {
  await page.evaluate(([open, shown, openKey, shownKey]) => {
    localStorage.setItem(openKey as string, String(open));
    localStorage.setItem(shownKey as string, String(shown));
  }, [drawerOpen, railShown, DRAWER_OPEN_KEY, DASHBOARD_RAIL_SHOWN_KEY]);
  await navigateToApp(page);
}

test('header controls never overlap, the Conversation keeps its floor, and the stored layout survives every window', async ({ page }) => {
  test.setTimeout(240_000);
  await navigateToApp(page);
  const railExists = (await railToggle(page).count()) > 0;
  const failures: string[] = [];

  for (const drawerOpen of [false, true]) {
    for (const railShown of railExists ? [false, true] : [false]) {
      await startFrom(page, drawerOpen, railShown);
      const stored = await storedLayout(page);
      for (const scale of SCALES) {
        await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', s), scale);
        for (const width of WIDTHS) {
          await page.setViewportSize({ width, height: 800 });
          await page.waitForTimeout(SETTLE_MS);
          const where = `${width}px at ${scale}, drawer ${drawerOpen ? 'stored open' : 'stored shut'}, rail ${railShown ? 'stored shown' : 'stored hidden'}`;

          const controls = await drawnHeaderControls(page);
          for (let i = 0; i < controls.length; i++) {
            for (let j = i + 1; j < controls.length; j++) {
              if (overlaps(controls[i], controls[j])) failures.push(`${where}: "${controls[i].label}" overlaps "${controls[j].label}"`);
            }
          }
          if (railExists && !controls.some(c => /dashboard/.test(c.label))) {
            failures.push(`${where}: the dashboard rail toggle is not drawn`);
          }
          const panes = await page.evaluate(() => ({
            conversationCollapsed: document.documentElement.hasAttribute('data-thread-collapsed'),
            conversationPx: document.querySelector('.pane-thread')?.getBoundingClientRect().width ?? 0,
            canvasCollapsed: document.documentElement.hasAttribute('data-content-collapsed'),
            canvasPx: document.querySelector('.pane-content')?.getBoundingClientRect().width ?? 0,
            remPx: parseFloat(getComputedStyle(document.documentElement).fontSize),
          }));
          const conversationFloorPx = await minThreadPane(page);
          if (!panes.conversationCollapsed && panes.conversationPx < conversationFloorPx - 1) {
            failures.push(`${where}: the Conversation is ${panes.conversationPx}px, under its ${conversationFloorPx}px floor`);
          }
          const canvasFloorPx = computeMinContentPaneWidth(panes.remPx);
          if (!panes.canvasCollapsed && panes.canvasPx < canvasFloorPx - 1) {
            failures.push(`${where}: an open Canvas is ${panes.canvasPx}px, under its ${canvasFloorPx}px floor`);
          }
        }
      }
      expect(await storedLayout(page), `a yield wrote the stored layout, starting from drawer ${drawerOpen}, rail ${railShown}`).toEqual(stored);
    }
  }

  expect(failures).toEqual([]);
});

test('a window too narrow for the drawer disables its toggle, and says why', async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 800 });
  await navigateToApp(page);
  await page.evaluate(() => document.documentElement.style.setProperty('--user-ui-scale', '125%'));
  await page.waitForTimeout(SETTLE_MS);
  const toggle = drawerToggle(page).first();
  await expect(toggle).toHaveAttribute('aria-disabled', 'true');
  await expect(toggle).toHaveAttribute('data-tooltip', /too narrow/);
  const before = await storedLayout(page);
  // Playwright refuses an aria-disabled button, so force the tap the handler must ignore.
  await toggle.click({ force: true });
  await page.waitForTimeout(SETTLE_MS);
  expect(await page.evaluate((a) => document.documentElement.hasAttribute(a), 'data-thread-drawer-open')).toBe(false);
  expect(await storedLayout(page)).toEqual(before);
});

test('showing a yielded rail swaps it for the drawer, and the drawer takes the room back', async ({ page }) => {
  await page.setViewportSize({ width: 1100, height: 800 });
  await navigateToApp(page);
  test.skip((await railToggle(page).count()) === 0, 'the workspace has no Home, so no rail');
  await startFrom(page, true, true);
  const drawn = (attr: string) => page.evaluate((a) => document.documentElement.hasAttribute(a), attr);

  // Both side panes cannot fit beside the Conversation: the rail yields first.
  await expect.poll(() => drawn('data-dashboard-rail')).toBe(false);
  expect(await drawn('data-thread-drawer-open')).toBe(true);
  await expect(railToggle(page)).toHaveAttribute('aria-pressed', 'false');
  expect(await page.evaluate((k) => localStorage.getItem(k), DASHBOARD_RAIL_SHOWN_KEY)).toBe('true');

  // Showing it closes the drawer for real, since that is what makes room.
  await railToggle(page).click();
  await expect.poll(() => drawn('data-dashboard-rail')).toBe(true);
  expect(await drawn('data-thread-drawer-open')).toBe(false);
  expect(await page.evaluate((k) => localStorage.getItem(k), DRAWER_OPEN_KEY)).toBe('false');

  // Showing the drawer needs no swap: the rail yields to it, drawn-only.
  await toggleDrawer(page);
  await expect.poll(() => drawn('data-thread-drawer-open')).toBe(true);
  await expect.poll(() => drawn('data-dashboard-rail')).toBe(false);
  expect(await page.evaluate((k) => localStorage.getItem(k), DASHBOARD_RAIL_SHOWN_KEY)).toBe('true');
});

test('a drawer drag that makes room holds a yielded Canvas collapsed until release', async ({ page }) => {
  // A wide drawer leaves the drag room to narrow it well past the 40px below.
  await page.addInitScript((key) => localStorage.setItem(key, '440'), THREAD_DRAWER_WIDTH_KEY);
  const startWidth = 1280;
  await page.setViewportSize({ width: startWidth, height: 800 });
  await navigateToApp(page);
  await setFlag(page, 'data-thread-drawer-open', true, () => toggleDrawer(page));
  // The rail draws narrower in a narrower window, so it would skew the resize.
  await setFlag(page, 'data-dashboard-rail', false, () => page.locator('.dashboard-rail-toggle').click());
  await page.waitForTimeout(SETTLE_MS);

  // Resize the window until the split is 40px short of both floors.
  const room = await page.evaluate(() => ({
    splitPx: document.querySelector('.split-layout')!.getBoundingClientRect().width,
    remPx: parseFloat(getComputedStyle(document.documentElement).fontSize),
  }));
  const bothPx = (await minThreadPane(page)) + computeMinContentPaneWidth(room.remPx);
  await page.setViewportSize({ width: Math.round(startWidth + bothPx - 40 - room.splitPx), height: 800 });
  await page.waitForTimeout(SETTLE_MS);

  const collapsed = () => page.evaluate(() => document.documentElement.hasAttribute('data-content-collapsed'));
  expect(await collapsed(), 'the Canvas yields before the drag').toBe(true);
  const divider = await page.locator('.drawer-divider').boundingBox();
  expect(divider).not.toBeNull();
  const startX = divider!.x + divider!.width / 2;
  await page.mouse.move(startX, 400);
  await page.mouse.down();
  // Narrowing the drawer gives the split room for both floors.
  for (const dx of [-30, -80, -120, -20, -120]) {
    await page.mouse.move(startX + dx, 400, { steps: 4 });
    expect(await collapsed(), `pointer at ${dx}px`).toBe(true);
  }
  await page.mouse.up();
  await expect.poll(collapsed, { message: 'the Canvas returns at release' }).toBe(false);
});
