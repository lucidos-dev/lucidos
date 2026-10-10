import { mkdirSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect, type Page } from './fixtures';
import {
  apiRequest, ensureMobileView, gotoWithRetry, isMobileViewport, shellDeviceId, waitForPaneAtRest, waitForVisibleInput,
} from './helpers';

// Retro theme parts in a real browser (ADR 0313), on desktop Chromium and
// mobile WebKit. Every test uses a fixture theme from e2e/themes/, never a
// shipped theme.
//
//   - scanlines paint on the screen fill, under every protected surface;
//   - the composer caret is a block where the browser draws one, a protected
//     input keeps the normal caret, and reduced motion stops the blink;
//   - floating surfaces and step cards take a double frame, and a protected
//     dialog keeps its solid one;
//   - theme-effects `reduce` turns the scanlines off and keeps the rest;
//   - the default theme paints none of it.

const RETRO = 'e2e-retro-phosphor';
const here = dirname(fileURLToPath(import.meta.url));
const fixture = readFileSync(resolve(here, 'themes', 'retro-phosphor.json'), 'utf-8');
const SCANLINES = '--part-screen-background-image';
const GREEN = 'rgb(51, 255, 51)';

/** The shell has booted: the splash is gone and the composer is on screen. */
async function ready(page: Page) {
  await expect(page.locator('.boot-splash')).toHaveCount(0, { timeout: 30_000 });
  return waitForVisibleInput(page);
}

async function setPreference(page: Page, key: string, value: string): Promise<void> {
  const device = await shellDeviceId(page);
  const res = await apiRequest(page).put(`/api/v1/preferences?key=${key}`, { data: { value, device_id: device } });
  expect(res.ok(), `${key}=${value}`).toBe(true);
  expect((await res.json()).success, `${key}=${value}`).toBe(true);
}

async function style(page: Page, selector: string, prop: string): Promise<string> {
  return page.locator(selector).first().evaluate(
    (el, p) => getComputedStyle(el).getPropertyValue(p), prop,
  );
}

async function waitForScanlines(page: Page, present: boolean): Promise<void> {
  await expect.poll(() => page.evaluate(
    (t) => document.documentElement.style.getPropertyValue(t), SCANLINES,
  )).toEqual(present ? expect.stringMatching(/\S/) : '');
}

/** The screen fills that carry the scanlines in this layout. */
function screens(page: Page): string[] {
  return isMobileViewport(page) ? ['.app-shell', '.mobile-swipe-pane'] : ['.app-shell'];
}

/** Protected cards and a floating surface, placed where real ones render: the
 *  content pane body, as the inline forms and the Changes panel are, or the
 *  thread pane beside the composer. The opaque card is a bare filled box, so
 *  a pixel check tells whether anything paints over it. */
async function injectCards(page: Page, where: 'content' | 'thread' = 'content'): Promise<void> {
  await ready(page);
  if (where === 'content') {
    await ensureMobileView(page, 'content');
    await waitForPaneAtRest(page);
  }
  await page.evaluate((hostSelectors) => {
    document.querySelector('#rt-root')?.remove();
    const host = hostSelectors
      .flatMap(s => [...document.querySelectorAll<HTMLElement>(s)])
      .find(el => el.getBoundingClientRect().width > 0);
    if (!host) throw new Error('no visible pane to hold the cards');
    const root = document.createElement('div');
    root.id = 'rt-root';
    root.innerHTML = `
      <div class="question-body protected-surface" id="rt-question">
        <span class="question-text">Allow this command?</span>
      </div>
      <div class="permission-body protected-surface" id="rt-opaque"
           style="background-color: rgb(20, 40, 80); border-radius: 0; height: 3rem"></div>
      <div id="rt-gap" style="height: 2rem"></div>
      <pre class="step-detail-result" id="rt-card">ls -la  # a step card</pre>
      <div class="surface" id="rt-surface" style="padding: 0.75rem">A floating surface</div>
      <div class="surface confirm-dialog protected-surface" id="rt-dialog" style="padding: 0.75rem">
        A protected dialog
      </div>
      <div class="prompt-area"><div class="prompt-row"><div class="protected-surface">
        <textarea class="prompt-textarea" id="rt-protected-input">secret</textarea>
      </div></div></div>`;
    host.prepend(root);
  }, where === 'content' ? ['.content-pane-body'] : ['.thread-content', '.thread-pane']);
}

test.describe('retro theme parts', () => {
  test.beforeEach(async ({ page }) => {
    await gotoWithRetry(page, '/');
    const res = await apiRequest(page).put(`/api/v1/data/themes/${RETRO}.json`, { data: fixture });
    expect(res.ok(), `saving ${RETRO}: ${await res.text()}`).toBe(true);
  });

  test.afterEach(async ({ page }) => {
    await setPreference(page, 'theme', 'lucidos');
    await setPreference(page, 'theme-effects', 'system');
    await setPreference(page, 'motion', 'system');
    await apiRequest(page).delete(`/api/v1/data/themes/${RETRO}.json`);
  });

  test('scanlines paint on the screen fill, under every protected surface', async ({ page }) => {
    await setPreference(page, 'theme', RETRO);
    await setPreference(page, 'theme-effects', 'full');
    await waitForScanlines(page, true);
    await injectCards(page);

    for (const screen of screens(page)) {
      expect(await style(page, screen, 'background-image'), screen).toContain('repeating-linear-gradient');
    }

    for (const card of ['#rt-question', '#rt-opaque', '#rt-dialog']) {
      const facts = await page.locator(card).evaluate((el) => {
        el.scrollIntoView({ block: 'center' });
        const box = el.getBoundingClientRect();
        const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2);
        return {
          onScreen: el.closest('.app-shell, .mobile-swipe-pane') !== null,
          image: getComputedStyle(el).backgroundImage,
          hit: hit === null ? 'nothing' : hit === el || el.contains(hit) ? 'the card' : hit.outerHTML.slice(0, 120),
        };
      });
      // Under the screen fill's background, so it paints above the lines.
      expect(facts.onScreen, `${card} sits inside a screen fill`).toBe(true);
      expect(facts.image, `${card} paints no scanlines of its own`).toBe('none');
      expect(facts.hit, `${card} takes its own pointer events`).toBe('the card');
    }

    // The proof of order: an opaque protected card looks the same with the
    // scanlines on and off, while the bare gap beside it changes. Each clip
    // stays 2px inside its box, clear of edges that blend at 3x.
    await page.locator('#rt-opaque').evaluate(el => el.scrollIntoView({ block: 'center' }));
    const inside = async (selector: string) => {
      const box = await page.locator(selector).boundingBox();
      if (!box) throw new Error(`${selector} has no box`);
      return { x: box.x + 2, y: box.y + 2, width: box.width - 4, height: box.height - 4 };
    };
    const cardClip = await inside('#rt-opaque');
    const gapClip = await inside('#rt-gap');
    const cardWithLines = await page.screenshot({ clip: cardClip });
    const gapWithLines = await page.screenshot({ clip: gapClip });

    await setPreference(page, 'theme-effects', 'reduce');
    await expect(page.locator('html')).toHaveAttribute('data-theme-effects', 'reduce');
    await expect.poll(() => style(page, '.app-shell', 'background-image')).toBe('none');
    expect((await page.screenshot({ clip: cardClip })).equals(cardWithLines), 'nothing paints over the protected card').toBe(true);
    expect((await page.screenshot({ clip: gapClip })).equals(gapWithLines), 'the lines show beside it').toBe(false);
  });

  test('the composer caret is a block where the browser draws one, and holds still under reduced motion', async ({ page, browserName }) => {
    await setPreference(page, 'theme', RETRO);
    await waitForScanlines(page, true);
    await injectCards(page, 'thread');
    const composer = await ready(page);

    const drawsShape = await page.evaluate(() => CSS.supports('caret-shape', 'block'));
    expect(drawsShape, 'Chromium draws caret-shape; WebKit does not yet').toBe(browserName === 'chromium');
    // WebKit gets the drawn caret instead (ADR 0317, theme-caret-fallback.spec.ts),
    // which hides the native one.
    await expect.poll(() => composer.evaluate(el => getComputedStyle(el).caretColor))
      .toBe(drawsShape ? GREEN : 'rgba(0, 0, 0, 0)');
    if (drawsShape) {
      expect(await composer.evaluate(el => getComputedStyle(el).getPropertyValue('caret-shape'))).toBe('block');
      expect(await style(page, '#rt-protected-input', 'caret-shape'), 'a protected input keeps the normal caret').toBe('auto');

      await setPreference(page, 'motion', 'reduce');
      await expect(page.locator('html')).toHaveAttribute('data-motion', 'reduce');
      expect(await composer.evaluate(el => getComputedStyle(el).getPropertyValue('caret-animation'))).toBe('manual');
    }
  });

  test('floating surfaces and step cards take a double frame, and a protected dialog keeps its own', async ({ page }) => {
    await setPreference(page, 'theme', RETRO);
    await waitForScanlines(page, true);
    await injectCards(page);

    for (const [selector, colour] of [['#rt-surface', GREEN], ['#rt-card', 'rgb(15, 77, 15)']]) {
      expect(await style(page, selector, 'border-top-style'), selector).toBe('double');
      expect(await style(page, selector, 'border-top-width'), selector).toBe('3px');
      expect(await style(page, selector, 'border-top-color'), selector).toBe(colour);
    }
    expect(await style(page, '#rt-dialog', 'border-top-style')).toBe('solid');
    expect(await style(page, '#rt-dialog', 'border-top-width')).toBe('1px');
  });

  test('reduce turns the scanlines off and keeps the caret and the frames', async ({ page, browserName }) => {
    await setPreference(page, 'theme', RETRO);
    await setPreference(page, 'theme-effects', 'reduce');
    await waitForScanlines(page, true);
    await injectCards(page);
    await expect(page.locator('html')).toHaveAttribute('data-theme-effects', 'reduce');

    for (const screen of screens(page)) expect(await style(page, screen, 'background-image'), screen).toBe('none');
    expect(await style(page, '#rt-surface', 'border-top-style')).toBe('double');
    if (browserName === 'chromium') {
      const composer = await ready(page);
      expect(await composer.evaluate(el => getComputedStyle(el).getPropertyValue('caret-shape'))).toBe('block');
    }
  });

  test('the default theme paints no retro part', async ({ page, browserName }) => {
    await injectCards(page);
    for (const screen of screens(page)) expect(await style(page, screen, 'background-image'), screen).toBe('none');
    expect(await style(page, '#rt-surface', 'border-top-style')).toBe('solid');
    expect(await style(page, '#rt-surface', 'border-top-width')).toBe('1px');
    expect(await style(page, '#rt-card', 'border-top-style')).toBe('solid');
    if (browserName === 'chromium') {
      const composer = await ready(page);
      expect(await composer.evaluate(el => getComputedStyle(el).getPropertyValue('caret-shape'))).toBe('auto');
    }
  });

  test('a screenshot of the phosphor theme', async ({ page }, testInfo) => {
    await setPreference(page, 'theme', RETRO);
    await setPreference(page, 'theme-effects', 'full');
    // A still caret, so the screenshot always shows it.
    await setPreference(page, 'motion', 'reduce');
    await waitForScanlines(page, true);
    await injectCards(page, 'thread');
    const composer = await ready(page);
    await composer.click();
    await composer.pressSequentially('ls -la');
    const dir = resolve(here, '..', 'test-results', 'theme-parts-retro');
    mkdirSync(dir, { recursive: true });
    await page.screenshot({ path: resolve(dir, `${testInfo.project.name}.png`), caret: 'initial' });
  });
});
