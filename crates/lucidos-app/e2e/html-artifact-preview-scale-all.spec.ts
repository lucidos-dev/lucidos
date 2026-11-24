import { test, expect, type Page } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream, waitForModalExitFade } from './helpers';

const SAMPLE_NAME = 'e2e-scaled-report.html';
const SAMPLE_PATH = `artifacts/${SAMPLE_NAME}`;
const SAMPLE_FILE = resolve(WORKSPACE, 'data', SAMPLE_PATH);

const BOX_PX = 100;

const UI_SCALE_ROW = '[data-search-anchor="appearance:ui-scale"] .settings-option';

interface FrameProbe {
  boxWidth: number;
  fullWidth: number;
  viewportWidth: number;
  scrollWidth: number;
  proseFontSize: string;
}

async function probeFrame(page: Page): Promise<FrameProbe> {
  const element = page.locator('.file-preview-inline:visible iframe').first();
  await expect(element).toBeVisible({ timeout: 15_000 });
  await expect(page.frameLocator('.file-preview-inline:visible iframe').first().locator('#box'))
    .toBeAttached({ timeout: 15_000 });
  const frame = await (await element.elementHandle())?.contentFrame();
  if (!frame) throw new Error('the preview iframe has no frame');
  return frame.evaluate(() => {
    const doc = document;
    const width = (sel: string) => doc.querySelector(sel)!.getBoundingClientRect().width;
    return {
      boxWidth: width('#box'),
      fullWidth: width('#full'),
      viewportWidth: doc.documentElement.clientWidth,
      scrollWidth: doc.documentElement.scrollWidth,
      proseFontSize: getComputedStyle(doc.querySelector('#prose')!).fontSize,
    };
  });
}

async function navigateTo(page: Page, target: string, params: object): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target, params },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
}

async function openArtifact(page: Page): Promise<void> {
  await navigateTo(page, 'file', { file_path: SAMPLE_PATH });
}

async function setUiScale(page: Page, percent: number): Promise<void> {
  await navigateTo(page, 'settings', { settings_view: 'appearance' });

  const row = page.locator(UI_SCALE_ROW);
  await expect(row).toBeVisible({ timeout: 15_000 });
  await row.click();
  const slider = page.locator('.scale-modal-slider');
  await expect(slider).toBeVisible();

  await slider.click();
  await page.keyboard.press('End');
  for (let at = 200; at > percent; at -= 12.5) await page.keyboard.press('ArrowLeft');
  await expect(page.locator('.scale-modal-label')).toHaveText(`${percent}%`);
  await page.keyboard.press('Escape');
  await waitForModalExitFade(page);
}

test.describe('an HTML artifact preview', () => {
  test.beforeAll(() => {
    mkdirSync(resolve(WORKSPACE, 'data/artifacts'), { recursive: true });
    writeFileSync(SAMPLE_FILE, `<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"><title>scaled report</title></head>
<body style="margin:0">
<div id="box" style="width:${BOX_PX}px;height:20px;background:#333"></div>
<div id="full" style="width:100%;height:20px;background:#666"></div>
<p id="prose">Unsized text</p>
</body>
</html>
`);
  });

  test.afterAll(() => {
    rmSync(SAMPLE_FILE, { force: true });
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForEventStream(page);
  });

  test('is drawn untouched at the default scale', async ({ page }) => {
    await openArtifact(page);
    const probe = await probeFrame(page);

    expect(probe.boxWidth).toBeCloseTo(BOX_PX, 0);
    expect(probe.fullWidth).toBeCloseTo(probe.viewportWidth, 0);
  });

  test('gives unsized text the chat prose step', async ({ page }) => {
    await openArtifact(page);
    expect((await probeFrame(page)).proseFontSize).toBe('12px');
  });

  test('grows with the UI scale, without overflowing its frame', async ({ page }) => {
    await setUiScale(page, 150);
    await openArtifact(page);
    const probe = await probeFrame(page);

    expect(probe.boxWidth).toBeCloseTo(BOX_PX * 1.5, 0);
    expect(probe.fullWidth).toBeCloseTo(probe.viewportWidth, 0);
    expect(probe.scrollWidth).toBeLessThanOrEqual(probe.viewportWidth + 1);

    await setUiScale(page, 100);
    await openArtifact(page);
    expect((await probeFrame(page)).boxWidth).toBeCloseTo(BOX_PX, 0);
  });
});
