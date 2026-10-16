import { mkdirSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect, type Locator, type Page } from './fixtures';
import { apiRequest, gotoWithRetry, setDevicePreference, waitForVisibleInput } from './helpers';

// The drawn caret (ADR 0317): a block-caret theme draws its caret in WebKit,
// which has no caret-shape, and Chromium keeps the native one with no overlay.
// The fixture theme is a test theme from e2e/themes/, never a shipped one.
//
// The expected caret position is measured independently of the overlay: a
// canvas measures the text before the caret in the composer's own font.

const THEME = 'e2e-caret-phosphor';
const here = dirname(fileURLToPath(import.meta.url));
const fixture = readFileSync(resolve(here, 'themes', 'retro-phosphor.json'), 'utf-8');
const GREEN = 'rgb(51, 255, 51)';

async function composerWithTheme(page: Page): Promise<Locator> {
  await setDevicePreference(page, 'theme', THEME);
  await expect.poll(() => page.evaluate(
    () => document.documentElement.style.getPropertyValue('--part-composer-text-caret-shape'),
  )).toBe('block');
  await expect(page.locator('.boot-splash')).toHaveCount(0, { timeout: 30_000 });
  const composer = await waitForVisibleInput(page);
  await page.evaluate(() => document.fonts.ready);
  return composer;
}

interface CaretFacts {
  shown: boolean;
  nativeHidden: boolean;
  covered: string | null;
  background: string | null;
  animation: string | null;
  /** How far the drawn caret sits from where the text says it should, in px. */
  dx: number | null;
  dy: number | null;
}

/** Where the drawn caret is, and where the composer's text says it belongs. */
function caretFacts(composer: Locator): Promise<CaretFacts> {
  return composer.evaluate((el: HTMLTextAreaElement): CaretFacts => {
    const overlay = el.nextElementSibling as HTMLElement | null;
    const cursor = overlay?.classList.contains('drawn-caret')
      ? overlay.querySelector<HTMLElement>('.drawn-caret-cursor') : null;
    const shown = !!cursor && !overlay!.hidden;
    const facts: CaretFacts = {
      shown,
      nativeHidden: el.hasAttribute('data-drawn-caret'),
      covered: cursor?.textContent ?? null,
      background: cursor ? getComputedStyle(cursor).backgroundColor : null,
      animation: cursor ? getComputedStyle(cursor).animationName : null,
      dx: null,
      dy: null,
    };
    if (!shown || !cursor) return facts;

    const style = getComputedStyle(el);
    const lines = el.value.slice(0, el.selectionEnd).split('\n');
    const ctx = document.createElement('canvas').getContext('2d')!;
    ctx.font = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
    const lineText = lines[lines.length - 1];
    const spacing = style.letterSpacing === 'normal' ? 0 : parseFloat(style.letterSpacing);
    const box = el.getBoundingClientRect();
    const lineHeight = parseFloat(style.lineHeight);
    const left = box.left + el.clientLeft + parseFloat(style.paddingLeft) - el.scrollLeft
      + ctx.measureText(lineText).width + spacing * [...lineText].length;
    const middle = box.top + el.clientTop + parseFloat(style.paddingTop) - el.scrollTop
      + (lines.length - 0.5) * lineHeight;
    const drawn = cursor.getBoundingClientRect();
    facts.dx = drawn.left - left;
    facts.dy = (drawn.top + drawn.bottom) / 2 - middle;
    return facts;
  });
}

test.describe('the drawn caret', () => {
  test.beforeEach(async ({ page }) => {
    await gotoWithRetry(page, '/');
    const res = await apiRequest(page).put(`/api/v1/data/themes/${THEME}.json`, { data: fixture });
    expect(res.ok(), `saving ${THEME}: ${await res.text()}`).toBe(true);
  });

  test.afterEach(async ({ page }) => {
    await setDevicePreference(page, 'theme', 'lucidos');
    await setDevicePreference(page, 'motion', 'system');
    await apiRequest(page).delete(`/api/v1/data/themes/${THEME}.json`);
  });

  test('WebKit draws the block at the caret, and it follows typing and arrow keys', async ({ page, browserName }) => {
    test.skip(browserName !== 'webkit', 'WebKit has no caret-shape; Chromium is covered below');
    const composer = await composerWithTheme(page);
    await composer.click();
    await composer.pressSequentially('ls -la');

    await expect.poll(async () => (await caretFacts(composer)).shown).toBe(true);
    let facts = await caretFacts(composer);
    expect(facts.nativeHidden).toBe(true);
    expect(await composer.evaluate(el => getComputedStyle(el).caretColor)).toBe('rgba(0, 0, 0, 0)');
    expect(facts.background, 'the theme caret-color').toBe(GREEN);
    expect(facts.covered, 'nothing to cover at the end of the text').toBe('');
    expect(Math.abs(facts.dx!), 'x at the end of the text').toBeLessThan(1.5);
    expect(Math.abs(facts.dy!), 'y at the end of the text').toBeLessThan(2);

    await composer.press('ArrowLeft');
    await composer.press('ArrowLeft');
    await expect.poll(async () => (await caretFacts(composer)).covered).toBe('l');
    facts = await caretFacts(composer);
    expect(Math.abs(facts.dx!), 'x after two ArrowLeft').toBeLessThan(1.5);

    await composer.fill('ls -la\ncat notes.txt');
    await expect.poll(async () => (await caretFacts(composer)).covered).toBe('');
    facts = await caretFacts(composer);
    expect(Math.abs(facts.dx!), 'x on the second line').toBeLessThan(1.5);
    expect(Math.abs(facts.dy!), 'y on the second line').toBeLessThan(2);

    // Enough lines to scroll the capped composer. A scripted fill does not
    // scroll to the caret; the next keystroke does, as it does for a user.
    await composer.fill(Array.from({ length: 40 }, (_, i) => `line ${i}`).join('\n'));
    await composer.pressSequentially('!');
    await expect.poll(() => composer.evaluate(el => el.scrollTop)).toBeGreaterThan(0);
    await expect.poll(async () => Math.abs((await caretFacts(composer)).dy ?? 99)).toBeLessThan(2);

    const dir = resolve(here, '..', 'test-results', 'theme-caret-fallback');
    mkdirSync(dir, { recursive: true });
    await composer.fill('ls -la');
    await composer.press('ArrowLeft');
    await composer.press('ArrowLeft');
    await setDevicePreference(page, 'motion', 'reduce');
    await expect(page.locator('html')).toHaveAttribute('data-motion', 'reduce');
    await expect.poll(async () => (await caretFacts(composer)).animation, 'reduced motion holds it still').toBe('none');
    await composer.locator('xpath=ancestor::div[contains(@class, "prompt-box")]')
      .screenshot({ path: resolve(dir, 'block-caret.png') });
  });

  test('WebKit hides the drawn caret on a range and on blur, and drops it on a theme switch', async ({ page, browserName }) => {
    test.skip(browserName !== 'webkit', 'WebKit has no caret-shape; Chromium is covered below');
    const composer = await composerWithTheme(page);
    await composer.click();
    await composer.pressSequentially('ls -la');
    await expect.poll(async () => (await caretFacts(composer)).shown).toBe(true);

    await composer.press('Shift+ArrowLeft');
    await expect.poll(async () => (await caretFacts(composer)).shown, 'a range hides it').toBe(false);
    await composer.press('ArrowRight');
    await expect.poll(async () => (await caretFacts(composer)).shown).toBe(true);

    await composer.evaluate(el => el.blur());
    await expect.poll(async () => (await caretFacts(composer)).shown, 'blur hides it').toBe(false);

    await composer.focus();
    await expect.poll(async () => (await caretFacts(composer)).shown).toBe(true);
    await setDevicePreference(page, 'theme', 'lucidos');
    await expect.poll(() => composer.evaluate(el => el.nextElementSibling?.classList.contains('drawn-caret') ?? false),
      'a theme with no caret-shape removes the overlay').toBe(false);
    expect((await caretFacts(composer)).nativeHidden).toBe(false);
  });

  test('Chromium keeps its native caret-shape and adds no overlay', async ({ page, browserName }) => {
    test.skip(browserName !== 'chromium', 'Chromium draws caret-shape natively');
    const composer = await composerWithTheme(page);
    await composer.click();
    await composer.pressSequentially('ls -la');
    await composer.press('ArrowLeft');
    expect(await composer.evaluate(el => getComputedStyle(el).getPropertyValue('caret-shape'))).toBe('block');
    expect(await page.locator('.drawn-caret').count()).toBe(0);
    expect(await composer.getAttribute('data-drawn-caret')).toBeNull();
  });
});
