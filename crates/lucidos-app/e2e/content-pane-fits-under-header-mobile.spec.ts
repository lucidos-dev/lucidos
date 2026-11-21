import { test, expect, type Page } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import { apiRequest, assertHealthy, navigateToApp, openSettingsView, waitForEventStream } from './helpers';

/** A short content view fits under the phone header, so it does not scroll.
 *
 *  The pane reserves the header's room with a spacer above the view. A view
 *  that also fills the whole pane overflows it by exactly that room. The
 *  reader can then drag the view's top up behind the header. */

const ROOT = 'artifacts/e2e-pane-fit';
const SHORT_NOTE = `${ROOT}/short-note.md`;
const LONG_NOTE = `${ROOT}/long-note.md`;

async function paneOverflow(page: Page, readySelector: string): Promise<number> {
  const ready = page.locator(`.content-pane-body ${readySelector}:visible`).first();
  await expect(ready).toBeVisible({ timeout: 15_000 });
  return ready.evaluate((el) => {
    const pane = el.closest('.content-pane-body') as HTMLElement;
    return pane.scrollHeight - pane.clientHeight;
  });
}

test.describe('a short content view on a phone', () => {
  test.beforeAll(() => {
    mkdirSync(resolve(WORKSPACE, 'data', ROOT), { recursive: true });
    writeFileSync(resolve(WORKSPACE, 'data', SHORT_NOTE), '# Short note\n\nOne line.\n');
    const paragraphs = Array.from({ length: 80 }, (_, i) => `Paragraph ${i}.`).join('\n\n');
    writeFileSync(resolve(WORKSPACE, 'data', LONG_NOTE), `# Long note\n\n${paragraphs}\n`);
  });

  test.afterAll(() => {
    rmSync(resolve(WORKSPACE, 'data', ROOT), { recursive: true, force: true });
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForEventStream(page);
  });

  test('a Settings page has nothing to scroll', async ({ page }) => {
    await openSettingsView(page, 'system');
    await expect.poll(() => paneOverflow(page, '.settings-panel'), { timeout: 5_000 }).toBeLessThanOrEqual(1);
  });

  test('the Thread Queue has nothing to scroll', async ({ page }) => {
    await openSettingsView(page, 'thread-queue');
    await expect.poll(() => paneOverflow(page, '.list-section-title'), { timeout: 5_000 }).toBeLessThanOrEqual(1);
  });

  test('a short file preview has nothing to scroll', async ({ page }) => {
    await openFile(page, SHORT_NOTE);
    await expect.poll(() => paneOverflow(page, '.file-preview-content .markdown-content'), { timeout: 5_000 }).toBeLessThanOrEqual(1);
  });

  // The pane is what the header's hide-on-scroll follows, so a long document
  // must scroll the pane rather than a box inside it.
  test('a long file preview scrolls the pane itself', async ({ page }) => {
    await openFile(page, LONG_NOTE);
    const doc = page.locator('.content-pane-body .file-preview-content .markdown-content:visible').first();
    await expect(doc).toContainText('Paragraph 79', { timeout: 15_000 });
    // Polled, since the document settles as it fades in over its skeleton.
    await expect.poll(() => doc.evaluate((el) => {
      const pane = el.closest('.content-pane-body') as HTMLElement;
      const inner = el.closest('.file-preview-content') as HTMLElement;
      return pane.scrollHeight - pane.clientHeight > 500 && inner.scrollHeight - inner.clientHeight <= 1;
    }), { timeout: 5_000 }).toBe(true);
  });
});

async function openFile(page: Page, path: string): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'file', params: { file_path: path } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
}
