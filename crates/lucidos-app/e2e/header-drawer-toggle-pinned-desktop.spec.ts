import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer, stampOverlayBuild } from './helpers';

test.use({ reducedMotion: 'no-preference' });


test.use({ viewport: { width: 1280, height: 800 } });

const OPEN_DRAWER_WIDTH = 420;

const SAMPLE_CAP_MS = 5000;

const TAIL_MS = 700;

const SETTLE_MS = 600;

interface Box {
  left: number;
  right: number;
  width: number;
}

interface RowControl {
  name: string;
  painted: Box | null;
}

interface Sample {
  frame: number;
  toggleLeft: number;
  toggle: Box | null;
  toggleGlyph: Box | null;
  toggleOpacity: number;
  toggleCount: number;
  rowControls: RowControl[];
  rowClip: string;
  hamburgerGlyph: Box | null;
}

async function startSampling(page: Page): Promise<void> {
  await page.evaluate((capMs) => {
    const header = document.querySelector('.desktop-header')!;
    const w = window as unknown as { __sampleUntil: number; __toggleSamples: Promise<unknown[]> };
    const effectiveOpacity = (el: Element | null): number => {
      let o = 1;
      for (let n: Element | null = el; n && n !== document.documentElement; n = n.parentElement) {
        o *= parseFloat(getComputedStyle(n).opacity || '1');
        if (n.classList.contains('app-header')) break;
      }
      return o;
    };
    const cut = (r: DOMRect, left: number, right: number) => {
      const l = Math.max(r.left, left);
      const rt = Math.min(r.right, right);
      return rt > l ? { left: l, right: rt, width: rt - l } : null;
    };
    const insets = (clip: string): number[] => {
      const m = /^inset\(([^)]*)\)/.exec(clip);
      if (!m) return [0, 0, 0, 0];
      const [t, r = t, b = t, l = r] = m[1].trim().split(/\s+/).map(parseFloat);
      return [t, r, b, l];
    };
    const throughClip = (clipper: Element, el: Element | null | undefined) => {
      if (!el) return null;
      const box = clipper.getBoundingClientRect();
      const [, r, , l] = insets(getComputedStyle(clipper).clipPath);
      return cut(el.getBoundingClientRect(), box.left + l, box.right - r);
    };
    const rowControls = (row: HTMLElement | null) => {
      if (!row || getComputedStyle(row).visibility === 'hidden') return [];
      const searching = row.classList.contains('search-active');
      return Array.from(row.querySelectorAll<HTMLElement>(
        searching ? '.thread-search-bar' : '.threads-header-btn',
      )).map(el => ({
        name: el.getAttribute('aria-label') ?? el.className,
        painted: throughClip(row, el),
      }));
    };
    const box = (r: DOMRect | undefined) => (r ? { left: r.left, right: r.right, width: r.width } : null);
    w.__sampleUntil = performance.now() + capMs;
    const samples: unknown[] = [];
    w.__toggleSamples = new Promise<unknown[]>((done) => {
      let frame = 0;
      const tick = () => {
        const slot = header.querySelector('.thread-toggle-slot');
        const btn = slot?.querySelector('.thread-toggle');
        const row = header.querySelector<HTMLElement>('.threads-header');
        const painted = Array.from(header.querySelectorAll('.thread-toggle')).filter((el) => {
          return el.getBoundingClientRect().width > 0 && effectiveOpacity(el) > 0.01;
        });
        const btnLeft = btn?.getBoundingClientRect().left ?? Number.NaN;
        samples.push({
          frame: frame++,
          toggleLeft: btnLeft,
          toggle: slot && btn
            ? throughClip(slot, btn) ?? { left: btnLeft, right: btnLeft, width: 0 }
            : null,
          toggleGlyph: slot ? throughClip(slot, btn?.querySelector('svg')) : null,
          toggleOpacity: effectiveOpacity(slot ?? null),
          toggleCount: painted.length,
          rowControls: rowControls(row),
          rowClip: row ? getComputedStyle(row).clipPath : 'no row',
          hamburgerGlyph: box(header.querySelector('.hamburger-panel svg')?.getBoundingClientRect()),
        });
        if (performance.now() < w.__sampleUntil) requestAnimationFrame(tick);
        else done(samples);
      };
      requestAnimationFrame(tick);
    });
  }, SAMPLE_CAP_MS);
}

async function finishSampling(page: Page): Promise<Sample[]> {
  return page.evaluate((tailMs) => {
    const w = window as unknown as { __sampleUntil: number; __toggleSamples: Promise<unknown[]> };
    w.__sampleUntil = Math.min(w.__sampleUntil, performance.now() + tailMs);
    return w.__toggleSamples;
  }, TAIL_MS) as Promise<Sample[]>;
}

async function clickToggle(page: Page): Promise<void> {
  await page.evaluate(() => {
    document.querySelector<HTMLElement>('.desktop-header .thread-toggle-slot .thread-toggle')!.click();
  });
}

async function toggleConversationPane(page: Page): Promise<void> {
  await page.locator('.split-divider').dblclick();
}

function assertConsistentThroughout(samples: Sample[], what: string): void {
  expect(samples.length, `${what}: no frames sampled`).toBeGreaterThan(5);
  for (const s of samples) {
    expect(s.toggle, `${what} frame ${s.frame}: the toggle left the header`).not.toBeNull();
    expect(
      s.toggleOpacity,
      `${what} frame ${s.frame}: the toggle was at opacity ${s.toggleOpacity.toFixed(2)}`,
    ).toBeCloseTo(1, 2);
    expect(
      s.toggleCount,
      `${what} frame ${s.frame}: ${s.toggleCount} toggles painted at once`,
    ).toBeLessThanOrEqual(1);
    for (const c of s.rowControls) {
      if (!c.painted || s.toggle!.width === 0) continue;
      expect(
        c.painted.left,
        `${what} frame ${s.frame}: "${c.name}" painted from ${c.painted.left.toFixed(1)}, `
          + `under a toggle ending at ${s.toggle!.right.toFixed(1)} (row clip ${s.rowClip})`,
      ).toBeGreaterThanOrEqual(s.toggle!.right - 1);
    }
  }
}

function assertGlyphsApart(samples: Sample[], what: string): void {
  for (const s of samples) {
    if (!s.toggleGlyph || !s.hamburgerGlyph) continue;
    const overlap = Math.min(s.toggleGlyph.right, s.hamburgerGlyph.right)
      - Math.max(s.toggleGlyph.left, s.hamburgerGlyph.left);
    expect(
      overlap,
      `${what} frame ${s.frame}: the toggle's glyph (${s.toggleGlyph.left.toFixed(1)} to `
        + `${s.toggleGlyph.right.toFixed(1)}) painted over the hamburger's (from `
        + `${s.hamburgerGlyph.left.toFixed(1)})`,
    ).toBeLessThanOrEqual(1);
  }
}

function assertStaysPut(samples: Sample[], home: number, what: string): void {
  for (const s of samples) {
    expect(
      Math.abs(s.toggleLeft - home),
      `${what} frame ${s.frame}: the toggle moved from ${home.toFixed(1)} to ${s.toggleLeft.toFixed(1)}`,
    ).toBeLessThanOrEqual(1);
  }
}

for (const packaged of [false, true]) {
  const build = packaged ? 'packaged macOS' : 'web';

  test.describe(`Desktop header, ${build}: the drawer toggle stays in the corner`, () => {
    test.beforeEach(async ({ page, context }) => {
      await assertHealthy(page);
      await context.addInitScript((width) => {
        localStorage.setItem('lucidos-split-ratio', '0.4');
        localStorage.setItem('lucidos-thread-drawer-open', 'false');
        localStorage.setItem('lucidos-thread-drawer-width', String(width));
      }, OPEN_DRAWER_WIDTH);
      await navigateToApp(page);
      await openThreadDrawer(page);
      if (packaged) await stampOverlayBuild(page);
      await page.waitForTimeout(SETTLE_MS);
    });

    test('shutting and opening the drawer never moves, dims or doubles it', async ({ page }) => {
      await startSampling(page);
      await clickToggle(page);
      const shut = await finishSampling(page);
      await startSampling(page);
      await clickToggle(page);
      const opened = await finishSampling(page);

      assertConsistentThroughout(shut, 'shut');
      assertConsistentThroughout(opened, 'open');
      const home = shut[0].toggle!;
      assertStaysPut([...shut, ...opened], home.left, 'shut and open');
      for (const s of [...shut, ...opened]) {
        expect(Math.abs(s.toggle!.width - home.width), `frame ${s.frame}: the toggle resized`)
          .toBeLessThanOrEqual(1);
      }
      if (packaged) expect(home.left, 'the toggle sits on the traffic lights').toBeGreaterThanOrEqual(70);
      else expect(home.left, 'the toggle left the corner').toBeLessThan(20);

      const lowestFilter = Math.min(...shut.flatMap(s => s.rowControls
        .filter(c => c.name === 'Filter threads' && c.painted)
        .map(c => c.painted!.left)));
      expect(lowestFilter, 'Filter never travelled toward the corner')
        .toBeLessThan(home.right + OPEN_DRAWER_WIDTH / 2);
    });

    test('the search field starts after the toggle, which stays clickable', async ({ page }) => {
      await page.locator('.desktop-header .threads-header button[aria-label="Search threads"]').click();
      await expect(page.locator('.desktop-header .threads-header.search-active')).toHaveCount(1);
      await page.waitForTimeout(SETTLE_MS);

      const g = await page.evaluate(() => {
        const toggle = document.querySelector('.desktop-header .thread-toggle-slot .thread-toggle')!;
        const field = document.querySelector('.desktop-header .threads-header .thread-search-bar')!;
        const t = toggle.getBoundingClientRect();
        const hit = document.elementFromPoint(t.left + t.width / 2, t.top + t.height / 2);
        return {
          toggleRight: t.right,
          fieldLeft: field.getBoundingClientRect().left,
          fieldWidth: field.getBoundingClientRect().width,
          hitIsToggle: !!hit?.closest('.thread-toggle'),
        };
      });
      expect(g.fieldWidth, 'the search field did not open').toBeGreaterThan(100);
      expect(g.fieldLeft, 'the search field starts under the toggle')
        .toBeGreaterThanOrEqual(g.toggleRight);
      expect(g.hitIsToggle, 'the toggle is covered while searching').toBe(true);
    });

    test('a Conversation-pane collapse wipes it away in place, and back, clear of the hamburger', async ({ page }) => {
      await startSampling(page);
      await toggleConversationPane(page);
      const out = await finishSampling(page);
      await startSampling(page);
      await toggleConversationPane(page);
      const back = await finishSampling(page);

      assertConsistentThroughout(out, 'collapse');
      assertConsistentThroughout(back, 're-expand');
      assertGlyphsApart(out, 'collapse');
      assertGlyphsApart(back, 're-expand');
      assertStaysPut([...out, ...back], out[0].toggleLeft, 'collapse and re-expand');

      expect(out[out.length - 1].toggleGlyph, 'the icon is still painting after the collapse')
        .toBeNull();
      expect(back[back.length - 1].toggleGlyph?.width ?? 0, 'the icon did not come back whole')
        .toBeGreaterThan(8);
    });
  });
}
