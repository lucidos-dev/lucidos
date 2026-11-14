import { test, expect } from './fixtures';
import { appPath, createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';

const APP_ID = 'e2e-sdk-model-picker';
let fixture: { cleanup: () => void };
test.beforeAll(() => {
  fixture = createIframeAppFixture(APP_ID, {
    manifest: { id: APP_ID, name: 'SDK model picker', description: 'Model picker integration fixture' },
    html: `<!doctype html><html><head><meta charset="UTF-8"><script src="/api/v1/sdk.js"></script></head>
<body><div class="field"></div><button class="after">After picker</button><output class="result"></output><script src="script.js"></script></body></html>`,
    js: `(async () => {
      const { models } = await lucidos.request('/models');
      const model = models.find(row => !row.successor && row.routes.some(route => route.proxy));
      if (!model) throw new Error('Missing seeded model proxy');
      const picker = lucidos.ui.ModelPicker.create({
        value: { model: model.id, reasoning_effort: null, provider: null },
        filter: { callableOnly: true },
        onChange: value => { document.querySelector('.result').textContent = JSON.stringify(value); },
      });
      document.querySelector('.field').append(picker.element);
      await picker.ready;
      document.body.dataset.ready = 'true';
    })().catch(error => { document.querySelector('.result').textContent = error.message; });`,
  });
});
test.afterAll(() => fixture.cleanup());

for (const width of [1000, 360]) {
  test(`shared picker remains inside its ${width}px iframe and owns keyboard focus`, async ({ page }) => {
    await gotoWithRetry(page, '/');
    await page.setContent(`<iframe class="picker-frame" src="${appPath(APP_ID)}"
      sandbox="allow-scripts allow-same-origin allow-popups allow-forms allow-modals allow-popups-to-escape-sandbox allow-downloads"
      style="width:${width}px;height:600px;border:0"></iframe>`);
    const frame = page.frameLocator('.picker-frame');
    await expect(frame.locator('body')).toHaveAttribute('data-ready', 'true');
    const trigger = frame.getByRole('button', { name: 'Model', exact: true });
    await trigger.focus();
    await trigger.press('ArrowDown');
    const menu = frame.locator('.dropdown-menu');
    await expect(menu).toBeVisible();
    const bounds = await menu.boundingBox();
    const frameBounds = await page.locator('.picker-frame').boundingBox();
    expect(bounds!.x).toBeGreaterThanOrEqual(frameBounds!.x);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(frameBounds!.x + width);
    if (width === 1000) {
      await expect(frame.locator('.control-list')).toBeFocused();
      await page.keyboard.press('Tab');
      await expect(menu).toHaveCount(0);
      await expect(frame.locator('.after')).toBeFocused();
      await trigger.click();
    }
    await page.keyboard.press('Escape');
    await expect(menu).toHaveCount(0);
    await expect(trigger).toBeFocused();
    await expect(frame.locator('.result')).toBeEmpty();
  });
}
