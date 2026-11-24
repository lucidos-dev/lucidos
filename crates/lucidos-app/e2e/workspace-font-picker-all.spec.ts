import { test, expect, type Locator, type Page } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';

const LONG_NAME = `${'VeryLongBrandFamilyName'.repeat(6)}-Regular.woff2`;

async function right(locator: Locator): Promise<number> {
  const box = await locator.boundingBox();
  expect(box, 'element has a layout box').not.toBeNull();
  return box!.x + box!.width;
}

async function openInstallForm(page: Page): Promise<Locator> {
  await assertHealthy(page);
  await navigateToApp(page);
  await waitForEventStream(page);
  const nav = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'settings', params: { settings_view: 'appearance' } },
  });
  expect(nav.ok(), `POST /api/v1/ui/navigate -> ${nav.status()}`).toBeTruthy();
  await page.getByRole('button', { name: 'Install font' }).click();
  const form = page.locator('.workspace-font-form');
  await expect(form).toBeVisible();
  return form;
}

test.describe('the font file picker', () => {
  test('sits flush with the column, holds a long name, and opens from the keyboard', async ({ page, browserName }) => {
    const form = await openInstallForm(page);
    const picker = form.locator('.workspace-font-picker');
    const button = picker.locator('label.settings-option', { hasText: 'Choose files' });
    const status = picker.locator('.workspace-font-picker-status');
    const input = picker.locator('input[type="file"]');

    await expect(status).toHaveText('No file chosen');
    await expect(input).toHaveAttribute('accept', '.woff2,.woff,.ttf,.otf');
    await expect(input).toHaveAttribute('multiple', '');
    await expect(page.getByLabel('Choose font files')).toHaveCount(1);

    const statusBox = (await status.boundingBox())!;
    const buttonBox = (await button.boundingBox())!;
    expect(statusBox.x + statusBox.width).toBeLessThanOrEqual(buttonBox.x);

    const groupTrigger = form.locator('.settings-row', { hasText: 'Group' }).locator('.dropdown-trigger');
    const scaleButton = page.locator('.settings-row', { hasText: 'UI scale' }).locator('.settings-option');
    const edge = await right(button);
    expect(Math.abs(edge - (await right(groupTrigger)))).toBeLessThan(0.5);
    expect(Math.abs(edge - (await right(scaleButton)))).toBeLessThan(0.5);

    const installBox = (await form.getByRole('button', { name: 'Install', exact: true }).boundingBox())!;
    expect(Math.abs(buttonBox.height - installBox.height)).toBeLessThan(0.5);

    if (browserName === 'webkit') {
      await input.focus();
    } else {
      await form.locator('input[placeholder="Brand Sans"]').focus();
      await page.keyboard.press('Shift+Tab');
      await expect(button).not.toHaveCSS('box-shadow', 'none');
    }
    await expect(input).toBeFocused();
    const chooser = page.waitForEvent('filechooser');
    await page.keyboard.press('Enter');
    expect((await chooser).isMultiple()).toBe(true);

    await input.setInputFiles({ name: LONG_NAME, mimeType: 'font/woff2', buffer: Buffer.from('x') });
    await expect(status).toHaveText(LONG_NAME);
    const truncated = await status.evaluate(el => ({
      clipped: el.scrollWidth > el.clientWidth,
      overflow: getComputedStyle(el).textOverflow,
    }));
    expect(truncated).toEqual({ clipped: true, overflow: 'ellipsis' });
    expect(Math.abs((await right(button)) - edge)).toBeLessThan(0.5);
    const pane = page.locator('.content-pane-body').first();
    expect(await pane.evaluate(el => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(0);

    await input.setInputFiles([
      { name: 'Brand-Regular.woff2', mimeType: 'font/woff2', buffer: Buffer.from('x') },
      { name: 'Brand-Bold.woff2', mimeType: 'font/woff2', buffer: Buffer.from('x') },
      { name: 'Brand-Italic.woff2', mimeType: 'font/woff2', buffer: Buffer.from('x') },
    ]);
    await expect(status).toHaveText('3 files');
  });
});
