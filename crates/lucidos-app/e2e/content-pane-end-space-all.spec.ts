/**
 * Every content-pane view keeps the end space below its last element, on a
 * phone and on desktop (`docs/glossary.md` § End space). A view pinned to the
 * pane height let its content run past its own box. The last row then sat
 * flush on the screen's bottom edge, under the home indicator.
 *
 * The source scan `styles/__tests__/content-pane-end-space.test.ts` pins the
 * rule. This measures what renders.
 */
import { test, expect, type Page } from './fixtures';
import { mkdirSync, rmSync, writeFileSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import { apiRequest, assertHealthy, isMobileViewport, navigateToApp, openSettingsView, waitForEventStream } from './helpers';

const LONG_NOTE_DIR = 'artifacts/e2e-end-space';
const LONG_NOTE = `${LONG_NOTE_DIR}/long-note.md`;

interface EndSpace {
  /** The view's spacer, in px. */
  spacer: number;
  /** One rem, in px, so the spacer is checked against its token. */
  rem: number;
  /** How far the lowest content reaches past where the spacer starts. */
  contentPastSpacer: number;
  /** How far the spacer runs past the pane's scroll end, out of reach. */
  spacerPastPaneEnd: number;
  /** Whether the pane scrolls at all, so a test can prove it outgrew one screen. */
  overflows: boolean;
}

/** Measures the view the pane shows: its spacer, and where its content ends.
 *  Out-of-flow boxes are skipped, since a positioned overlay ends anywhere. */
async function measure(page: Page): Promise<EndSpace> {
  return page.locator('.content-pane-body:visible').first().evaluate((pane) => {
    const view = pane.firstElementChild!;
    const viewRect = view.getBoundingClientRect();
    const spacer = parseFloat(getComputedStyle(view, '::after').height) || 0;
    let lowest = viewRect.top;
    const walk = (el: Element) => {
      const style = getComputedStyle(el);
      if (style.position === 'absolute' || style.position === 'fixed' || style.display === 'none') return;
      const r = el.getBoundingClientRect();
      if (r.width > 0 && r.height > 0) lowest = Math.max(lowest, r.bottom);
      for (const child of Array.from(el.children)) walk(child);
    };
    for (const child of Array.from(view.children)) walk(child);
    const paneRect = pane.getBoundingClientRect();
    const paneEnd = paneRect.top - pane.scrollTop + pane.scrollHeight;
    return {
      spacer,
      rem: parseFloat(getComputedStyle(document.documentElement).fontSize),
      contentPastSpacer: lowest - (viewRect.bottom - spacer),
      spacerPastPaneEnd: viewRect.bottom - paneEnd,
      overflows: pane.scrollHeight > pane.clientHeight,
    };
  });
}

async function expectEndSpace(page: Page, ready: string): Promise<EndSpace> {
  await expect(page.locator(`.content-pane-body ${ready}:visible`).first()).toBeVisible({ timeout: 15_000 });
  let last: EndSpace | null = null;
  const holds = (m: EndSpace) => m.spacer >= 1.5 * m.rem - 0.5
    && m.contentPastSpacer <= 1
    && m.spacerPastPaneEnd <= 1;
  await expect.poll(async () => {
    last = await measure(page);
    return holds(last) ? 'holds' : JSON.stringify(last);
  }, { timeout: 5_000, message: 'the view ends without its end space' }).toBe('holds');
  return last!;
}

async function navigate(page: Page, target: string, params: Record<string, string> = {}): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target, params },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate ${target} -> ${res.status()}`).toBeTruthy();
}

test.describe('the content pane end space', () => {
  test.beforeAll(() => {
    mkdirSync(resolve(WORKSPACE, 'data', LONG_NOTE_DIR), { recursive: true });
    const paragraphs = Array.from({ length: 80 }, (_, i) => `Paragraph ${i}.`).join('\n\n');
    writeFileSync(resolve(WORKSPACE, 'data', LONG_NOTE), `# Long note\n\n${paragraphs}\n`);
  });

  test.afterAll(() => {
    rmSync(resolve(WORKSPACE, 'data', LONG_NOTE_DIR), { recursive: true, force: true });
  });

  test.beforeEach(async ({ page }) => {
    // Short enough that the long Settings pages outgrow one screen everywhere.
    const size = page.viewportSize()!;
    await page.setViewportSize({ width: size.width, height: 600 });
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForEventStream(page);
  });

  test('a long Settings page ends with room below its last row', async ({ page }) => {
    await openSettingsView(page, 'appearance');
    const shown = await expectEndSpace(page, '.theme-carousel[role="radiogroup"]');
    // Proves the page outgrew the pane, or the check above proves nothing.
    expect(shown.overflows, 'Appearance never outgrew the pane').toBe(true);
  });

  test('System overview ends with room below its last row', async ({ page }) => {
    await openSettingsView(page, 'system-overview');
    await expectEndSpace(page, '.system-page');
  });

  for (const [target, ready] of [
    ['apps', '.apps-view'],
    ['plugins', '.plugins-view'],
    ['triggers', '.content-view.active'],
    ['changes', '.panel-content'],
    ['notifications', '.panel-content'],
  ] as const) {
    test(`the ${target} panel ends with its end space`, async ({ page }) => {
      await navigate(page, target);
      await expectEndSpace(page, ready);
    });
  }

  // Full-bleed on desktop, where it scrolls inside itself. A phone flows it
  // in the pane like a column, so it takes the end space there.
  test('a long file preview on a phone ends with room below its last line', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'a desktop preview scrolls inside itself');
    await navigate(page, 'file', { file_path: LONG_NOTE });
    await expect(page.locator('.content-pane-body .markdown-content:visible').first())
      .toContainText('Paragraph 79', { timeout: 15_000 });
    const shown = await expectEndSpace(page, '.file-preview-frame');
    expect(shown.overflows, 'the note never outgrew the pane').toBe(true);
  });
});
