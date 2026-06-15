import { test, expect } from '@playwright/test';
import { appPath, createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';

const APP_ID = 'e2e-sdk-select-test';
let fixture: { cleanup: () => void };

test.describe('SDK iframe — lucidos.ui.Select', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>SDK select test</title>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
</head>
<body>
<div id="programmatic"></div>
<form id="enhance-form">
  <select class="lucidos-select" id="native-select" data-placeholder="Pick a status…">
    <option value="todo">To do</option>
    <option value="doing">In progress</option>
    <option value="done">Done</option>
  </select>
</form>
<div id="status">init</div>
<div id="last-change"></div>
<div id="native-change"></div>
<script src="script.js"></script>
</body>
</html>
`,
      js: `
function start() {
  if (!window.lucidos || !window.lucidos.ui || !window.lucidos.ui.Select) {
    setTimeout(start, 50);
    return;
  }
  var sel = lucidos.ui.Select.create({
    options: [
      { value: 'apple', label: 'Apple' },
      { value: 'banana', label: 'Banana' },
      { value: 'cherry', label: 'Cherry' },
    ],
    value: 'apple',
    onChange: function (v) {
      document.getElementById('last-change').textContent = v;
    },
  });
  document.getElementById('programmatic').appendChild(sel.element);
  window.__sel = sel;

  lucidos.ui.enhanceSelects();
  document.getElementById('native-select').addEventListener('change', function (e) {
    document.getElementById('native-change').textContent = e.target.value;
  });

  document.getElementById('status').textContent = 'ready';
}
start();
`,
    });
  });

  test.afterAll(() => {
    fixture.cleanup();
  });

  test('programmatic create + click selection + keyboard nav', async ({ page }) => {
    await gotoWithRetry(page, appPath(APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    const root = page.locator('#programmatic .lucidos-select');
    const trigger = root.locator('.lucidos-select-trigger');

    await expect(trigger).toBeVisible();
    await expect(trigger.locator('.lucidos-select-label')).toHaveText('Apple');
    await expect(root).toHaveAttribute('data-state', 'closed');

    await trigger.click();
    await expect(root).toHaveAttribute('data-state', 'open');
    await expect(root.locator('.lucidos-select-menu')).toBeVisible();

    await root.locator('.lucidos-select-option', { hasText: 'Banana' }).click();
    await expect(root).toHaveAttribute('data-state', 'closed');
    await expect(trigger.locator('.lucidos-select-label')).toHaveText('Banana');
    await expect(page.locator('#last-change')).toHaveText('banana');

    await trigger.focus();
    await page.keyboard.press('ArrowDown');
    await expect(root).toHaveAttribute('data-state', 'open');
    await page.keyboard.press('ArrowDown'); // banana → cherry
    await page.keyboard.press('Enter');
    await expect(root).toHaveAttribute('data-state', 'closed');
    await expect(trigger.locator('.lucidos-select-label')).toHaveText('Cherry');
    await expect(page.locator('#last-change')).toHaveText('cherry');

    await trigger.focus();
    await page.keyboard.press('ArrowDown');
    await expect(root).toHaveAttribute('data-state', 'open');
    await page.keyboard.press('Escape');
    await expect(root).toHaveAttribute('data-state', 'closed');
    await expect(trigger.locator('.lucidos-select-label')).toHaveText('Cherry');
  });

  test('enhanceSelects mirrors value back to native <select>', async ({ page }) => {
    await gotoWithRetry(page, appPath(APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    const native = page.locator('#native-select');
    await expect(native).toHaveCSS('display', 'none');
    await expect(native).toHaveAttribute('aria-hidden', 'true');

    const themed = page.locator('#enhance-form .lucidos-select');
    await expect(themed.locator('.lucidos-select-trigger .lucidos-select-label')).toHaveText('To do');

    await themed.locator('.lucidos-select-trigger').click();
    await themed.locator('.lucidos-select-option', { hasText: 'Done' }).click();

    await expect(themed.locator('.lucidos-select-trigger .lucidos-select-label')).toHaveText('Done');
    await expect(native).toHaveValue('done');
    await expect(page.locator('#native-change')).toHaveText('done');
  });

  test('outside click closes the menu', async ({ page }) => {
    await gotoWithRetry(page, appPath(APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    const root = page.locator('#programmatic .lucidos-select');
    await root.locator('.lucidos-select-trigger').click();
    await expect(root).toHaveAttribute('data-state', 'open');

    await page.locator('#status').click();
    await expect(root).toHaveAttribute('data-state', 'closed');
  });
});
