/**
 * Mobile: a pane swipe reaches no invalid state.
 *
 * The strip follows one lead at a time, only a lift commits, and every drag
 * ends at rest (`components/layout/paneDrag.ts`). The rules are unit-tested.
 * This spec checks the strip itself in a browser, for the four shapes the
 * user felt as "weird intermittent states":
 *
 *   - a cancelled far drag, which used to switch panes;
 *   - a thumb resting on the edge strip, which used to hijack every other
 *     touch and cancel its scroll;
 *   - a navigation mid-drag, which used to leave every later move animating;
 *   - a drag during a slide, which used to jump the strip to the slide's end.
 *
 * Touches are generic `Event`s with hand-built lists, because WebKit refuses
 * the Touch and TouchEvent constructors. Each touch carries an identifier.
 */
import { test, expect, Page } from './fixtures';
import { assertHealthy, ensureOnThreadPane, navigateToApp, waitForPaneAtRest } from './helpers';

// Reads real motion, so it opts out of the calm default (playwright.config.ts).
test.use({ reducedMotion: 'no-preference' });

interface Point { id: number; x: number; y: number }

/** Dispatch one touch event on the element under its changed touch. Returns
 *  whether the strip cancelled it. */
async function fire(page: Page, type: string, touches: Point[], changed: Point[]): Promise<boolean> {
  return page.evaluate(({ type, touches, changed }) => {
    const at = changed[changed.length - 1];
    const target = document.elementFromPoint(at.x, at.y);
    if (!target) throw new Error(`nothing at ${at.x},${at.y}`);
    const toTouch = (p: { id: number; x: number; y: number }) => ({
      identifier: p.id, target, clientX: p.x, clientY: p.y, pageX: p.x, pageY: p.y, screenX: p.x, screenY: p.y,
    });
    const ev = new Event(type, { bubbles: true, cancelable: true, composed: true });
    Object.defineProperty(ev, 'touches', { value: touches.map(toTouch) });
    Object.defineProperty(ev, 'targetTouches', { value: touches.map(toTouch) });
    Object.defineProperty(ev, 'changedTouches', { value: changed.map(toTouch) });
    target.dispatchEvent(ev);
    return ev.defaultPrevented;
  }, { type, touches, changed });
}

/** Drag touch `id` from `x0` to `x1` in 20px steps, with `others` held down. */
async function dragTo(page: Page, id: number, x0: number, x1: number, y: number, others: Point[] = []): Promise<void> {
  const step = x1 > x0 ? 20 : -20;
  for (let x = x0 + step; step > 0 ? x <= x1 : x >= x1; x += step) {
    await fire(page, 'touchmove', [...others, { id, x, y }], [{ id, x, y }]);
  }
}

const mobileView = (page: Page) => page.locator('.app-header').first().getAttribute('data-mobile-view');

/** The visible pane's left edge, which is 0 when the strip rests on it. */
async function visiblePaneLeft(page: Page, index: number): Promise<number> {
  return page.evaluate((i) => document.querySelectorAll('.mobile-swipe-pane')[i].getBoundingClientRect().left, index);
}

async function screen(page: Page): Promise<{ w: number; y: number }> {
  return page.evaluate(() => ({ w: window.innerWidth, y: Math.round(window.innerHeight * 0.4) }));
}

test.describe('Mobile pane swipe reaches no invalid state', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await ensureOnThreadPane(page);
    await waitForPaneAtRest(page);
  });

  test('a cancelled far drag springs back instead of switching panes', async ({ page }) => {
    const { w, y } = await screen(page);
    const x0 = Math.round(w * 0.8);
    const x1 = Math.round(w * 0.1);
    await fire(page, 'touchstart', [{ id: 1, x: x0, y }], [{ id: 1, x: x0, y }]);
    await dragTo(page, 1, x0, x1, y);
    await fire(page, 'touchcancel', [], [{ id: 1, x: x1, y }]);

    await waitForPaneAtRest(page);
    expect(await mobileView(page)).toBe('thread');
    expect(Math.abs(await visiblePaneLeft(page, 1))).toBeLessThan(0.5);
  });

  test('a thumb resting on the edge strip neither hijacks a swipe nor blocks a scroll', async ({ page }) => {
    const { w, y } = await screen(page);
    const thumb = { id: 1, x: 4, y };
    // The edge guard still cancels the thumb's own touch.
    expect(await fire(page, 'touchstart', [thumb], [thumb])).toBe(true);

    // A vertical scroll beside it keeps its default.
    const mid = Math.round(w * 0.5);
    const scroll = { id: 2, x: mid, y };
    expect(await fire(page, 'touchstart', [thumb, scroll], [scroll])).toBe(false);
    const scrolled = { id: 2, x: mid, y: y + 80 };
    expect(await fire(page, 'touchmove', [thumb, scrolled], [scrolled])).toBe(false);
    await fire(page, 'touchend', [thumb], [scrolled]);
    expect(await mobileView(page)).toBe('thread');

    // A swipe beside it leads the strip and commits on its own lift.
    const x0 = Math.round(w * 0.8);
    const x1 = Math.round(w * 0.1);
    await fire(page, 'touchstart', [thumb, { id: 3, x: x0, y }], [{ id: 3, x: x0, y }]);
    await dragTo(page, 3, x0, x1, y, [thumb]);
    await fire(page, 'touchend', [thumb], [{ id: 3, x: x1, y }]);
    await fire(page, 'touchend', [], [thumb]);

    await waitForPaneAtRest(page);
    expect(await mobileView(page)).toBe('content');
  });

  test('a navigation mid-drag ends the drag, and later moves do nothing', async ({ page }) => {
    const { w, y } = await screen(page);
    const x0 = Math.round(w * 0.6);
    await fire(page, 'touchstart', [{ id: 1, x: x0, y }], [{ id: 1, x: x0, y }]);
    await dragTo(page, 1, x0, Math.round(w * 0.4), y);

    await page.evaluate(() => {
      document.querySelector<HTMLElement>('button.mobile-dot[aria-label="threads view"]')!.click();
    });
    await dragTo(page, 1, Math.round(w * 0.4), Math.round(w * 0.1), y);
    const track = await page.evaluate(() => {
      const el = document.querySelector<HTMLElement>('.mobile-swipe-track')!;
      return { transition: el.style.transition, transform: el.style.transform };
    });
    // The strip slides to the new pane, and the finger no longer moves it.
    expect(track.transition).not.toBe('none');
    expect(track.transform).toBe('translateX(0px)');
    await fire(page, 'touchend', [], [{ id: 1, x: Math.round(w * 0.1), y }]);

    await waitForPaneAtRest(page);
    expect(await mobileView(page)).toBe('threads');
    expect(Math.abs(await visiblePaneLeft(page, 0))).toBeLessThan(0.5);
  });

  test('a drag during a slide catches the strip where it is on screen', async ({ page }) => {
    const original = await page.evaluate(() => document.documentElement.style.getPropertyValue('--duration-scale'));
    try {
      // A ten-times slower slide, so the drag lands well inside it.
      await page.evaluate(() => document.documentElement.style.setProperty('--duration-scale', '10'));
      await page.evaluate(() => {
        document.querySelector<HTMLElement>('button.mobile-dot[aria-label="content view"]')!.click();
      });
      await page.waitForTimeout(600);
      const { w, y } = await screen(page);
      const x = Math.round(w * 0.5);

      // A tap leaves the slide running.
      await fire(page, 'touchstart', [{ id: 1, x, y }], [{ id: 1, x, y }]);
      await fire(page, 'touchend', [], [{ id: 1, x, y }]);
      expect(await page.evaluate(() => document.querySelector('.mobile-swipe-track')!.getAnimations().length)).toBeGreaterThan(0);

      // One task, so the slide cannot advance between the two readings.
      const jump = await page.evaluate(({ x, y }) => {
        const track = document.querySelector<HTMLElement>('.mobile-swipe-track')!;
        const offset = () => new DOMMatrix(getComputedStyle(track).transform).m41;
        const target = document.elementFromPoint(x, y)!;
        const send = (type: string, cx: number) => {
          const t = { identifier: 2, target, clientX: cx, clientY: y, pageX: cx, pageY: y, screenX: cx, screenY: y };
          const ev = new Event(type, { bubbles: true, cancelable: true, composed: true });
          Object.defineProperty(ev, 'touches', { value: [t] });
          Object.defineProperty(ev, 'targetTouches', { value: [t] });
          Object.defineProperty(ev, 'changedTouches', { value: [t] });
          target.dispatchEvent(ev);
        };
        const before = offset();
        send('touchstart', x);
        send('touchmove', x - 12);
        return Math.abs(offset() - before);
      }, { x, y });
      // The lock threshold leaves 4px of visible drag. Jumping to the slide's
      // end moved the strip by most of a pane.
      expect(jump).toBeLessThan(20);

      await fire(page, 'touchend', [], [{ id: 2, x: x - 12, y }]);
      expect(await mobileView(page)).toBe('content');
    } finally {
      await page.evaluate((v) => {
        if (v) document.documentElement.style.setProperty('--duration-scale', v);
        else document.documentElement.style.removeProperty('--duration-scale');
      }, original);
    }
  });
});
