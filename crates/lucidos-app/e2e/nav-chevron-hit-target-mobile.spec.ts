import { test, expect, Page } from './fixtures';
import { assertHealthy, navigateToApp, ensureMobileView, watchPreferenceReads } from './helpers';
import { ensureHomeThread } from './db-helpers';

test.use({ viewport: { width: 393, height: 852 } });

const SIMULATED_SAFE_AREA_TOP_PX = 59;

interface Box { left: number; right: number; top: number; bottom: number }

interface ChevronProbe {
  box: Box;
  glyph: Box;
  hit: Box;
  nearMiss: Record<string, string>;
  rowEdge: number;
  reserve: number;
  iconBox: number;
}

async function probeChevron(
  page: Page, headerSel: string, chevronSel: string, outer: 'left' | 'right',
): Promise<ChevronProbe | null> {
  return page.evaluate(({ headerSel, chevronSel, outer }) => {
    const header = document.querySelector(headerSel) as HTMLElement | null;
    const row = header?.querySelector('.mobile-header-row') as HTMLElement | null;
    const btn = header?.querySelector(chevronSel) as HTMLElement | null;
    const svg = btn?.querySelector('svg');
    if (!row || !btn || !svg || btn.getBoundingClientRect().width === 0) return null;

    const rect = (el: Element) => {
      const r = el.getBoundingClientRect();
      return { left: r.left, right: r.right, top: r.top, bottom: r.bottom };
    };
    const box = rect(btn);
    const glyph = rect(svg);
    const cx = (box.left + box.right) / 2;
    const cy = (box.top + box.bottom) / 2;
    const reaches = (x: number, y: number) =>
      document.elementFromPoint(x, y)?.closest('button') === btn;

    const walk = (dx: number, dy: number): number => {
      let d = 0;
      while (d < 80 && reaches(cx + dx * (d + 0.5), cy + dy * (d + 0.5))) d += 0.5;
      return d;
    };
    const hit = {
      left: cx - walk(-1, 0),
      right: cx + walk(1, 0),
      top: cy - walk(0, -1),
      bottom: cy + walk(0, 1),
    };

    const describe = (x: number, y: number): string => {
      const el = document.elementFromPoint(x, y);
      const control = el?.closest('button');
      if (control === btn) return 'chevron';
      if (control) return control.getAttribute('aria-label') ?? control.className;
      return el ? `bare ${el.tagName.toLowerCase()}.${el.className}` : 'nothing';
    };
    const root = parseFloat(getComputedStyle(document.documentElement).fontSize);
    const nearMiss: Record<string, string> = {};
    for (const base of [4, 7]) {
      const d = (base * root) / 16;
      nearMiss[`left+${base}`] = describe(glyph.left - d, cy);
      nearMiss[`right+${base}`] = describe(glyph.right + d, cy);
      nearMiss[`above+${base}`] = describe(cx, glyph.top - d);
      nearMiss[`below+${base}`] = describe(cx, glyph.bottom + d);
    }

    const probe = document.createElement('div');
    probe.style.position = 'absolute';
    probe.style.visibility = 'hidden';
    row.appendChild(probe);
    const widthOf = (value: string) => {
      probe.style.width = value;
      return probe.getBoundingClientRect().width;
    };
    const reserve = widthOf('var(--header-nav-edge-reserve)');
    const iconBox = widthOf('var(--mobile-header-icon-box)');
    probe.remove();

    const rowRect = row.getBoundingClientRect();
    return {
      box, glyph, hit, nearMiss, reserve, iconBox,
      rowEdge: outer === 'left' ? rowRect.left : rowRect.right,
    };
  }, { headerSel, chevronSel, outer });
}

async function hitTargetPx(page: Page): Promise<number> {
  return page.evaluate(() => {
    const probe = document.createElement('div');
    probe.style.position = 'absolute';
    probe.style.width = 'var(--header-nav-hit-target)';
    document.body.appendChild(probe);
    const w = probe.getBoundingClientRect().width;
    probe.remove();
    return w;
  });
}

async function prepare(page: Page): Promise<void> {
  const preferencesSettled = watchPreferenceReads(page);
  await navigateToApp(page);
  await preferencesSettled();
  await page.evaluate((px) => {
    document.documentElement.style.setProperty('--safe-area-floor-top', `${px}px`);
  }, SIMULATED_SAFE_AREA_TOP_PX);
}

interface Neighbour {
  sel: string;
  glyph?: string;
  optional?: boolean;
}

const MARK: Neighbour = { sel: '.mobile-thread-header .brand-mark', glyph: '.brand-mark-glyph' };

type Landed = 'self' | 'chevron' | 'other';
const ALL_SELF: Landed[] = ['self', 'self', 'self', 'self', 'self'];

async function tapsOn(page: Page, n: Neighbour): Promise<Landed[] | null> {
  return page.evaluate(({ sel, glyph, optional }) => {
    const el = document.querySelector(sel) as HTMLElement | null;
    const target = glyph ? el?.querySelector(glyph) : el;
    const r = target?.getBoundingClientRect();
    if (!el || !r || r.width === 0) return null;
    const style = getComputedStyle(el);
    const padding = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
    if (optional && r.width - padding < 1) return null;
    const cx = (r.left + r.right) / 2;
    const cy = (r.top + r.bottom) / 2;
    return [[cx, cy], [r.left + 1, cy], [r.right - 1, cy], [cx, r.top + 1], [cx, r.bottom - 1]]
      .map(([x, y]) => {
        const hit = document.elementFromPoint(x, y);
        if (el.contains(hit)) return 'self';
        return hit?.closest('.nav-chevron') ? 'chevron' : 'other';
      });
  }, n);
}

async function setScale(page: Page, scale: string, rootPx: number): Promise<void> {
  await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', s), scale);
  await expect
    .poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize),
      { timeout: 5_000, message: `the root never settled at ui-scale ${scale}` })
    .toBe(`${rootPx}px`);
}

const PANES = [
  {
    view: 'thread' as const,
    header: '.mobile-thread-header',
    back: 'button[aria-label="Previous thread"]',
    forward: 'button[aria-label="Next thread"]',
  },
  {
    view: 'content' as const,
    header: '.mobile-content-header',
    back: '.content-back-btn',
    forward: '.content-forward-btn',
  },
];

test.describe('The mobile nav chevrons take a near miss', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('each chevron hit box is the full target, placed where the chevron was', async ({ page }) => {
    await prepare(page);

    for (const [scale, rootPx] of [['100%', 16], ['112.5%', 18]] as const) {
      for (const pane of PANES) {
        await ensureMobileView(page, pane.view);
        await setScale(page, scale, rootPx);
        const target = await hitTargetPx(page);
        expect(target, 'the hit target is 44px at the 16px root').toBeGreaterThanOrEqual((44 * rootPx) / 16 - 0.01);

        for (const [name, sel, outer] of [['back', pane.back, 'left'], ['forward', pane.forward, 'right']] as const) {
          await expect
            .poll(() => probeChevron(page, pane.header, sel, outer), { timeout: 10_000, message: `${pane.view} ${name} never laid out` })
            .not.toBeNull();
          const p = (await probeChevron(page, pane.header, sel, outer))!;
          const where = `${pane.view} ${name} at ${scale}`;
          console.log(`${where}: button ${(p.box.right - p.box.left).toFixed(1)}x${(p.box.bottom - p.box.top).toFixed(1)}, `
            + `hit ${(p.hit.right - p.hit.left).toFixed(1)}x${(p.hit.bottom - p.hit.top).toFixed(1)} `
            + `[${p.hit.left.toFixed(1)}..${p.hit.right.toFixed(1)}], near misses ${JSON.stringify(p.nearMiss)}`);

          expect(p.box.top, `${where}: the row is under the status bar`).toBeGreaterThanOrEqual(SIMULATED_SAFE_AREA_TOP_PX);

          expect(p.box.right - p.box.left, `${where}: the button box grew`).toBeCloseTo(p.iconBox, 0);
          expect(p.box.bottom - p.box.top, `${where}: the button box grew`).toBeCloseTo(p.iconBox, 0);
          const offset = outer === 'left' ? p.box.left - p.rowEdge : p.rowEdge - p.box.right;
          expect(offset, `${where}: the chevron left its reserve slot`).toBeCloseTo(p.reserve, 0);
          expect((p.glyph.left + p.glyph.right) / 2, `${where}: the glyph moved off the button centre`)
            .toBeCloseTo((p.box.left + p.box.right) / 2, 0);

          expect(p.hit.bottom - p.hit.top, `${where}: hit height`).toBeGreaterThanOrEqual(target - 1);
          expect(p.hit.right - p.hit.left, `${where}: hit width`).toBeGreaterThanOrEqual(target - 1);

          for (const [key, reached] of Object.entries(p.nearMiss)) {
            const outerSide = key.startsWith(outer);
            if (pane.view === 'content' && name === 'forward' && outerSide && reached !== 'chevron') {
              expect(reached, `${where}: a near miss ${key} fell through to the row`).not.toMatch(/^bare |^nothing$/);
              continue;
            }
            expect(reached, `${where}: a near miss ${key}`).toBe('chevron');
          }
        }
      }

      await ensureMobileView(page, 'thread');
      await setScale(page, scale, rootPx);
      expect(await tapsOn(page, MARK), `the mark glyph at ${scale}`).toEqual(ALL_SELF);
    }
  });
});

test.describe('The chevron reach never takes a neighbour\'s tap', () => {
  test.use({ viewport: { width: 320, height: 640 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    ensureHomeThread();
  });

  test('the mark, the title and the edge controls keep their taps at 200%', async ({ page }) => {
    await prepare(page);
    for (const pane of PANES) {
      await ensureMobileView(page, pane.view);
      await setScale(page, '200%', 32);
      const neighbours: Neighbour[] = pane.view === 'thread'
        ? [
          MARK,
          { sel: '.mobile-thread-header .home-thread-btn' },
          { sel: '.mobile-thread-header .thread-toggle' },
          { sel: '.mobile-thread-header .hamburger-panel' },
        ]
        : [{ sel: '.mobile-content-header .hamburger-panel' }, { sel: '.mobile-content-header .mobile-content-title', optional: true }];
      for (const n of neighbours) {
        const taps = await tapsOn(page, n);
        if (taps === null && n.optional) continue;
        const where = `${pane.view} pane: ${n.sel} at 200%`;
        expect(taps, `${where} is not rendered`).not.toBeNull();
        expect(taps![0], `${where}: its centre`).toBe('self');
        expect(taps, `${where}: a tap landed on a chevron`).not.toContain('chevron');
      }
    }
  });
});
