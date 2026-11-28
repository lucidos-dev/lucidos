import { test, expect, type Page } from './fixtures';
import {
  assertHealthy,
  blurActiveElement,
  disableMobileDynamicBars,
  disarmFollowSeed,
  enableMobileDynamicBars,
  navigateToApp,
  sendMessage,
  uniqueMessage,
  waitForResponse,
} from './helpers';


const PORTRAIT = { width: 390, height: 844 };
const LANDSCAPE = { width: 844, height: 390 };
const TABLET = { width: 1180, height: 820 };
const CONTAINER = '.mobile-swipe-pane .thread-content.visible';
const PROMPT = '.mobile-swipe-pane .prompt-area';

async function rotate(page: Page, size: { width: number; height: number }): Promise<void> {
  await page.setViewportSize(size);
  await page.waitForFunction(({ width }) => window.innerWidth === width, size);
}

function layout(page: Page): Promise<{ phone: boolean; split: boolean }> {
  return page.evaluate(() => ({
    phone: !!document.querySelector('.mobile-swipe-wrapper'),
    split: !!document.querySelector('.split-layout'),
  }));
}

async function tallThreadAtTop(page: Page): Promise<void> {
  await navigateToApp(page);
  await sendMessage(page, `Say exactly: "${uniqueMessage('phone-landscape')}"`);
  await waitForResponse(page);
  await blurActiveElement(page);
  await page.evaluate((sel) => {
    const container = document.querySelector(sel);
    if (!container) return;
    const filler = document.createElement('div');
    filler.style.minHeight = '3000px';
    filler.style.flexShrink = '0';
    container.appendChild(filler);
    container.scrollTop = 0;
  }, CONTAINER);
}

async function scrollInSteps(page: Page, total: number): Promise<void> {
  await page.evaluate(async ({ sel, total }) => {
    const c = document.querySelector(sel);
    if (!c) return;
    const step = 80 * Math.sign(total);
    for (let moved = 0; Math.abs(moved) < Math.abs(total); moved += step) {
      const before = c.scrollTop;
      c.scrollTop = before + step;
      if (c.scrollTop === before) break;
      await new Promise((r) => setTimeout(r, 80));
    }
  }, { sel: CONTAINER, total });
}

function promptHiddenPx(page: Page): Promise<number> {
  return page.evaluate(({ sel, container }) => {
    const prompt = document.querySelector(sel);
    const pane = document.querySelector(container)?.closest('.thread-pane');
    if (!prompt || !pane) return -1;
    return prompt.getBoundingClientRect().bottom - pane.getBoundingClientRect().bottom;
  }, { sel: PROMPT, container: CONTAINER });
}

async function expectBarsTrackScroll(page: Page): Promise<void> {
  await scrollInSteps(page, -10_000);
  await expect.poll(() => promptHiddenPx(page)).toBeLessThanOrEqual(1);
  await scrollInSteps(page, 800);
  await expect.poll(() => promptHiddenPx(page)).toBeGreaterThan(10);
}

test.describe('A phone in landscape', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await disarmFollowSeed(page);
  });

  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
  });

  test('keeps the phone layout and its fixed header', async ({ page }) => {
    await navigateToApp(page);
    await rotate(page, LANDSCAPE);
    await expect.poll(() => layout(page)).toEqual({ phone: true, split: false });
    const position = await page.evaluate(() => getComputedStyle(document.querySelector('.app-header')!).position);
    expect(position).toBe('fixed');
  });

  test('keeps the dynamic bars working across a rotation and back', async ({ page }) => {
    await enableMobileDynamicBars(page);
    await tallThreadAtTop(page);
    await rotate(page, LANDSCAPE);
    await expectBarsTrackScroll(page);
    await rotate(page, PORTRAIT);
    await expectBarsTrackScroll(page);
  });

  test('rebinds the bars after the desktop split and back, leaving no offset behind', async ({ page }) => {
    await enableMobileDynamicBars(page);
    await tallThreadAtTop(page);
    await scrollInSteps(page, 800);
    await expect.poll(() => promptHiddenPx(page)).toBeGreaterThan(10);

    await rotate(page, TABLET);
    await expect.poll(() => layout(page)).toEqual({ phone: false, split: true });
    const headerTranslate = await page.evaluate(() => document.querySelector<HTMLElement>('.app-header')!.style.translate);
    expect(headerTranslate).toBe('');

    await rotate(page, PORTRAIT);
    await expect.poll(() => layout(page)).toEqual({ phone: true, split: false });
    await page.waitForSelector(CONTAINER);
    await page.evaluate((sel) => {
      const container = document.querySelector(sel);
      if (!container) return;
      const filler = document.createElement('div');
      filler.style.minHeight = '3000px';
      filler.style.flexShrink = '0';
      container.appendChild(filler);
    }, CONTAINER);
    await expectBarsTrackScroll(page);
  });

  test('keeps every control clear of the Dynamic Island, with full-bleed bands', async ({ page }) => {
    await tallThreadAtTop(page);
    await rotate(page, LANDSCAPE);
    const island = 59;
    await page.evaluate((px) => {
      const s = document.documentElement.style;
      s.setProperty('--safe-area-floor-left', `${px}px`);
      s.setProperty('--safe-area-floor-right', '0px');
      s.setProperty('--safe-area-floor-bottom', '21px');
    }, island);

    const report = await page.evaluate(({ px, container }) => {
      const width = window.innerWidth;
      const visible = (el: Element) => {
        const r = el.getBoundingClientRect();
        const cs = getComputedStyle(el);
        return r.width > 0 && r.height > 0 && cs.visibility !== 'hidden' && cs.display !== 'none'
          && r.right > 0 && r.left < width;
      };
      const controls = Array.from(document.querySelectorAll(
        `.app-header button, ${container} .mobile-thread-title-row button, .mobile-swipe-pane .prompt-area button, .mobile-swipe-pane .prompt-area textarea`,
      )).filter(visible);
      const intruders = controls
        .map((el) => ({ el, r: el.getBoundingClientRect() }))
        .filter(({ r }) => r.left < px - 0.5 || r.right > width - px + 0.5)
        .map(({ el, r }) => `${el.getAttribute('aria-label') ?? el.className} @ ${Math.round(r.left)}-${Math.round(r.right)}`);
      const header = document.querySelector('.app-header')!.getBoundingClientRect();
      const band = document.querySelector(`${container} .mobile-thread-title-row`)!.getBoundingClientRect();
      const text = parseFloat(getComputedStyle(document.querySelector(container)!).paddingLeft);
      return {
        checked: controls.length,
        intruders,
        header: [Math.round(header.left), Math.round(header.right)],
        band: [Math.round(band.left), Math.round(band.right)],
        textInset: text,
        width,
      };
    }, { px: island, container: CONTAINER });

    expect(report.checked).toBeGreaterThan(3);
    expect(report.intruders).toEqual([]);
    expect(report.header).toEqual([0, report.width]);
    expect(report.band).toEqual([0, report.width]);
    expect(report.textInset).toBeGreaterThanOrEqual(island);
  });
});
