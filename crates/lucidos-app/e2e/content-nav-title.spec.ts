import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp, openTriggersPanel, clickVisibleElement, isMobileViewport } from './helpers';

// A content-pane navigation fades its view in from behind the navigation cover,
// while the header title switches word at once, with no fade of its own. The
// drawer's Threads/Filters swap dips instead
// (threads-header-filter-transitions.spec).
// Unsuffixed, so it runs on desktop Chromium, phone Chromium and iPhone WebKit.

/** The slider's slowest position: 0.1x, so --duration-normal lasts 2s. */
const SLOWEST = '-10';
/** A slowed arrival, plus its fuse and room for the frame it lands on. */
const SLOW_FADE_MS = 2_900;

interface Frame { veil: number; text: string; opacity: number }

function titleSelector(page: Page): string {
  return isMobileViewport(page)
    ? '.mobile-content-header .mobile-content-title'
    : '.desktop-header .pane-header-content-title .pane-header-title-text';
}

/** Opens the nav drawer, then picks `label` and samples the frames after the
 *  click on the page's own clock. */
async function navigateAndSample(page: Page, label: string): Promise<{ frames: Frame[]; starts: number[] }> {
  await clickVisibleElement(page, '.hamburger-panel');
  await expect(page.locator('.drawer-item:visible', { hasText: label }).first()).toBeVisible();
  return page.evaluate(async ({ label, title, ms }) => {
    const opacity = (el: Element | null) => (el ? parseFloat(getComputedStyle(el).opacity) : -1);
    const read = () => {
      const t = document.querySelector(title);
      return { veil: opacity(document.querySelector('.content-pane > .nav-cover')), text: t?.textContent ?? '', opacity: opacity(t) };
    };
    const item = Array.from(document.querySelectorAll<HTMLElement>('.drawer-item'))
      .find(el => el.getBoundingClientRect().width > 0 && (el.textContent ?? '').includes(label));
    if (!item) throw new Error(`no drawer item "${label}"`);
    item.click();
    await new Promise(r => requestAnimationFrame(r));
    const anims = document.getAnimations().filter((a): a is CSSAnimation =>
      a instanceof CSSAnimation && a.animationName === 'nav-cover-clear');
    await Promise.all(anims.map(a => a.ready));
    const starts = anims.map(a => a.startTime as number);
    const frames = [read()];
    const end = performance.now() + ms;
    while (performance.now() < end) {
      await new Promise(r => requestAnimationFrame(r));
      frames.push(read());
    }
    return { frames, starts };
  }, { label, title: titleSelector(page), ms: SLOW_FADE_MS });
}

test.describe('a content-pane navigation', () => {
  test.beforeEach(async ({ page }) => { await assertHealthy(page); });

  test('the view fades in while the title switches at once', async ({ page }) => {
    await page.addInitScript((pos) => localStorage.setItem('lucidos-animation-speed-slider', pos), SLOWEST);
    await navigateToApp(page);
    await openTriggersPanel(page);
    await expect(page.locator(`${titleSelector(page)}:visible`)).toHaveText('Triggers');

    const { frames, starts } = await navigateAndSample(page, 'Files');
    const mid = (v: number) => v > 0.05 && v < 0.95;

    expect(starts.length, 'the view did not fade').toBe(1);
    expect(frames[0].veil, 'the swap frame was not covered').toBeGreaterThan(0.9);
    expect(frames.filter(f => mid(f.veil)).length, 'the view never faded in').toBeGreaterThan(5);
    for (const [i, f] of frames.entries()) {
      expect(f.text, `frame ${i}: the title lagged the view`).toBe('Files');
      expect(f.opacity, `frame ${i}: the title faded`).toBe(1);
    }
    expect(frames.at(-1)!.veil, 'the cover outlived its fuse').toBe(-1);
  });

  test('with motion reduced the title shows at once', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await navigateToApp(page);
    await openTriggersPanel(page);
    const { frames, starts } = await navigateAndSample(page, 'Files');
    expect(starts, 'something still animates under reduced motion').toEqual([]);
    expect(frames[0].text).toBe('Files');
    expect(frames[0].opacity).toBe(1);
    expect(frames[0].veil).toBeLessThanOrEqual(0);
  });
});
