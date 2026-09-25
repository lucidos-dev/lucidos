import { randomUUID } from 'node:crypto';
import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer, isMobileViewport } from './helpers';
import { clearAllThreads, psql, seedThreadRow } from './db-helpers';

// The threads Filter's fades: the glyph, its badge, the pane title and the
// filter panel all change on one timing (--duration-fast). Unsuffixed, so it
// runs on desktop Chromium, phone Chromium and iPhone WebKit.
//
// Most tests slow every animation tenfold through the Animation speed slider,
// so each fade spans about 90 frames and a sampled frame lands mid-fade. The
// slider scales the CSS tokens and the TS exit timer alike.

/** The slider's slowest position: 0.1x, so --duration-fast lasts 1.5s. */
const SLOWEST = '-10';
/** A slowed fade, plus room for the frame it lands on. */
const SLOW_FADE_MS = 1_900;

interface Frame {
  fade: number;
  coverVisible: boolean;
  coverBg: string;
  panel: boolean;
  glyph: Record<string, number>;
  title: Record<string, number>;
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
      cover: () => q(`${sel.drawer} .thread-filter-cover`),
      button: () => q(`${sel.header} button[aria-label="Filter threads"]`),
      title: () => q(`${sel.header} ${sel.title}`),
      read() {
        const cover = fx.cover();
        const button = fx.button();
        const badge = button?.querySelector('.badge');
        const style = cover ? getComputedStyle(cover) : null;
        return {
          fade: opacity(cover?.querySelector('.thread-filter-fade')),
          coverVisible: style?.visibility === 'visible',
          coverBg: style?.backgroundColor ?? '',
          panel: !!cover?.querySelector('.thread-filter-panel'),
          glyph: layers(button?.querySelector('.filter-glyph')),
          title: layers(fx.title()),
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

  test('the panel fades through: the cover lands at once, and goes only once the options have faded', async ({ page }) => {
    await open(page);
    const { opening, closing } = await page.evaluate(async (ms) => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.toggle();
      const opening = await fx.sample(ms);
      fx.toggle();
      const closing = await fx.sample(ms);
      return { opening, closing };
    }, SLOW_FADE_MS) as { opening: Frame[]; closing: Frame[] };

    // Open: the opaque cover is up from the first frame, then the options fade in.
    for (const [i, f] of opening.entries()) {
      expect(f.coverVisible, `open frame ${i}: the cover is not up`).toBe(true);
      expect(f.coverBg, `open frame ${i}: the cover is not opaque`).toMatch(/^rgb\(/);
      expect(f.panel, `open frame ${i}: no options to fade in`).toBe(true);
    }
    expect(opening.filter(f => mid(f.fade)).length, 'the options never faded in').toBeGreaterThan(5);
    for (let i = 1; i < opening.length; i++) {
      expect(opening[i].fade, `open frame ${i}: the fade went backwards`).toBeGreaterThanOrEqual(opening[i - 1].fade - 0.01);
    }
    expect(opening.at(-1)!.fade).toBe(1);

    // Close: the options fade out while the cover still hides the list, and the
    // cover goes only once they are gone. The list never shows under options.
    expect(closing.filter(f => mid(f.fade)).length, 'the options never faded out').toBeGreaterThan(5);
    for (const [i, f] of closing.entries()) {
      if (f.fade > 0.02) expect(f.coverVisible, `close frame ${i}: the list shows under options at ${f.fade}`).toBe(true);
    }
    for (let i = 1; i < closing.length; i++) {
      expect(closing[i].fade, `close frame ${i}: the fade went backwards`).toBeLessThanOrEqual(closing[i - 1].fade + 0.01);
    }
    const last = closing.at(-1)!;
    expect(last.coverVisible, 'the cover outlived the fade').toBe(false);
    expect(last.panel, 'the options outlived the fade').toBe(false);
  });

  test('reopening during the fade out reverses it, with no flash and no remount', async ({ page }) => {
    await open(page);
    const result = await page.evaluate(async (ms) => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.toggle();
      await fx.until((f: Frame) => f.fade === 1);
      const panel = fx.cover().querySelector('.thread-filter-panel');
      fx.toggle();
      await fx.until((f: Frame) => f.fade < 0.6);
      const at = fx.read().fade;
      fx.toggle();
      const frames = await fx.sample(ms);
      return { at, frames, samePanel: fx.cover().querySelector('.thread-filter-panel') === panel };
    }, SLOW_FADE_MS) as { at: number; frames: Frame[]; samePanel: boolean };

    expect(result.at, 'the reopen missed the fade out').toBeGreaterThan(0.2);
    for (const [i, f] of result.frames.entries()) {
      expect(f.coverVisible, `frame ${i}: the cover flashed off`).toBe(true);
      expect(f.fade, `frame ${i}: the options flashed out`).toBeGreaterThan(result.at - 0.1);
    }
    expect(result.frames.at(-1)!.fade).toBe(1);
    expect(result.samePanel, 'the reopen remounted the panel').toBe(true);
  });

  test('closing during the fade in turns it around from where it is', async ({ page }) => {
    await open(page);
    const result = await page.evaluate(async (ms) => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.toggle();
      await fx.until((f: Frame) => f.fade > 0.3);
      const at = fx.read().fade;
      fx.toggle();
      return { at, frames: await fx.sample(ms) };
    }, SLOW_FADE_MS) as { at: number; frames: Frame[] };

    expect(result.at).toBeLessThan(0.8);
    for (const [i, f] of result.frames.entries()) {
      expect(f.fade, `frame ${i}: the fade jumped up before reversing`).toBeLessThanOrEqual(result.at + 0.05);
    }
    expect(result.frames.at(-1)!.coverVisible).toBe(false);
  });

  test('a leaving panel takes no pointer and no focus', async ({ page }) => {
    await open(page);
    const result = await page.evaluate(async () => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.toggle();
      await fx.until((f: Frame) => f.fade === 1);
      fx.toggle();
      await new Promise(r => requestAnimationFrame(r));
      const cover = fx.cover() as HTMLElement;
      const box = cover.getBoundingClientRect();
      const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2);
      return {
        stillFading: fx.read().fade > 0.5,
        inert: cover.inert,
        hitInside: !!hit && cover.contains(hit),
        focusInside: cover.contains(document.activeElement),
      };
    });
    expect(result.stillFading, 'the check missed the fade out').toBe(true);
    expect(result.inert).toBe(true);
    expect(result.hitInside, 'a pointer still lands on the leaving panel').toBe(false);
    expect(result.focusInside).toBe(false);
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

    // A status pick closes the panel: glyph, title and panel all move together.
    frames = await run('Review');
    expect(crossfaded(frames, 'all', 'review'), 'funnel to status did not fade').toBe(true);
    expect(frames.some(f => mid(f.title.threads) && mid(f.title.filters)), 'the title did not fade').toBe(true);
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

  test('every fade starts in the same frame, opening and closing', async ({ page }) => {
    await open(page);
    const starts = await page.evaluate(async () => {
      const fx = (window as unknown as { __fx: any }).__fx;
      const header = fx.button().closest('.threads-header, .mobile-threads-header') as HTMLElement;
      const scope = [header, fx.cover()];
      const collect = async () => {
        await new Promise(r => requestAnimationFrame(r));
        const anims = document.getAnimations().filter((a): a is CSSTransition =>
          a instanceof CSSTransition && a.transitionProperty === 'opacity'
          && scope.some(s => s.contains((a.effect as KeyframeEffect).target as Node)));
        await Promise.all(anims.map(a => a.ready));
        return anims.map(a => ({
          what: ((a.effect as KeyframeEffect).target as HTMLElement).className,
          start: a.startTime as number,
        }));
      };
      fx.toggle();
      const opening = await collect();
      await fx.until((f: Frame) => f.fade === 1);
      fx.toggle();
      const closing = await collect();
      return { opening, closing };
    });

    for (const [phase, list] of Object.entries(starts)) {
      const names = list.map(a => a.what).join(' | ');
      // The title's two words and the panel's options, at the very least.
      expect(list.length, `${phase}: too few fades found (${names})`).toBeGreaterThanOrEqual(3);
      expect(names, `${phase}: the panel options did not fade`).toContain('thread-filter-fade');
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
      const seconds = (el: Element | null) => Math.max(
        ...getComputedStyle(el!).transitionDuration.split(',').map(s => parseFloat(s)),
      );
      fx.toggle();
      await new Promise(r => requestAnimationFrame(r));
      await new Promise(r => requestAnimationFrame(r));
      const opened = fx.read();
      const button = fx.button() as HTMLElement;
      const durations = {
        fade: seconds(fx.cover().querySelector('.thread-filter-fade')),
        glyph: seconds(button.querySelector('.filter-glyph .crossfade-layer')),
        wrapper: seconds(button.querySelector('.filter-glyph')),
        title: seconds(fx.title().querySelector('.crossfade-layer')),
        badge: seconds(button.querySelector('.badge')),
      };
      fx.toggle();
      await new Promise(r => requestAnimationFrame(r));
      await new Promise(r => requestAnimationFrame(r));
      return { opened, closed: fx.read(), durations };
    });
    expect(result.opened.fade).toBe(1);
    expect(result.opened.title.filters).toBe(1);
    for (const [what, s] of Object.entries(result.durations)) {
      expect(s, `${what} still animates under reduced motion`).toBeLessThan(0.001);
    }
    expect(result.closed.coverVisible).toBe(false);
    expect(result.closed.title.threads).toBe(1);
  });

  test('a panel restored open by a reload shows at once, with no fade', async ({ page }) => {
    await open(page, { slow: false });
    await page.evaluate(() => (window as unknown as { __fx: any }).__fx.toggle());
    await expect(page.locator(`${selectors(page).drawer} .thread-filter-cover[data-open]`)).toHaveCount(1);
    await page.reload();
    const boot = await page.waitForFunction(() => {
      const fx = (window as unknown as { __fx: any }).__fx;
      const cover = fx?.cover();
      if (!cover?.hasAttribute('data-open')) return null;
      const fade = cover.querySelector('.thread-filter-fade');
      return { running: fade.getAnimations().length, opacity: getComputedStyle(fade).opacity };
    }, undefined, { timeout: 15_000, polling: 'raf' });
    expect(await boot.jsonValue()).toEqual({ running: 0, opacity: '1' });
    await page.evaluate(() => (window as unknown as { __fx: any }).__fx.toggle());
  });
});
