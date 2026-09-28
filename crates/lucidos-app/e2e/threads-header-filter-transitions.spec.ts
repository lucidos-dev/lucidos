import { randomUUID } from 'node:crypto';
import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer, isMobileViewport } from './helpers';
import { clearAllThreads, psql, seedThreadRow } from './db-helpers';

// The threads Filter's fades. The Threads/Filters swap dips through the pane
// background: the navigation cover rises over the leaving view, holds while the
// views swap, and clears off the arriving one. The pane title switches word at
// once, with no fade. The Filter button's glyph and badge are button feedback,
// on --duration-fast.
// Unsuffixed, so it runs on desktop Chromium, phone Chromium and iPhone WebKit.
//
// Most tests slow every animation tenfold through the Animation speed slider,
// so each fade spans about 90 to 120 frames and a sampled frame lands mid-fade.

/** The slider's slowest position: 0.1x, so --duration-slow lasts 3s. */
const SLOWEST = '-10';
/** A slowed dip, plus its fuse and room for the frame it lands on. */
const SLOW_FADE_MS = 3_600;

interface Frame {
  /** The navigation cover's opacity, or -1 once it has unmounted. */
  veil: number;
  coverVisible: boolean;
  panel: boolean;
  panelVisible: boolean;
  listVisible: boolean;
  glyph: Record<string, number>;
  titleText: string;
  titleOpacity: number;
  badge: number;
  badgeVisible: boolean;
}

/** The layout's own copies: the page mounts both header rows, one hidden. */
function selectors(page: Page) {
  return isMobileViewport(page)
    ? { header: '.mobile-threads-header', title: '.mobile-header-title', drawer: '.mobile-threads-pane .thread-drawer' }
    : { header: '.desktop-header .threads-header', title: '.threads-header-title', drawer: '.thread-drawer' };
}

/** Installs `window.__fx`, which reads one frame and samples many. It runs in
 *  the page, so a click and the frames after it share one clock. */
async function installProbe(page: Page): Promise<void> {
  await page.addInitScript((sel) => {
    const q = (s: string) => document.querySelector(s) as HTMLElement | null;
    const opacity = (el: Element | null | undefined) => (el ? parseFloat(getComputedStyle(el).opacity) : -1);
    const layers = (root: Element | null | undefined) => Object.fromEntries(
      Array.from(root?.querySelectorAll<HTMLElement>('.crossfade-layer') ?? [])
        .map(l => [l.dataset.layer ?? '', opacity(l)]),
    );
    const fx = {
      cover: () => q(`${sel.drawer} > .thread-filter-cover`),
      veil: () => q(`${sel.drawer} > .nav-cover`),
      button: () => q(`${sel.header} button[aria-label="Filter threads"]`),
      title: () => q(`${sel.header} ${sel.title}`),
      read() {
        const cover = fx.cover();
        const button = fx.button();
        const badge = button?.querySelector('.badge');
        const panel = cover?.querySelector('.thread-filter-panel');
        const list = q(`${sel.drawer} .thread-drawer-list`);
        const title = fx.title();
        return {
          veil: opacity(fx.veil()),
          coverVisible: !!cover && getComputedStyle(cover).visibility === 'visible',
          panel: !!panel,
          panelVisible: !!panel && getComputedStyle(panel).visibility === 'visible',
          listVisible: !!list && getComputedStyle(list).visibility === 'visible',
          glyph: layers(button?.querySelector('.filter-glyph')),
          titleText: title?.textContent ?? '',
          titleOpacity: opacity(title),
          badge: opacity(badge),
          badgeVisible: !!badge && getComputedStyle(badge).visibility === 'visible',
        };
      },
      async sample(ms: number) {
        const frames = [];
        const end = performance.now() + ms;
        while (performance.now() < end) {
          await new Promise(r => requestAnimationFrame(r));
          frames.push(fx.read());
        }
        return frames;
      },
      /** Waits frame by frame until `pred` holds, at most `ms`. */
      async until(pred: (f: ReturnType<typeof fx.read>) => boolean, ms = 5_000) {
        const end = performance.now() + ms;
        while (performance.now() < end) {
          await new Promise(r => requestAnimationFrame(r));
          if (pred(fx.read())) return true;
        }
        return false;
      },
      toggle: () => fx.button()!.click(),
      /** A row in the panel, by its visible label. */
      clickPanelRow(label: string) {
        const rows = Array.from(fx.cover()!.querySelectorAll<HTMLElement>('.drawer-view-option, .thread-filter-option'));
        const row = rows.find(r => !r.classList.contains('thread-filter-option-child')
          && (r.textContent ?? '').trim().startsWith(label));
        if (!row) throw new Error(`no panel row "${label}"`);
        (row.querySelector('input') ?? row).click();
      },
    };
    (window as unknown as { __fx: typeof fx }).__fx = fx;
  }, selectors(page));
}

async function open(page: Page, { slow = true } = {}): Promise<void> {
  await installProbe(page);
  if (slow) {
    await page.addInitScript((pos) => localStorage.setItem('lucidos-animation-speed-slider', pos), SLOWEST);
  }
  await navigateToApp(page);
  await openThreadDrawer(page);
  // Keep the pointer off the header, so no hover veil reads as a state.
  await page.mouse.move(1, 700);
}

/** Frames strictly inside a fade, not at either end. */
const mid = (v: number) => v > 0.05 && v < 0.95;

test.describe('Threads Filter fades', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test.afterEach(() => clearAllThreads());

  test('each swap dips through the background, both ways', async ({ page }) => {
    await open(page);
    const { opening, closing } = await page.evaluate(async (ms) => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.toggle();
      const opening = await fx.sample(ms);
      fx.toggle();
      const closing = await fx.sample(ms);
      return { opening, closing };
    }, SLOW_FADE_MS) as { opening: Frame[]; closing: Frame[] };

    const dips = (frames: Frame[], phase: string, leaving: 'threads' | 'filters') => {
      const arriving = leaving === 'threads' ? 'filters' : 'threads';
      const shows = (f: Frame) => (f.listVisible ? 'threads' : f.panelVisible ? 'filters' : 'none');
      for (const [i, f] of frames.entries()) {
        expect(f.listVisible && f.panelVisible, `${phase} frame ${i}: rows and options drawn together`).toBe(false);
      }
      // It starts over the leaving view, from transparent.
      expect(shows(frames[0]), `${phase}: the leaving view went before the dip`).toBe(leaving);
      expect(frames[0].veil, `${phase}: the cover started opaque`).toBeLessThan(0.2);
      // One swap, and it lands under an opaque cover.
      const swap = frames.findIndex(f => shows(f) === arriving);
      expect(swap, `${phase}: the arriving view never showed`).toBeGreaterThan(0);
      expect(frames.slice(swap).every(f => shows(f) === arriving), `${phase}: the views swapped back`).toBe(true);
      expect(frames[swap - 1].veil, `${phase}: the leaving view went under a thin cover`).toBeGreaterThan(0.9);
      expect(frames[swap].veil, `${phase}: the arriving view showed under a thin cover`).toBeGreaterThan(0.9);
      // The cover rises steadily, then falls steadily, then unmounts.
      const rising = frames.slice(0, swap);
      const falling = frames.slice(swap).filter(f => f.veil >= 0);
      expect(rising.filter(f => mid(f.veil)).length, `${phase}: the leaving view never faded out`).toBeGreaterThan(5);
      expect(falling.filter(f => mid(f.veil)).length, `${phase}: the arriving view never faded in`).toBeGreaterThan(5);
      for (let i = 1; i < rising.length; i++) {
        expect(rising[i].veil, `${phase} frame ${i}: the cover fell before the swap`).toBeGreaterThanOrEqual(rising[i - 1].veil - 0.01);
      }
      for (let i = 1; i < falling.length; i++) {
        expect(falling[i].veil, `${phase} frame ${swap + i}: the cover rose after the swap`).toBeLessThanOrEqual(falling[i - 1].veil + 0.01);
      }
      expect(frames.at(-1)!.veil, `${phase}: the cover outlived its fuse`).toBe(-1);
      // The title does not fade: it says the arriving view from the first frame.
      const word = arriving === 'threads' ? 'Threads' : 'Filters';
      for (const [i, f] of frames.entries()) {
        expect(f.titleText, `${phase} frame ${i}: the title lagged the tap`).toBe(word);
        expect(f.titleOpacity, `${phase} frame ${i}: the title faded`).toBe(1);
      }
    };

    dips(opening, 'open', 'threads');
    dips(closing, 'close', 'filters');
    // Kept mounted, so the next open costs no render.
    expect(closing.at(-1)!.panel, 'the panel was unmounted').toBe(true);
  });

  test('a swap during a dip restarts the dip, with no remount', async ({ page }) => {
    await open(page);
    const result = await page.evaluate(async () => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.toggle();
      const panel = fx.cover().querySelector('.thread-filter-panel');
      const first = fx.veil();
      await fx.until((f: Frame) => f.veil > 0.4 && f.listVisible);
      fx.toggle();
      await new Promise(r => requestAnimationFrame(r));
      const after = fx.read();
      const restarted = fx.veil() !== first;
      const settled = await fx.until((f: Frame) => f.veil === -1);
      return { after, restarted, settled, end: fx.read(), samePanel: fx.cover().querySelector('.thread-filter-panel') === panel };
    }) as { after: Frame; restarted: boolean; settled: boolean; end: Frame; samePanel: boolean };

    expect(result.restarted, 'the new swap reused the old cover').toBe(true);
    expect(result.after.veil, 'the new dip did not start from its beginning').toBeLessThan(0.2);
    expect(result.after.panelVisible, 'the reversed panel showed').toBe(false);
    expect(result.settled).toBe(true);
    expect(result.end.listVisible).toBe(true);
    expect(result.end.titleText).toBe('Threads');
    expect(result.samePanel, 'the swap remounted the panel').toBe(true);
  });

  test('a leaving panel takes no pointer and no focus', async ({ page }) => {
    await open(page);
    const result = await page.evaluate(async () => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.toggle();
      await fx.until((f: Frame) => f.veil === -1);
      fx.toggle();
      // Mid-dip, while the leaving panel is still on screen.
      await fx.until((f: Frame) => f.veil > 0.3);
      const cover = fx.cover() as HTMLElement;
      const box = cover.getBoundingClientRect();
      const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2);
      return {
        stillFading: fx.read().panelVisible,
        inert: cover.inert,
        hitInside: !!hit && cover.contains(hit),
        hitVeil: !!hit && hit === fx.veil(),
        focusInside: cover.contains(document.activeElement),
      };
    });
    expect(result.stillFading, 'the check missed the fade').toBe(true);
    expect(result.inert).toBe(true);
    expect(result.hitInside, 'a pointer still lands on the leaving panel').toBe(false);
    expect(result.hitVeil, 'the veil takes the pointer').toBe(false);
    expect(result.focusInside).toBe(false);
  });

  test('the leaving list takes no tap, while the list still pans', async ({ page }) => {
    await open(page);
    const result = await page.evaluate(async () => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.toggle();
      // Mid-dip, while the leaving list is still on screen.
      await fx.until((f: Frame) => f.veil > 0.3 && f.listVisible);
      const list = fx.cover().parentElement.querySelector('.thread-drawer-list') as HTMLElement;
      const box = list.getBoundingClientRect();
      const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2);
      return { onScreen: fx.read().listVisible, hitRow: !!hit && hit !== list && list.contains(hit) };
    });
    expect(result.onScreen, 'the check missed the dip').toBe(true);
    expect(result.hitRow, 'a pointer still lands on a leaving row').toBe(false);
  });

  test('the glyph crossfades on a type filter, a status pick, and back', async ({ page }) => {
    await open(page);
    const run = (fn: string) => page.evaluate(async ({ fn, ms }) => {
      const fx = (window as unknown as { __fx: any }).__fx;
      if (fn === 'toggle') fx.toggle(); else fx.clickPanelRow(fn);
      return fx.sample(ms) as Promise<Frame[]>;
    }, { fn, ms: SLOW_FADE_MS });
    const crossfaded = (frames: Frame[], from: string, to: string) =>
      frames.some(f => mid(f.glyph[from]) && mid(f.glyph[to]));
    const settledOn = (frames: Frame[], key: string) => {
      const g = frames.at(-1)!.glyph;
      for (const [k, v] of Object.entries(g)) expect(v, `glyph ${k}`).toBe(k === key ? 1 : 0);
    };

    await run('toggle');
    // Untick Lucidos: thread types now narrow All statuses, so the funnel fills.
    let frames = await run('Lucidos');
    expect(crossfaded(frames, 'all', 'filtered'), 'outline to solid did not fade').toBe(true);
    settledOn(frames, 'filtered');
    frames = await run('Lucidos');
    expect(crossfaded(frames, 'filtered', 'all'), 'solid to outline did not fade').toBe(true);
    settledOn(frames, 'all');

    // A status pick closes the panel: glyph, title and view all move together.
    frames = await run('Review');
    expect(crossfaded(frames, 'all', 'review'), 'funnel to status did not fade').toBe(true);
    expect(frames.every(f => f.titleText === 'Threads' && f.titleOpacity === 1), 'the title did not switch at once').toBe(true);
    settledOn(frames, 'review');

    await run('toggle');
    frames = await run('Running');
    expect(crossfaded(frames, 'review', 'running'), 'status to status did not fade').toBe(true);
    await run('toggle');
    frames = await run('All statuses');
    expect(crossfaded(frames, 'running', 'all'), 'status back to the funnel did not fade').toBe(true);
    settledOn(frames, 'all');
  });

  test('the funnel rim is painted once: the wrapper carries the translucency', async ({ page }) => {
    await open(page, { slow: false });
    const { header } = selectors(page);
    const button = page.locator(`${header} button[aria-label="Filter threads"]`);
    const read = () => button.evaluate((btn) => ({
      wrapper: getComputedStyle(btn.querySelector('.filter-glyph')!).opacity,
      shapes: Array.from(btn.querySelectorAll('.filter-glyph svg')).map(s => getComputedStyle(s).color),
    }));

    const resting = await read();
    expect(resting.wrapper).toBe('0.72');
    expect(resting.shapes.length).toBeGreaterThan(1);
    for (const c of resting.shapes) expect(c, 'a shape inside the stack is translucent').toBe('rgb(255, 255, 255)');

    // Pressed, it paints opaque like any pressed header icon.
    await button.click();
    await page.mouse.move(1, 700);
    await expect.poll(async () => (await read()).wrapper).toBe('1');
  });

  test('the badge fades out as the panel opens and back as it closes', async ({ page }) => {
    const now = new Date().toISOString();
    psql(seedThreadRow({ id: randomUUID(), title: 'filter-badge-fade', now, status: 'failed', archiveState: 'inbox' }));
    await open(page);
    const { header } = selectors(page);
    const badge = page.locator(`${header} button[aria-label="Filter threads"] .badge`);
    await expect(badge).toHaveText('1');
    await expect(badge).toBeVisible();

    const { opening, closing } = await page.evaluate(async (ms) => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.toggle();
      const opening = await fx.sample(ms);
      fx.toggle();
      const closing = await fx.sample(ms);
      return { opening, closing };
    }, SLOW_FADE_MS) as { opening: Frame[]; closing: Frame[] };

    expect(opening.filter(f => mid(f.badge)).length, 'the badge never faded out').toBeGreaterThan(5);
    expect(opening.at(-1)!.badge).toBe(0);
    expect(opening.at(-1)!.badgeVisible, 'a hidden badge still takes the pointer').toBe(false);
    expect(closing.filter(f => mid(f.badge)).length, 'the badge never faded back').toBeGreaterThan(5);
    expect(closing.at(-1)!.badge).toBe(1);
    expect(closing.at(-1)!.badgeVisible).toBe(true);
    await expect(badge).toHaveText('1');

    // It still sits in the button's top-right corner, off the funnel's body.
    const g = await page.locator(`${header} button[aria-label="Filter threads"]`).evaluate((btn) => {
      const b = btn.querySelector('.badge')!.getBoundingClientRect();
      const s = btn.querySelector('.filter-glyph')!.getBoundingClientRect();
      return { badgeCx: b.left + b.width / 2, badgeCy: b.top + b.height / 2, glyphCx: s.left + s.width / 2, glyphCy: s.top + s.height / 2 };
    });
    expect(g.badgeCx).toBeGreaterThan(g.glyphCx);
    expect(g.badgeCy).toBeLessThan(g.glyphCy);
  });

  test('the cover and the button fades start in the same frame, and the title never animates', async ({ page }) => {
    await open(page);
    const starts = await page.evaluate(async () => {
      const fx = (window as unknown as { __fx: any }).__fx;
      const header = fx.button().closest('.threads-header, .mobile-threads-header') as HTMLElement;
      const drawer = fx.cover().parentElement as HTMLElement;
      const scope = [header, drawer];
      const collect = async () => {
        await new Promise(r => requestAnimationFrame(r));
        const anims = document.getAnimations().filter((a) => {
          const target = (a.effect as KeyframeEffect).target as Node;
          if (!scope.some(s => s.contains(target))) return false;
          if (a instanceof CSSAnimation) return a.animationName === 'nav-cover-dip';
          return a instanceof CSSTransition && a.transitionProperty === 'opacity';
        });
        await Promise.all(anims.map(a => a.ready));
        return anims.map(a => {
          const target = (a.effect as KeyframeEffect).target as HTMLElement;
          return {
            what: a instanceof CSSAnimation ? a.animationName : fx.title().contains(target) ? 'title' : target.className,
            start: a.startTime as number,
          };
        });
      };
      fx.toggle();
      const opening = await collect();
      await fx.until((f: Frame) => f.veil === -1);
      fx.toggle();
      const closing = await collect();
      return { opening, closing };
    });

    for (const [phase, list] of Object.entries(starts)) {
      const names = list.map(a => a.what).join(' | ');
      expect(names, `${phase}: the view did not dip`).toContain('nav-cover-dip');
      expect(names, `${phase}: the title animated`).not.toContain('title');
      const times = list.map(a => a.start);
      expect(Math.max(...times) - Math.min(...times), `${phase}: fades started apart (${names})`)
        .toBeLessThan(1);
    }
  });

  test('with motion reduced every change is instant', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await open(page, { slow: false });
    const result = await page.evaluate(async () => {
      const fx = (window as unknown as { __fx: any }).__fx;
      const seconds = (el: Element | null, of: 'transitionDuration' | 'transitionDelay' = 'transitionDuration') =>
        Math.max(...getComputedStyle(el!)[of].split(',').map(s => parseFloat(s)));
      fx.toggle();
      await new Promise(r => requestAnimationFrame(r));
      await new Promise(r => requestAnimationFrame(r));
      const opened = fx.read();
      const button = fx.button() as HTMLElement;
      const durations = {
        // The swap is a 0s transition, so its delay is what reduced motion drops.
        swap: seconds(fx.cover(), 'transitionDelay'),
        glyph: seconds(button.querySelector('.filter-glyph .crossfade-layer')),
        wrapper: seconds(button.querySelector('.filter-glyph')),
        badge: seconds(button.querySelector('.badge')),
      };
      const animated = fx.veil() ? getComputedStyle(fx.veil()).animationName : 'none';
      fx.toggle();
      await new Promise(r => requestAnimationFrame(r));
      await new Promise(r => requestAnimationFrame(r));
      return { opened, closed: fx.read(), durations, animated };
    });
    expect(result.opened.veil, 'the veil is drawn under reduced motion').toBeLessThanOrEqual(0);
    expect(result.opened.titleText).toBe('Filters');
    expect(result.opened.panelVisible).toBe(true);
    expect(result.opened.listVisible).toBe(false);
    for (const [what, s] of Object.entries(result.durations)) {
      expect(s, `${what} still animates under reduced motion`).toBeLessThan(0.001);
    }
    expect(result.animated).toBe('none');
    expect(result.closed.coverVisible).toBe(false);
    expect(result.closed.listVisible).toBe(true);
    expect(result.closed.veil).toBeLessThanOrEqual(0);
    expect(result.closed.titleText).toBe('Threads');
  });

  test('a panel restored open by a reload shows at once, with no fade', async ({ page }) => {
    await open(page, { slow: false });
    await page.evaluate(() => (window as unknown as { __fx: any }).__fx.toggle());
    await expect(page.locator(`${selectors(page).drawer} > .thread-filter-cover[data-open]`)).toHaveCount(1);
    await page.reload();
    const boot = await page.waitForFunction(() => {
      const fx = (window as unknown as { __fx: any }).__fx;
      const cover = fx?.cover();
      if (!cover?.hasAttribute('data-open')) return null;
      return { veil: !!fx.veil(), title: fx.title()?.getAnimations({ subtree: true }).length ?? -1, text: fx.read().titleText };
    }, undefined, { timeout: 15_000, polling: 'raf' });
    expect(await boot.jsonValue()).toEqual({ veil: false, title: 0, text: 'Filters' });
    await page.evaluate(() => (window as unknown as { __fx: any }).__fx.toggle());
  });
});
