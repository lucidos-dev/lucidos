import { randomUUID } from 'node:crypto';
import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer, isMobileViewport } from './helpers';
import { clearAllThreads, psql, seedThreadRow } from './db-helpers';

// The threads Filter's fades. Every swap of what the drawer shows dips through
// the pane background: the navigation cover rises over the leaving view, holds
// while the views swap, and clears off the arriving one. That covers the list to
// Filters and back, and a grouping change from the header's grouping button.
// The pane title switches word at once, with no fade. The Filter
// button's glyph is button feedback, on --duration-fast. Under Ongoing the
// button fades out in a slot that keeps its box, so nothing beside it moves.
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
  /** The drawing of the leaving list a grouping change holds over the pane. */
  drawing: boolean;
  drawingVisible: boolean;
  /** Count pills in the drawing still painted: one that sets its own
   *  `visibility: visible` beats a hidden drawing unless its opacity is 0. */
  drawingBadges: number;
  drawingText: string;
  glyph: Record<string, number>;
  titleText: string;
  titleOpacity: number;
  /** The Filter button's slot: shown under Folders, faded out under Ongoing. */
  slotShown: boolean;
  slotWidth: number;
  slotOpacity: number;
  /** Search's x: the Filter button must never move it. */
  searchX: number;
}

/** The layout's own copies: the page mounts both header rows, one hidden. */
function selectors(page: Page) {
  const s = isMobileViewport(page)
    ? { header: '.mobile-threads-header', title: '.mobile-header-title', drawer: '.mobile-threads-pane .thread-drawer' }
    : { header: '.desktop-header .threads-header', title: '.threads-header-title', drawer: '.thread-drawer' };
  // The list and the layers over it: the filter cover, the drawing and the veil.
  return { ...s, body: `${s.drawer} > .thread-drawer-body` };
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
      cover: () => q(`${sel.body} > .thread-filter-cover`),
      veil: () => q(`${sel.body} > .nav-cover`),
      drawing: () => q(`${sel.body} > .thread-view-drawing`),
      button: () => q(`${sel.header} button[aria-label="Filter threads"]`),
      title: () => q(`${sel.header} ${sel.title}`),
      read() {
        const cover = fx.cover();
        const button = fx.button();
        const slot = button?.closest('.filter-slot');
        const panel = cover?.querySelector('.thread-filter-panel');
        const list = q(`${sel.body} > .thread-drawer-list`);
        const title = fx.title();
        const drawing = fx.drawing();
        return {
          veil: opacity(fx.veil()),
          coverVisible: !!cover && getComputedStyle(cover).visibility === 'visible',
          panel: !!panel,
          panelVisible: !!panel && getComputedStyle(panel).visibility === 'visible',
          listVisible: !!list && getComputedStyle(list).visibility === 'visible',
          drawing: !!drawing,
          drawingVisible: !!drawing && getComputedStyle(drawing).visibility === 'visible',
          drawingBadges: drawing && opacity(drawing) > 0
            ? Array.from(drawing.querySelectorAll('.section-count-badge'))
              .filter(b => getComputedStyle(b).visibility === 'visible').length
            : 0,
          drawingText: drawing?.textContent ?? '',
          glyph: layers(button?.querySelector('.crossfade-glyph')),
          titleText: title?.textContent ?? '',
          titleOpacity: opacity(title),
          slotShown: !!slot?.hasAttribute('data-shown'),
          slotWidth: slot?.getBoundingClientRect().width ?? -1,
          slotOpacity: opacity(slot),
          searchX: q(`${sel.header} button[aria-label="Search threads"]`)?.getBoundingClientRect().left ?? -1,
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
      /** Press the header's grouping button, which offers `grouping`. */
      pickGrouping(grouping: 'folders' | 'ongoing') {
        const offer = grouping === 'ongoing' ? 'Show Ongoing' : 'Show Folders';
        const button = q(`${sel.header} .grouping-btn`);
        if (!button?.getAttribute('aria-label')?.startsWith(offer)) throw new Error(`the grouping button does not offer ${grouping}`);
        button.click();
      },
      /** A row in the panel, by its visible label. */
      clickPanelRow(label: string) {
        const rows = Array.from(fx.cover()!.querySelectorAll<HTMLElement>('.thread-filter-option'));
        const row = rows.find(r => !r.classList.contains('thread-filter-option-child')
          && (r.textContent ?? '').trim().startsWith(label));
        if (!row) throw new Error(`no panel row "${label}"`);
        (row.querySelector('input') ?? row).click();
      },
    };
    (window as unknown as { __fx: typeof fx }).__fx = fx;
  }, selectors(page));
}

async function open(page: Page, { slow = true, grouping }: { slow?: boolean; grouping?: 'ongoing' } = {}): Promise<void> {
  await installProbe(page);
  // Folders is the default, stored as no key at all.
  if (grouping) await page.addInitScript((g) => localStorage.setItem('lucidos-drawer-grouping', g), grouping);
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
      const word = arriving === 'threads' ? 'Folders' : 'Filters';
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
    expect(result.end.titleText).toBe('Folders');
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

  test('the glyph crossfades on a type filter, and back', async ({ page }) => {
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
    // Untick Lucidos: thread types now narrow the list, so the funnel fills.
    let frames = await run('Lucidos');
    expect(crossfaded(frames, 'all', 'filtered'), 'outline to solid did not fade').toBe(true);
    settledOn(frames, 'filtered');
    frames = await run('Lucidos');
    expect(crossfaded(frames, 'filtered', 'all'), 'solid to outline did not fade').toBe(true);
    settledOn(frames, 'all');
  });

  // A grouping change replaces the list. It must play the dip a panel swap
  // plays: the leaving list stays on screen (as a drawing) until the midpoint,
  // and the arriving list shows only under the opaque cover.
  const groupingCases = [
    { name: 'Folders to Ongoing', from: undefined, to: 'ongoing', leavingText: 'grouping-dip' },
    { name: 'Ongoing to Folders', from: 'ongoing', to: 'folders', leavingText: 'Blocked' },
  ] as const;
  for (const c of groupingCases) {
    test(`a grouping change from ${c.name} dips like a panel swap`, async ({ page }) => {
      psql(seedThreadRow({ id: randomUUID(), title: 'grouping-dip', now: new Date().toISOString(), status: 'failed', archiveState: 'inbox' }));
      await open(page, { grouping: c.from });
      const { drawer } = selectors(page);
      // The leaving list is on screen, with something in it to draw.
      await expect(page.locator(c.from === 'ongoing' ? `${drawer} .ongoing-grouping` : `${drawer} .thread-row`, { hasText: c.leavingText }).first())
        .toBeVisible();
      const frames = await page.evaluate(async ({ ms, to }) => {
        const fx = (window as unknown as { __fx: any }).__fx;
        fx.pickGrouping(to);
        return fx.sample(ms);
      }, { ms: SLOW_FADE_MS, to: c.to }) as Frame[];

      const shows = (f: Frame) => (f.drawingVisible ? 'leaving' : 'arriving');
      for (const [i, f] of frames.entries()) {
        expect(f.panelVisible, `frame ${i}: the panel showed`).toBe(false);
        // The title names the arriving grouping from the first frame.
        expect(f.titleText, `frame ${i}: the title lagged the tap`).toBe(c.to === 'ongoing' ? 'Ongoing' : 'Folders');
        expect(f.titleOpacity, `frame ${i}: the title faded`).toBe(1);
        // The grouping moves no glyph: the type filter is what fills the funnel.
        expect(f.glyph.all, `frame ${i}: the glyph moved`).toBe(1);
      }
      // It starts over the leaving list, drawn as it was, under a clear cover.
      expect(shows(frames[0]), 'the leaving list went before the dip').toBe('leaving');
      expect(frames[0].drawingText, 'the drawing is not the leaving list').toContain(c.leavingText);
      expect(frames[0].veil, 'the cover started opaque').toBeLessThan(0.2);
      // One swap, under an opaque cover.
      const swap = frames.findIndex(f => shows(f) === 'arriving');
      expect(swap, 'the arriving list never showed').toBeGreaterThan(0);
      expect(frames.slice(swap).every(f => shows(f) === 'arriving'), 'the drawing came back').toBe(true);
      // Nothing in the drawing outlives the midpoint, a collapsed count included.
      for (const [i, f] of frames.slice(swap).entries()) {
        expect(f.drawingBadges, `frame ${swap + i}: a count in the drawing stayed on screen`).toBe(0);
      }
      expect(frames[swap - 1].veil, 'the leaving list went under a thin cover').toBeGreaterThan(0.9);
      expect(frames[swap].veil, 'the arriving list showed under a thin cover').toBeGreaterThan(0.9);
      // The same shape as a panel swap: rise, hold, fall, unmount.
      const rising = frames.slice(0, swap);
      const falling = frames.slice(swap).filter(f => f.veil >= 0);
      expect(rising.filter(f => mid(f.veil)).length, 'the leaving list never faded out').toBeGreaterThan(5);
      expect(falling.filter(f => mid(f.veil)).length, 'the arriving list never faded in').toBeGreaterThan(5);
      for (let i = 1; i < rising.length; i++) {
        expect(rising[i].veil, `frame ${i}: the cover fell before the swap`).toBeGreaterThanOrEqual(rising[i - 1].veil - 0.01);
      }
      for (let i = 1; i < falling.length; i++) {
        expect(falling[i].veil, `frame ${swap + i}: the cover rose after the swap`).toBeLessThanOrEqual(falling[i - 1].veil + 0.01);
      }
      expect(frames.at(-1)!.veil, 'the cover outlived its fuse').toBe(-1);
      expect(frames.at(-1)!.drawing, 'the drawing outlived its fuse').toBe(false);
      expect(frames.at(-1)!.listVisible).toBe(true);
      await expect(page.locator(`${selectors(page).header} .grouping-btn`))
        .toHaveAttribute('aria-label', c.to === 'ongoing' ? 'Show Folders' : /^Show Ongoing/);
    });
  }

  test('a tap mid-dip lands on neither list', async ({ page }) => {
    psql(seedThreadRow({ id: randomUUID(), title: 'grouping-tap', now: new Date().toISOString(), status: 'failed', archiveState: 'inbox' }));
    await open(page);
    await expect(page.locator(`${selectors(page).drawer} .thread-row`, { hasText: 'grouping-tap' })).toBeVisible();
    const result = await page.evaluate(async () => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.pickGrouping('ongoing');
      await fx.until((f: Frame) => f.veil > 0.3 && f.drawingVisible);
      const drawing = fx.drawing() as HTMLElement;
      const list = fx.cover().parentElement.querySelector('.thread-drawer-list') as HTMLElement;
      const box = drawing.getBoundingClientRect();
      // Aim at the drawn row too: a live row stood there a moment ago.
      const drawnRow = Array.from(drawing.querySelectorAll<HTMLElement>('.thread-row'))
        .find(r => (r.textContent ?? '').includes('grouping-tap'))!.getBoundingClientRect();
      const points = [[box.left + box.width / 2, box.top + box.height / 2], [drawnRow.left + drawnRow.width / 2, drawnRow.top + drawnRow.height / 2]];
      const hits = points.map(([x, y]) => document.elementFromPoint(x, y));
      return {
        onScreen: fx.read().drawingVisible,
        contentInert: (drawing.firstElementChild as HTMLElement).inert,
        onFrame: hits.every(h => h === drawing),
        onList: hits.some(h => !!h && list.contains(h)),
      };
    });
    expect(result.onScreen, 'the check missed the dip').toBe(true);
    expect(result.contentInert, 'the drawn list takes input').toBe(true);
    expect(result.onFrame, 'a tap missed the drawing frame').toBe(true);
    expect(result.onList, 'a tap reached the arriving list before it showed').toBe(false);
  });

  test('opening the panel mid-dip holds the drawing to the new midpoint, with no flash', async ({ page }) => {
    // The panel opens under Folders only, so the dip starts from Ongoing.
    await open(page, { grouping: 'ongoing' });
    const result = await page.evaluate(async (ms) => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.pickGrouping('folders');
      await fx.until((f: Frame) => f.veil > 0.4 && f.drawingVisible);
      fx.toggle();
      return fx.sample(ms);
    }, SLOW_FADE_MS) as Frame[];
    const shows = (f: Frame) => (f.panelVisible ? 'filters' : f.drawingVisible ? 'leaving' : 'list');
    expect(result[0].veil, 'the new dip did not start from its beginning').toBeLessThan(0.2);
    for (const [i, f] of result.entries()) {
      expect(shows(f), `frame ${i}: the list hidden under the drawing flashed`).not.toBe('list');
    }
    const swap = result.findIndex(f => shows(f) === 'filters');
    expect(swap, 'the panel never showed').toBeGreaterThan(0);
    expect(result[swap].veil, 'the panel showed under a thin cover').toBeGreaterThan(0.9);
    expect(result.at(-1)!.titleText).toBe('Filters');
  });

  test('a grouping change with motion reduced swaps at once', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await open(page, { slow: false });
    const { after, slotMs } = await page.evaluate(async () => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.pickGrouping('ongoing');
      await new Promise(r => requestAnimationFrame(r));
      await new Promise(r => requestAnimationFrame(r));
      // "At once" is the slot's transition length, not a frame count. A
      // stalled animation clock can leave even a 0.3ms fade unfinished two
      // frames on, as it does after drawer-grouping.spec.ts in one run.
      const anims = fx.button()?.closest('.filter-slot')?.getAnimations() ?? [];
      const slotMs = Math.max(0, ...anims.map((a: Animation) => Number(a.effect?.getComputedTiming().duration ?? 0)));
      await Promise.all(anims.map((a: Animation) => a.finished));
      return { after: fx.read(), slotMs };
    }) as { after: Frame; slotMs: number };
    expect(after.drawingVisible, 'the drawing is drawn under reduced motion').toBe(false);
    expect(after.veil).toBeLessThanOrEqual(0);
    expect(after.listVisible).toBe(true);
    expect(slotMs, 'the Filter button took time to leave').toBeLessThanOrEqual(1);
    expect(after.slotOpacity, 'the Filter button is still painted').toBe(0);
    await expect(page.locator(`${selectors(page).drawer} .ongoing-grouping`)).toBeVisible();
  });

  test('under Ongoing the Filter button fades out in place, and fades back under Folders', async ({ page }) => {
    await open(page);
    const before = await page.evaluate(() => (window as unknown as { __fx: any }).__fx.read()) as Frame;
    expect(before.slotShown).toBe(true);
    expect(before.slotOpacity).toBe(1);
    const { leaving, returning } = await page.evaluate(async (ms) => {
      const fx = (window as unknown as { __fx: any }).__fx;
      fx.pickGrouping('ongoing');
      const leaving = await fx.sample(ms);
      fx.pickGrouping('folders');
      const returning = await fx.sample(ms);
      return { leaving, returning };
    }, SLOW_FADE_MS) as { leaving: Frame[]; returning: Frame[] };

    const fading = (o: number) => o > 0.05 && o < 0.95;
    for (const [phase, frames] of [['leave', leaving], ['return', returning]] as const) {
      for (const [i, f] of frames.entries()) {
        expect(f.slotShown, `${phase} frame ${i}: the slot lagged the pick`).toBe(phase === 'return');
        // Nothing moves: the slot keeps its box, and Search stays put.
        expect(Math.abs(f.slotWidth - before.slotWidth), `${phase} frame ${i}: the slot changed width`).toBeLessThan(0.5);
        expect(Math.abs(f.searchX - before.searchX), `${phase} frame ${i}: Search moved`).toBeLessThan(0.5);
      }
      expect(frames.filter(f => fading(f.slotOpacity)).length, `${phase}: the button never faded`).toBeGreaterThan(5);
    }
    expect(leaving.at(-1)!.slotOpacity, 'the button stayed under Ongoing').toBe(0);
    expect(returning.at(-1)!.slotOpacity, 'the button did not come back whole').toBe(1);
  });

  test('the funnel rim is painted once: the wrapper carries the translucency', async ({ page }) => {
    await open(page, { slow: false });
    const { header } = selectors(page);
    const button = page.locator(`${header} button[aria-label="Filter threads"]`);
    const read = () => button.evaluate((btn) => ({
      wrapper: getComputedStyle(btn.querySelector('.crossfade-glyph')!).opacity,
      shapes: Array.from(btn.querySelectorAll('.crossfade-glyph svg')).map(s => getComputedStyle(s).color),
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
        glyph: seconds(button.querySelector('.crossfade-glyph .crossfade-layer')),
        wrapper: seconds(button.querySelector('.crossfade-glyph')),
        slot: seconds(button.closest('.filter-slot')),
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
    expect(result.closed.titleText).toBe('Folders');
  });

  test('a panel restored open by a reload shows at once, with no fade', async ({ page }) => {
    await open(page, { slow: false });
    await page.evaluate(() => (window as unknown as { __fx: any }).__fx.toggle());
    await expect(page.locator(`${selectors(page).body} > .thread-filter-cover[data-open]`)).toHaveCount(1);
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
