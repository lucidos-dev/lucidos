/**
 * An HTML artifact preview is drawn at the user's UI scale.
 *
 * The preview is an `<iframe srcDoc>`, which is its own realm and inherits no
 * root font-size from the shell. So at 125% every surface around it grew and the
 * artifact did not, which is the report this fixes. The preview stamps `zoom`
 * into the artifact's own head instead (ADR 0217).
 *
 * Measured on the RENDERED box, not on the stamped declaration. The declaration
 * is what the unit tests pin, and it is not what was wrong. It is also why this
 * runs on every project: the mechanism was chosen over the iframe-element one
 * because Chromium and WebKit disagree about that one, and `mobile-webkit` is
 * the only WebKit here.
 */
import { test, expect, type Page } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';

const SAMPLE_NAME = 'e2e-scaled-report.html';
const SAMPLE_PATH = `artifacts/${SAMPLE_NAME}`;
const SAMPLE_FILE = resolve(WORKSPACE, 'data', SAMPLE_PATH);

/** The box whose width is the whole assertion. Authored in px, the way a report
 *  actually is, so it moves only if something scales it. */
const BOX_PX = 100;

const UI_SCALE_ROW = '[data-search-anchor="appearance:ui-scale"] .settings-option';

/** What the previewed document rendered at. */
interface FrameProbe {
  /** The px-sized box, in the artifact's own client coordinates, which carry the
   *  zoom in both engines. */
  boxWidth: number;
  /** A `width: 100%` box. It must still fit, or the reader gets a horizontal
   *  scrollbar over an ordinary report. */
  fullWidth: number;
  viewportWidth: number;
  scrollWidth: number;
}

async function probeFrame(page: Page): Promise<FrameProbe> {
  const frame = page.locator('.file-preview-inline:visible iframe').first();
  await expect(frame).toBeVisible({ timeout: 15_000 });
  // The srcdoc parses on its own schedule, so poll for the body rather than
  // racing it with a bare evaluate.
  await expect
    .poll(() => frame.evaluate((el: HTMLIFrameElement) => !!el.contentDocument?.querySelector('#box')),
      { timeout: 15_000 })
    .toBe(true);
  return frame.evaluate((el: HTMLIFrameElement) => {
    const doc = el.contentDocument!;
    const width = (sel: string) => doc.querySelector(sel)!.getBoundingClientRect().width;
    return {
      boxWidth: width('#box'),
      fullWidth: width('#full'),
      viewportWidth: doc.documentElement.clientWidth,
      scrollWidth: doc.documentElement.scrollWidth,
    };
  });
}

/** Ask the page to open something, the way an agent or an app does. Used for
 *  both destinations here, so neither depends on the drawer being open: a visit
 *  to Settings leaves it closed, which is what made the panel route flaky. */
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

/** Put the device on `percent`, through the control a user would use.
 *
 *  Never reload after this. The write is server-held, so a reload races the PUT
 *  and can come back at the old scale. Nothing here tests that it persists. */
async function setUiScale(page: Page, percent: number): Promise<void> {
  await navigateTo(page, 'settings', { settings_view: 'appearance' });

  const row = page.locator(UI_SCALE_ROW);
  await expect(row).toBeVisible({ timeout: 15_000 });
  await row.click();
  const slider = page.locator('.scale-modal-slider');
  await expect(slider).toBeVisible();

  // Click to focus the row, then walk to the end and step back. Arrow keys move
  // one 12.5% step each, so the walk is exact where a pointer fraction is not.
  await slider.click();
  await page.keyboard.press('End');
  for (let at = 200; at > percent; at -= 12.5) await page.keyboard.press('ArrowLeft');
  await expect(page.locator('.scale-modal-label')).toHaveText(`${percent}%`);
  await page.keyboard.press('Escape');
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
    // Every navigate below is delivered over SSE, so the stream has to be up
    // before the first one is emitted.
    await waitForEventStream(page);
  });

  test('is drawn untouched at the default scale', async ({ page }) => {
    await openArtifact(page);
    const probe = await probeFrame(page);

    expect(probe.boxWidth).toBeCloseTo(BOX_PX, 0);
    // The stamp is skipped entirely at 100%, so this is also the assertion that
    // an unscaled artifact is the bytes on disk and nothing else.
    expect(probe.fullWidth).toBeCloseTo(probe.viewportWidth, 0);
  });

  test('grows with the UI scale, without overflowing its frame', async ({ page }) => {
    await setUiScale(page, 150);
    await openArtifact(page);
    const probe = await probeFrame(page);

    expect(probe.boxWidth).toBeCloseTo(BOX_PX * 1.5, 0);
    // A full-width element still fits: the zoom must not push the document
    // sideways under the reader.
    expect(probe.fullWidth).toBeCloseTo(probe.viewportWidth, 0);
    expect(probe.scrollWidth).toBeLessThanOrEqual(probe.viewportWidth + 1);

    // And back, so the scale is a live preference rather than a one-way trip.
    await setUiScale(page, 100);
    await openArtifact(page);
    expect((await probeFrame(page)).boxWidth).toBeCloseTo(BOX_PX, 0);
  });
});
