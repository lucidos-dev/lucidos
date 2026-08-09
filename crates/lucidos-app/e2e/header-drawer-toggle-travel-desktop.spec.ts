import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer } from './helpers';

/**
 * The desktop header's drawer toggle is one opaque icon that TRAVELS, sampled
 * mid-animation rather than only at rest.
 *
 * The bug: the toggle existed twice, once pinned at the header's leading edge
 * and once inside `.pane-header-brand`, crossfading on `data-thread-drawer-open`.
 * Minimizing the drawer faded an icon UP from nothing at a position it had never
 * occupied while its twin slid toward it fading DOWN, so most of the slide
 * carried two half-transparent icons that ended on nearly the same x. Both ENDS
 * of that animation looked correct, which is exactly why the settled-state specs
 * beside this one never caught it: the whole defect lives between them.
 *
 * So this samples every frame of the transition from inside the page. A
 * round-trip per sample would miss most of a 300ms animation, and the point is
 * to test the SHIPPED timing rather than a slowed-down one (the animation-speed
 * scale would let us stretch it, and then we would not be looking at what a user
 * sees).
 *
 * Two animations, the second being the same defect one animation over: the
 * Canvas pane's hamburger slides left into the toggle's resting slot when the
 * Conversation pane collapses, so a toggle that faded in place put a ghost under
 * an arriving control. It travels out sideways with its pane instead.
 *
 * Desktop-only, and named `-desktop` so the mobile projects skip it rather
 * than runtime-skipping (playwright.config.ts `testIgnore`): on mobile the
 * thread list is a swipe pane and its header's toggle never moves.
 *
 * The static half of the contract (nothing declares an opacity on the slot, the
 * two positions are the ones the retired pair sat at) is scanned in
 * `src/styles/__tests__/header-drawer-toggle-travel.test.ts`.
 */

test.use({ viewport: { width: 1280, height: 800 } });

/** Well clear of the drawer's floor (312px at this project's 16px root), so the
 *  travel has real distance to cover and a sample can land in the middle of it. */
const OPEN_DRAWER_WIDTH = 420;

/** How long to keep sampling. Comfortably past `--duration-slow` (300ms) plus
 *  everything that happens before the gesture lands: sampling is armed FIRST, so
 *  the window also has to absorb Playwright's own actionability checks (the
 *  collapse case drives a real double-click on the divider) on a loaded machine.
 *  Over-sampling costs settled frames, which every assertion here holds on. */
const SAMPLE_MS = 1500;

interface Sample {
  /** Frames since sampling began, for failure messages. */
  frame: number;
  /** The toggle's PAINTED box: the button's own rect, intersected with the slot
   *  whenever the slot is clipping it (the collapse exit shrinks the slot behind
   *  a full-size button). Neither box alone is the truth in both directions. */
  toggle: { left: number; right: number; width: number };
  /** Effective opacity of the toggle, multiplied down its ancestor chain. */
  toggleOpacity: number;
  /** How many toggles the desktop header has painted this frame. */
  toggleCount: number;
  /** The drawer header row's right edge (0 while the row has no box). */
  threadsHeaderRight: number;
  /** The Canvas pane header's leading control. */
  hamburgerLeft: number;
}

/**
 * Start a per-frame sampler in the page and leave its promise on `window`.
 *
 * Installed BEFORE the gesture, so the first frames of the transition are in the
 * record; the settled frames it also collects are not noise, since every
 * invariant below holds at rest too.
 */
async function startSampling(page: Page, ms: number): Promise<void> {
  await page.evaluate((durationMs) => {
    const header = document.querySelector('.desktop-header')!;
    const rect = (el: Element | null) => {
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { left: r.left, right: r.right, width: r.width };
    };
    // An element's own opacity says nothing if an ancestor is fading it, and a
    // region fade is exactly what this used to be, so multiply up to the header.
    const effectiveOpacity = (el: Element | null): number => {
      let o = 1;
      for (let n: Element | null = el; n && n !== document.documentElement; n = n.parentElement) {
        o *= parseFloat(getComputedStyle(n).opacity || '1');
        if (n.classList.contains('app-header')) break;
      }
      return o;
    };
    const samples: unknown[] = [];
    (window as unknown as { __toggleSamples: Promise<unknown[]> }).__toggleSamples =
      new Promise<unknown[]>((done) => {
        const started = performance.now();
        let frame = 0;
        // What is actually PAINTED of the toggle. The collapse exit shrinks the
        // slot to zero behind a full-size button under `overflow: clip`, so the
        // button's rect over-reports there; everywhere else the slot is exactly
        // the button and the clip is off, so the slot's rect is right. Take the
        // intersection, gated on the slot actually clipping.
        const paintedToggle = (slot: Element | null) => {
          if (!slot) return null;
          const btn = slot.querySelector('.thread-toggle');
          if (!btn) return null;
          const b = btn.getBoundingClientRect();
          const s = slot.getBoundingClientRect();
          const clips = getComputedStyle(slot).overflowX !== 'visible';
          const left = clips ? Math.max(b.left, s.left) : b.left;
          const right = clips ? Math.min(b.right, s.right) : b.right;
          return { left, right, width: Math.max(0, right - left) };
        };
        const tick = () => {
          const slot = header.querySelector('.thread-toggle-slot');
          const painted = Array.from(header.querySelectorAll('.thread-toggle')).filter((el) => {
            const r = el.getBoundingClientRect();
            return r.width > 0 && effectiveOpacity(el) > 0.01;
          });
          const row = header.querySelector('.threads-header');
          const rowBox = rect(row);
          const burger = rect(header.querySelector('.hamburger-panel'));
          samples.push({
            frame: frame++,
            toggle: paintedToggle(slot),
            toggleOpacity: effectiveOpacity(slot),
            toggleCount: painted.length,
            // A row clipped to nothing is not a right edge to clear.
            threadsHeaderRight: rowBox && rowBox.width > 0 ? rowBox.right : 0,
            hamburgerLeft: burger ? burger.left : Number.POSITIVE_INFINITY,
          });
          if (performance.now() - started < durationMs) requestAnimationFrame(tick);
          else done(samples);
        };
        requestAnimationFrame(tick);
      });
  }, ms);
}

async function collectSamples(page: Page): Promise<Sample[]> {
  return page.evaluate(
    () => (window as unknown as { __toggleSamples: Promise<Sample[]> }).__toggleSamples,
  ) as Promise<Sample[]>;
}

/** Toggle the Conversation pane's collapse. A REAL double-click: SplitLayout
 *  gates the divider's dblclick on a pointerdown-recorded interval
 *  (createDblClickGate), so a bare synthetic dblclick event would be refused. */
async function collapseConversationPane(page: Page): Promise<void> {
  await page.locator('.split-divider').dblclick();
}

/** Every invariant that must hold in EVERY frame of any header animation. */
function assertConsistentThroughout(samples: Sample[], what: string): void {
  expect(samples.length, `${what}: no frames sampled`).toBeGreaterThan(5);
  for (const s of samples) {
    expect(s.toggle, `${what} frame ${s.frame}: the toggle left the header`).not.toBeNull();
    expect(
      s.toggleOpacity,
      `${what} frame ${s.frame}: the toggle was at opacity ${s.toggleOpacity.toFixed(2)}, `
        + 'so it is fading rather than travelling',
    ).toBeCloseTo(1, 2);
    expect(
      s.toggleCount,
      `${what} frame ${s.frame}: ${s.toggleCount} toggles painted at once`,
    ).toBeLessThanOrEqual(1);
  }
}

test.describe('Desktop header: the drawer toggle travels, opaque, alone', () => {
  test.beforeEach(async ({ page, context }) => {
    await assertHealthy(page);
    await context.addInitScript((width) => {
      localStorage.setItem('lucidos-split-ratio', '0.4');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
      localStorage.setItem('lucidos-thread-drawer-width', String(width));
    }, OPEN_DRAWER_WIDTH);
    await navigateToApp(page);
    await openThreadDrawer(page);
    // The open animation is its own transition; start each case from rest.
    await page.waitForTimeout(600);
  });

  test('minimizing the drawer never dims it, doubles it, or crosses the drawer row', async ({ page }) => {
    await startSampling(page, SAMPLE_MS);
    // Synthetic click, deliberately: it reaches ThreadToggleButton's onClick the
    // same way a real one does, and it returns immediately so the sampler is
    // still running for the frame the transition starts on.
    await page.evaluate(() => {
      document.querySelector<HTMLElement>('.desktop-header .thread-toggle-slot .thread-toggle')!.click();
    });
    const samples = await collectSamples(page);

    assertConsistentThroughout(samples, 'minimize');
    for (const s of samples) {
      // The drawer row shrinks from its right edge while the toggle rides just
      // outside it. This is the "icons on top of each other" half: before the
      // fix the second copy sat AT the destination from frame one, inside the
      // row's box for most of the slide.
      expect(
        s.toggle.left,
        `minimize frame ${s.frame}: the toggle (left ${s.toggle.left.toFixed(1)}) was inside `
          + `the drawer row (right ${s.threadsHeaderRight.toFixed(1)})`,
      ).toBeGreaterThanOrEqual(s.threadsHeaderRight - 1);
    }

    // …and it genuinely MOVED rather than being swapped between two positions:
    // a crossfading pair reports only its two endpoints, never the run between.
    const xs = samples.map((s) => s.toggle.left);
    const distinct = new Set(xs.map((x) => Math.round(x)));
    expect(
      distinct.size,
      `the toggle only ever reported ${[...distinct].join(', ')}: it jumped instead of travelling`,
    ).toBeGreaterThan(5);
    expect(Math.max(...xs) - Math.min(...xs), 'the toggle covered no distance')
      .toBeGreaterThan(OPEN_DRAWER_WIDTH / 2);
  });

  test('restoring the drawer is the same animation in reverse', async ({ page }) => {
    await page.evaluate(() => {
      document.querySelector<HTMLElement>('.desktop-header .thread-toggle-slot .thread-toggle')!.click();
    });
    await page.waitForTimeout(600);

    await startSampling(page, SAMPLE_MS);
    await page.evaluate(() => {
      document.querySelector<HTMLElement>('.desktop-header .thread-toggle-slot .thread-toggle')!.click();
    });
    const samples = await collectSamples(page);

    assertConsistentThroughout(samples, 'restore');
    for (const s of samples) {
      expect(
        s.toggle.left,
        `restore frame ${s.frame}: the toggle ran under the opening drawer row`,
      ).toBeGreaterThanOrEqual(s.threadsHeaderRight - 1);
    }
    const xs = samples.map((s) => s.toggle.left);
    expect(Math.max(...xs) - Math.min(...xs), 'the toggle did not travel back')
      .toBeGreaterThan(OPEN_DRAWER_WIDTH / 2);
  });

  test('collapsing the Conversation pane takes it out sideways, not out from under the hamburger', async ({ page }) => {
    await startSampling(page, SAMPLE_MS);
    await collapseConversationPane(page);
    const samples = await collectSamples(page);

    assertConsistentThroughout(samples, 'collapse');
    for (const s of samples) {
      // The whole point: the toggle's resting slot IS where the Canvas pane's
      // hamburger arrives, so leaving by fading put a dimmed ghost under it.
      expect(
        s.toggle.right,
        `collapse frame ${s.frame}: the toggle (right ${s.toggle.right.toFixed(1)}) overlapped `
          + `the hamburger (left ${s.hamburgerLeft.toFixed(1)})`,
      ).toBeLessThanOrEqual(s.hamburgerLeft + 1);
    }

    // Nothing of it is left painted in the slot the hamburger now owns.
    const last = samples[samples.length - 1];
    expect(last.toggle.width, 'the toggle is still painting after the collapse')
      .toBeLessThanOrEqual(1);
  });

  test('re-expanding it brings the toggle back OUTSIDE the growing drawer row', async ({ page }) => {
    // The case the first cut of this change got wrong, and the one the collapse
    // test above cannot see. Sending the toggle out to a negative `left` looked
    // right going out, but `left` interpolates between two resolved lengths and
    // the collapsed one serves the way back too: from a whole icon box LEFT of
    // where the drawer row's right edge starts, the toggle spent 92% of the
    // re-expand inside the growing row, sitting on its Search button. Both ENDS
    // of that animation were correct, which is exactly why only a per-frame
    // sample catches it.
    await collapseConversationPane(page);
    await page.waitForTimeout(600);

    await startSampling(page, SAMPLE_MS);
    await page.locator('.split-divider').dblclick();
    const samples = await collectSamples(page);

    assertConsistentThroughout(samples, 're-expand');
    for (const s of samples) {
      expect(
        s.toggle.left,
        `re-expand frame ${s.frame}: the toggle (left ${s.toggle.left.toFixed(1)}) was inside `
          + `the re-opening drawer row (right ${s.threadsHeaderRight.toFixed(1)})`,
      ).toBeGreaterThanOrEqual(s.threadsHeaderRight - 1);
    }

    // And it came back whole rather than staying clipped.
    const last = samples[samples.length - 1];
    expect(last.toggle.width, 'the toggle did not return to full width')
      .toBeGreaterThan(8);
  });
});
