import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect, type Page } from './fixtures';
import { apiRequest, gotoWithRetry, shellDeviceId } from './helpers';
import { appPath, createIframeAppFixture } from './db-helpers';


const EVERY_CAP = 'e2e-parts-every-cap';
const TEXT_GLOW = 'e2e-text-glow-only';
const SCREEN_ONLY = 'e2e-parts-screen-only';
const here = dirname(fileURLToPath(import.meta.url));
const fixture = (name: string): string => readFileSync(resolve(here, 'themes', name), 'utf-8');

const CHUNK = '#lp-chunk';
const CARDS = ['#lp-question', '#lp-permission', '#lp-form', '#lp-split'];
const CARD_PARTS = ['.lp-text', 'h2', 'a', 'code', 'pre'];
const PAINT = ['color', 'caret-color', 'text-shadow', 'letter-spacing', 'box-shadow', 'filter', 'background-color'];

const CARD_BODY = '<span class="lp-text">Allow this?</span><h2>Heading</h2><a href="#">link</a><code>code</code><pre>block</pre>';

const TRANSCRIPT = `
<div class="response-body"><div class="response-chunk" id="lp-chunk">
  Glowing prose <a href="#">a link</a>
  <div class="question-body protected-surface" id="lp-question">${CARD_BODY}</div>
  <div class="permission-body protected-surface" id="lp-permission">${CARD_BODY}</div>
  <pre>code block <div class="split-button protected-surface" id="lp-split">${CARD_BODY}</div></pre>
</div></div>
<span class="initiator-icon"><svg id="lp-icon" width="16" height="16"><rect width="16" height="16"/></svg></span>
<div class="prompt-area"><div class="prompt-box" id="lp-box"><div class="prompt-row">
  <textarea class="prompt-textarea" id="lp-textarea"></textarea>
</div>
<div class="inline-form protected-surface" id="lp-form">${CARD_BODY}</div>
</div></div>
<span class="pane-header-title-text" id="lp-title">Title</span>
<span class="thread-row-title" id="lp-drawer-title">Drawer title</span>`;

async function setPreference(page: Page, key: string, value: string): Promise<void> {
  const device = await shellDeviceId(page);
  const res = await apiRequest(page).put(`/api/v1/preferences?key=${key}`, { data: { value, device_id: device } });
  expect(res.ok(), `${key}=${value}`).toBe(true);
  expect((await res.json()).success, `${key}=${value}`).toBe(true);
}

async function resetPreferences(page: Page): Promise<void> {
  await setPreference(page, 'theme', 'lucidos');
  await setPreference(page, 'theme-effects', 'system');
}

async function injectTranscript(page: Page): Promise<void> {
  await page.evaluate((html) => {
    document.querySelector('#lp-root')?.remove();
    const root = document.createElement('div');
    root.id = 'lp-root';
    root.innerHTML = html;
    document.body.appendChild(root);
  }, TRANSCRIPT);
}

async function style(page: Page, selector: string, prop: string): Promise<string> {
  return page.locator(selector).first().evaluate(
    (el, p) => getComputedStyle(el).getPropertyValue(p), prop,
  );
}

async function cardPaint(page: Page): Promise<Record<string, string>> {
  const out: Record<string, string> = {};
  for (const card of CARDS) {
    for (const part of CARD_PARTS) {
      for (const prop of PAINT) out[`${card} ${part} ${prop}`] = await style(page, `${card} ${part}`, prop);
    }
  }
  return out;
}

function layers(value: string): number {
  if (value === 'none' || value === '') return 0;
  let depth = 0;
  let count = 1;
  for (const c of value) {
    if (c === '(') depth++;
    else if (c === ')') depth--;
    else if (c === ',' && depth === 0) count++;
  }
  return count;
}

async function waitForToken(page: Page, token: string, present: boolean): Promise<void> {
  await expect.poll(() => page.evaluate(
    (t) => document.documentElement.style.getPropertyValue(t), token,
  )).toEqual(present ? expect.stringMatching(/\S/) : '');
}

test.describe('theme parts', () => {
  test.beforeEach(async ({ page }) => {
    await gotoWithRetry(page, '/');
    const api = apiRequest(page);
    for (const [id, file] of [[EVERY_CAP, 'parts-every-cap.json'], [TEXT_GLOW, 'text-glow-only.json']]) {
      const res = await api.put(`/api/v1/data/themes/${id}.json`, { data: fixture(file) });
      expect(res.ok(), `saving ${id}: ${await res.text()}`).toBe(true);
    }
  });

  test.afterEach(async ({ page }) => {
    await resetPreferences(page);
    const api = apiRequest(page);
    for (const id of [EVERY_CAP, TEXT_GLOW, SCREEN_ONLY]) await api.delete(`/api/v1/data/themes/${id}.json`);
  });

  test('a glow on chat text never reaches a protected card nested inside it', async ({ page }) => {
    await injectTranscript(page);
    const everyCap = JSON.parse(fixture('parts-every-cap.json'));
    const screenOnly = {
      name: 'Screen only',
      parts: { screen: everyCap.parts.screen },
      light: { parts: { screen: everyCap.light.parts.screen } },
    };
    const saved = await apiRequest(page).put(`/api/v1/data/themes/${SCREEN_ONLY}.json`, { data: JSON.stringify(screenOnly) });
    expect(saved.ok(), await saved.text()).toBe(true);
    await setPreference(page, 'theme', SCREEN_ONLY);
    await waitForToken(page, '--part-screen-background-image', true);
    const plain = await cardPaint(page);

    await setPreference(page, 'theme', EVERY_CAP);
    await waitForToken(page, '--part-chat-text-text-shadow', true);

    expect(layers(await style(page, CHUNK, 'text-shadow'))).toBe(1);
    expect(await style(page, CHUNK, 'color')).toBe('rgb(204, 51, 102)');
    expect(await style(page, CHUNK, 'letter-spacing')).not.toBe('normal');
    expect(await cardPaint(page)).toEqual(plain);
    for (const card of CARDS) {
      expect(await style(page, `${card} .lp-text`, 'text-shadow')).toBe('none');
      expect(await style(page, `${card} .lp-text`, 'letter-spacing')).toBe('normal');
    }
  });

  test('reduce drops part shadows and filters and keeps colours and letter-spacing', async ({ page }) => {
    await injectTranscript(page);
    await setPreference(page, 'theme', EVERY_CAP);
    await waitForToken(page, '--part-chat-text-text-shadow', true);
    await setPreference(page, 'theme-effects', 'full');
    await expect(page.locator('html')).toHaveAttribute('data-theme-effects', 'full');

    expect(await style(page, CHUNK, 'text-shadow')).not.toBe('none');
    expect(await style(page, '#lp-icon', 'filter')).not.toBe('none');
    expect(await style(page, '#lp-box', 'box-shadow')).not.toBe('none');
    const spacing = await style(page, CHUNK, 'letter-spacing');

    await setPreference(page, 'theme-effects', 'reduce');
    await expect(page.locator('html')).toHaveAttribute('data-theme-effects', 'reduce');
    for (const [selector, prop] of [
      [CHUNK, 'text-shadow'], ['#lp-icon', 'filter'], ['#lp-box', 'box-shadow'],
      ['#lp-textarea', 'text-shadow'], ['#lp-title', 'text-shadow'],
    ]) {
      await expect.poll(() => style(page, selector, prop), { message: `${selector} ${prop}` }).toBe('none');
    }
    expect(await style(page, CHUNK, 'color')).toBe('rgb(204, 51, 102)');
    expect(await style(page, CHUNK, 'letter-spacing')).toBe(spacing);
    await expect.poll(() => style(page, '#lp-box', 'border-color')).toBe('rgb(51, 255, 51)');

    await setPreference(page, 'theme-effects', 'full');
    await expect.poll(() => style(page, CHUNK, 'text-shadow')).not.toBe('none');
  });

  test('a colour token holding smuggled text adds no shadow layer', async ({ page }) => {
    await injectTranscript(page);
    await setPreference(page, 'theme', EVERY_CAP);
    await waitForToken(page, '--part-chat-text-text-shadow', true);
    const capped = await style(page, CHUNK, 'text-shadow');
    expect(layers(capped)).toBe(1);

    for (const smuggle of [
      { '--accent': 'red, 9em 9em 5em red' },
      { '--lp-evil': 'red, 9em 9em 5em red', '--accent': 'var(--lp-evil)' },
    ]) {
      await page.evaluate((map) => {
        for (const [name, value] of Object.entries(map)) document.documentElement.style.setProperty(name, value);
      }, smuggle);
      const shadow = await style(page, CHUNK, 'text-shadow');
      expect(layers(shadow), shadow).toBe(1);
      expect(shadow).not.toMatch(/\b1\d\dpx|\b[2-9]\d\dpx/);
      await page.evaluate(() => {
        document.documentElement.style.removeProperty('--lp-evil');
        document.documentElement.style.removeProperty('--accent');
      });
    }
  });

  test('a theme that sets only --text-glow still glows, and reduce drops it', async ({ page }) => {
    await injectTranscript(page);
    await setPreference(page, 'theme', TEXT_GLOW);
    await waitForToken(page, '--part-chat-text-text-shadow', true);
    await setPreference(page, 'theme-effects', 'full');
    await expect(page.locator('html')).toHaveAttribute('data-theme-effects', 'full');
    for (const selector of [CHUNK, '#lp-title', '#lp-textarea']) {
      expect(layers(await style(page, selector, 'text-shadow')), selector).toBe(1);
    }
    for (const card of CARDS) expect(await style(page, `${card} .lp-text`, 'text-shadow')).toBe('none');

    await setPreference(page, 'theme-effects', 'reduce');
    await expect(page.locator('html')).toHaveAttribute('data-theme-effects', 'reduce');
    expect(await style(page, CHUNK, 'text-shadow')).toBe('none');
  });

  test('the default theme sets no part token and paints no part', async ({ page }) => {
    await injectTranscript(page);
    await setPreference(page, 'theme', EVERY_CAP);
    await waitForToken(page, '--part-chat-text-text-shadow', true);
    await setPreference(page, 'theme', 'lucidos');
    await waitForToken(page, '--part-chat-text-text-shadow', false);
    expect(await style(page, CHUNK, 'text-shadow')).toBe('none');
    expect(await style(page, CHUNK, 'letter-spacing')).toBe('normal');
    expect(await style(page, '#lp-icon', 'filter')).toBe('none');
  });

  test('a theme sets the drawer title weight, the default stays 400, and reduce keeps it', async ({ page }) => {
    await injectTranscript(page);
    expect(await style(page, '#lp-drawer-title', 'font-weight')).toBe('400');

    await setPreference(page, 'theme', EVERY_CAP);
    await waitForToken(page, '--part-drawer-title-font-weight', true);
    await expect.poll(() => style(page, '#lp-drawer-title', 'font-weight')).toBe('600');

    await setPreference(page, 'theme-effects', 'reduce');
    await expect(page.locator('html')).toHaveAttribute('data-theme-effects', 'reduce');
    expect(await style(page, '#lp-drawer-title', 'font-weight')).toBe('600');

    await setPreference(page, 'theme', 'lucidos');
    await waitForToken(page, '--part-drawer-title-font-weight', false);
    await expect.poll(() => style(page, '#lp-drawer-title', 'font-weight')).toBe('400');
  });
});

const PLAIN_APP = 'e2e-theme-parts-plain';
const SDK_APP = 'e2e-theme-parts-sdk';
const OPTED_OUT_APP = 'e2e-theme-parts-off';

function appHtml(opts: { sdk: boolean; optOut?: boolean }): string {
  const tags = opts.sdk
    ? '<script src="/api/v1/sdk-prefs.js"></script><link rel="stylesheet" href="/api/v1/sdk-iframe.css">'
    : '';
  return `<!DOCTYPE html><html${opts.optOut ? ' data-theme-parts="off"' : ''}><head><meta charset="UTF-8">${tags}</head>
<body><p id="text">App text</p><a id="link" href="#">link</a><button id="control">Go</button></body></html>`;
}

test.describe('theme parts in app frames', () => {
  const fixtures: { cleanup: () => void }[] = [];

  test.beforeAll(() => {
    fixtures.push(createIframeAppFixture(PLAIN_APP, { html: appHtml({ sdk: false }), js: '' }));
    fixtures.push(createIframeAppFixture(SDK_APP, { html: appHtml({ sdk: true }), js: '' }));
    fixtures.push(createIframeAppFixture(OPTED_OUT_APP, { html: appHtml({ sdk: true, optOut: true }), js: '' }));
  });

  test.afterAll(() => {
    for (const f of fixtures) f.cleanup();
  });

  test('an SDK app paints every frame part, an opted-out one and a plain one paint none', async ({ page }) => {
    await gotoWithRetry(page, '/');
    const res = await apiRequest(page).put(`/api/v1/data/themes/${EVERY_CAP}.json`, { data: fixture('parts-every-cap.json') });
    expect(res.ok()).toBe(true);
    await setPreference(page, 'theme', EVERY_CAP);
    await setPreference(page, 'theme-effects', 'full');
    const device = await shellDeviceId(page);

    try {
      await page.setContent(`<!DOCTYPE html><html><body>
${[PLAIN_APP, SDK_APP, OPTED_OUT_APP].map(id => `<iframe id="${id}" src="${appPath(id)}?device=${device}"
  sandbox="allow-scripts allow-same-origin" style="width:300px;height:200px;border:0"></iframe>`).join('\n')}
</body></html>`);

      const frame = (id: string) => page.frameLocator(`#${id}`);
      const frameStyle = (id: string, selector: string, prop: string) => frame(id).locator(selector)
        .evaluate((el, p) => getComputedStyle(el).getPropertyValue(p), prop);
      await expect(frame(SDK_APP).locator('#text')).toBeVisible();

      expect(layers(await frameStyle(SDK_APP, 'body', 'text-shadow'))).toBe(1);
      expect(await frameStyle(SDK_APP, 'body', 'letter-spacing')).not.toBe('normal');
      expect(await frameStyle(SDK_APP, '#control', 'border-top-color')).toBe('rgb(51, 255, 51)');
      expect(await frameStyle(SDK_APP, '#control', 'box-shadow')).not.toBe('none');

      for (const [selector, prop] of [['body', 'text-shadow'], ['#link', 'text-shadow'], ['#control', 'box-shadow']]) {
        expect(await frameStyle(OPTED_OUT_APP, selector, prop), `${selector} ${prop}`).toBe('none');
      }
      expect(await frameStyle(OPTED_OUT_APP, 'body', 'letter-spacing')).toBe('normal');

      const inline = await frame(PLAIN_APP).locator('html').evaluate(el => (el as HTMLElement).style.cssText);
      expect(inline).not.toContain('--part-');
      expect(await frameStyle(PLAIN_APP, 'body', 'text-shadow')).toBe('none');
    } finally {
      await gotoWithRetry(page, '/');
      await resetPreferences(page);
      await apiRequest(page).delete(`/api/v1/data/themes/${EVERY_CAP}.json`);
    }
  });
});
