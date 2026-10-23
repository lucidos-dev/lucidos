/**
 * Every mobile header icon takes a tap that lands above or below its 1.75rem
 * button, upright and in landscape. The button box is what the row's reserves
 * place, so it cannot grow. A transparent `::before` reaches past it instead
 * (styles/mobile.css, `.mobile-header-row .icon-btn.header-icon`). The nav
 * chevrons have their own reach, pinned by nav-chevron-hit-target.
 *
 * Pinned for every visible icon on all three panes, at 100% and 112.5%:
 * - its hit box, found by walking `elementFromPoint` out from its centre,
 *   is the full 2.75rem target tall and its own button wide;
 * - a tap on its own button, centre or edge, still lands on it;
 * - a tap on a chevron's own button lands on the chevron.
 */
import { test, expect, Page } from './fixtures';
import { assertHealthy, navigateToApp, ensureMobileView } from './helpers';

const PORTRAIT = { width: 393, height: 852 };
const LANDSCAPE = { width: 852, height: 393 };
// The emulator zeroes every safe-area inset. A phone upright has the status bar
// above the header, and in landscape the Dynamic Island at one side.
const STATUS_BAR_PX = 59;
const ISLAND_PX = 59;

interface Probe {
  label: string;
  box: { left: number; right: number; top: number; bottom: number };
  /** How far a tap still reaches the button, out from its centre. */
  reach: { left: number; right: number; up: number; down: number };
  /** Where taps on the button's own centre and edges land. */
  own: boolean[];
}

async function probeHeaderIcons(page: Page): Promise<Probe[]> {
  return page.evaluate(() => {
    const view = document.querySelector('.app-header')?.getAttribute('data-mobile-view');
    const section = document.querySelector(`.app-header .mobile-${view}-header`);
    if (!section) return [];
    // The closed search bar stays laid out, but clipped and untappable.
    const buttons = Array.from(section.querySelectorAll<HTMLElement>(
      '.mobile-header-row .icon-btn.header-icon:not(.nav-chevron)',
    )).filter((b) => {
      const cs = getComputedStyle(b);
      return b.getBoundingClientRect().width > 0 && cs.visibility !== 'hidden' && cs.pointerEvents !== 'none';
    });

    return buttons.map((btn) => {
      const r = btn.getBoundingClientRect();
      const box = { left: r.left, right: r.right, top: r.top, bottom: r.bottom };
      const cx = (r.left + r.right) / 2;
      const cy = (r.top + r.bottom) / 2;
      const reaches = (x: number, y: number) =>
        y >= 0 && document.elementFromPoint(x, y)?.closest('button') === btn;
      const walk = (dx: number, dy: number): number => {
        let d = 0;
        while (d < 80 && reaches(cx + dx * (d + 0.5), cy + dy * (d + 0.5))) d += 0.5;
        return d;
      };
      const own = [[cx, cy], [r.left + 1, cy], [r.right - 1, cy], [cx, r.top + 1], [cx, r.bottom - 1]]
        .map(([x, y]) => btn.contains(document.elementFromPoint(x, y)));
      return {
        label: btn.getAttribute('aria-label') ?? btn.className,
        box,
        reach: { left: walk(-1, 0), right: walk(1, 0), up: walk(0, -1), down: walk(0, 1) },
        own,
      };
    });
  });
}

/** For each visible nav chevron on the shown row, whether taps on its own
 *  centre and edges land on it. A neighbour's reach must never cover it. */
async function chevronOwnTaps(page: Page): Promise<Record<string, boolean[]>> {
  return page.evaluate(() => {
    const view = document.querySelector('.app-header')?.getAttribute('data-mobile-view');
    const section = document.querySelector(`.app-header .mobile-${view}-header`);
    const out: Record<string, boolean[]> = {};
    for (const btn of Array.from(section?.querySelectorAll<HTMLElement>('.mobile-header-row .nav-chevron') ?? [])) {
      const r = btn.getBoundingClientRect();
      if (r.width === 0) continue;
      const cx = (r.left + r.right) / 2;
      const cy = (r.top + r.bottom) / 2;
      out[btn.getAttribute('aria-label') ?? btn.className] =
        [[cx, cy], [r.left + 1, cy], [r.right - 1, cy], [cx, r.top + 1], [cx, r.bottom - 1]]
          .map(([x, y]) => btn.contains(document.elementFromPoint(x, y)));
    }
    return out;
  });
}

/** Resolve a length to px through a probe, at the current root. */
async function px(page: Page, value: string): Promise<number> {
  return page.evaluate((v) => {
    const probe = document.createElement('div');
    probe.style.position = 'absolute';
    probe.style.width = v;
    document.body.appendChild(probe);
    const w = probe.getBoundingClientRect().width;
    probe.remove();
    return w;
  }, value);
}

async function setScale(page: Page, scale: string, rootPx: number): Promise<void> {
  await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', s), scale);
  await expect
    .poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize),
      { timeout: 5_000, message: `the root never settled at ui-scale ${scale}` })
    .toBe(`${rootPx}px`);
}

async function rotate(page: Page, size: { width: number; height: number }): Promise<void> {
  await page.setViewportSize(size);
  await page.waitForFunction(({ width }) => window.innerWidth === width, size);
}

/** Stamp the insets a real phone reports in this orientation. */
async function simulateInsets(page: Page, orientation: 'portrait' | 'landscape'): Promise<void> {
  await page.evaluate(({ orientation, top, side }) => {
    const s = document.documentElement.style;
    s.setProperty('--safe-area-floor-top', orientation === 'portrait' ? `${top}px` : '0px');
    s.setProperty('--safe-area-floor-left', orientation === 'landscape' ? `${side}px` : '0px');
  }, { orientation, top: STATUS_BAR_PX, side: ISLAND_PX });
}

test.describe('The mobile header icons take a near miss', () => {
  test.use({ viewport: PORTRAIT });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('every icon hit box is the full target tall, upright and in landscape', async ({ page }) => {
    await navigateToApp(page);
    // The app's own preference load writes the ui-scale. Wait for it, so the
    // scale set here is the last write rather than one that gets undone.
    await page.waitForFunction(
      () => localStorage.getItem('lucidos-ui-scale') !== null, undefined, { timeout: 10_000 },
    );

    for (const view of ['threads', 'thread', 'content'] as const) {
      for (const orientation of ['portrait', 'landscape'] as const) {
        // `ensureMobileView` reads a narrow viewport as the phone layout, so
        // the pane is picked upright and kept through the rotation.
        await rotate(page, PORTRAIT);
        await ensureMobileView(page, view);
        await rotate(page, orientation === 'portrait' ? PORTRAIT : LANDSCAPE);
        await simulateInsets(page, orientation);

        for (const [scale, rootPx] of [['100%', 16], ['112.5%', 18]] as const) {
          await setScale(page, scale, rootPx);
          const target = await px(page, 'var(--header-nav-hit-target)');
          await expect
            .poll(async () => (await probeHeaderIcons(page)).length, { message: `${view} ${orientation}: no header icons laid out` })
            .toBeGreaterThan(0);
          const probes = await probeHeaderIcons(page);

          for (const p of probes) {
            const where = `${view} ${orientation} at ${scale}: ${p.label}`;
            const cy = (p.box.top + p.box.bottom) / 2;
            console.log(`${where}: button ${(p.box.right - p.box.left).toFixed(1)}x${(p.box.bottom - p.box.top).toFixed(1)} `
              + `at y=${p.box.top.toFixed(1)}, reach ${JSON.stringify(p.reach)}`);

            expect(p.own, `${where}: a tap on its own button went elsewhere`).toEqual([true, true, true, true, true]);
            // Half the target each way, minus the walk's half-px step. In
            // landscape the row meets the screen top, which bounds the reach up.
            expect(p.reach.down, `${where}: reach below`).toBeGreaterThanOrEqual(target / 2 - 0.5);
            expect(p.reach.up, `${where}: reach above`).toBeGreaterThanOrEqual(Math.min(target / 2, cy) - 0.5);
            const width = p.reach.left + p.reach.right;
            expect(width, `${where}: hit width`).toBeGreaterThanOrEqual(p.box.right - p.box.left - 1);
          }

          for (const [label, taps] of Object.entries(await chevronOwnTaps(page))) {
            expect(taps, `${view} ${orientation} at ${scale}: a tap on the ${label} chevron went elsewhere`)
              .toEqual([true, true, true, true, true]);
          }
        }
      }
    }
  });
});
