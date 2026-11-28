import type { Page } from '@playwright/test';
import { test, expect } from './fixtures';
import {
  apiRequest, assertHealthy, ensureOnThreadPane, navigateToApp, sendMessage, uniqueMessage,
  waitForEventStream, waitForResponse,
} from './helpers';

const SWITCH = '/api/v1/preferences?key=terminal_enabled';
const VIEW = '.terminal-view:visible';
const SCREEN = `${VIEW} .terminal-screen`;

function key(page: Page, name: string) {
  return page.locator(`${VIEW} .terminal-key-row`).getByRole('button', { name, exact: true });
}

async function typeLine(page: Page, line: string): Promise<void> {
  await page.keyboard.type(line);
  await page.keyboard.press('Enter');
}

async function shellSize(page: Page, tag: string): Promise<{ rows: number; cols: number }> {
  await typeLine(page, `echo "${tag}:$(stty size)"`);
  const pattern = new RegExp(`${tag}:(\\d+) (\\d+)`);
  let match: RegExpMatchArray | null = null;
  await expect.poll(async () => (match = (await page.locator(SCREEN).innerText()).match(pattern)), {
    message: `the shell printed ${tag}`, timeout: 10_000,
  }).not.toBeNull();
  const [, rows, cols] = match as unknown as RegExpMatchArray;
  return { rows: Number(rows), cols: Number(cols) };
}

async function paneFit(page: Page): Promise<{ rows: number; fits: number }> {
  return page.locator(SCREEN).evaluate((screen) => {
    const style = getComputedStyle(screen);
    const room = screen.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom);
    const lines = screen.querySelectorAll('.xterm-rows > div');
    const lineHeight = lines[0]?.getBoundingClientRect().height ?? 1;
    return { rows: lines.length, fits: Math.floor(room / lineHeight) };
  });
}

test.describe('the thread terminal on a phone', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    const put = await apiRequest(page).put(SWITCH, { data: { value: 'true' } });
    expect(put.ok()).toBe(true);
  });

  test.afterEach(async ({ page }) => {
    await apiRequest(page).delete(SWITCH);
  });

  test('the key row sends Escape and a sticky Control, and the shell refits', async ({ page }) => {
    await navigateToApp(page);
    await waitForEventStream(page);
    await sendMessage(page, `Say exactly: "${uniqueMessage('terminal-phone')}"`);
    await waitForResponse(page);
    await ensureOnThreadPane(page);

    await page.locator('.mobile-thread-title-row .thread-title-menu:visible').click();
    await page.locator('.thread-overflow-menu').getByRole('menuitem', { name: 'Open terminal' }).click();
    await expect(page.locator(`${VIEW} [data-role="terminal-status"]`)).toContainText('1 device', { timeout: 20_000 });
    await page.locator(SCREEN).click();

    await typeLine(page, 'cat -v');
    await key(page, 'Escape').click();
    await page.keyboard.press('Enter');
    await expect(page.locator(SCREEN)).toContainText('^[', { timeout: 10_000 });

    await key(page, 'Control').click();
    await expect(key(page, 'Control')).toHaveAttribute('aria-pressed', 'true');
    await page.keyboard.type('c');
    await expect(key(page, 'Control')).toHaveAttribute('aria-pressed', 'false');
    await typeLine(page, 'echo "done-$((1+1))"');
    await expect(page.locator(SCREEN)).toContainText('done-2', { timeout: 10_000 });

    const viewport = page.viewportSize()!;
    const terminalInput = page.locator(`${SCREEN} .xterm-helper-textarea`);
    await expect(terminalInput).toBeFocused();
    const keyboardTop = await page.evaluate(() => {
      const viewport = window.visualViewport!;
      const height = Math.round(window.innerHeight * 0.6);
      Object.defineProperty(viewport, 'height', { configurable: true, value: height });
      viewport.dispatchEvent(new Event('resize'));
      return height;
    });
    const toolbar = page.locator(`${VIEW} .terminal-key-row`);
    await expect.poll(async () => {
      const box = await toolbar.boundingBox();
      return box ? Math.abs(box.y + box.height - keyboardTop) : Infinity;
    }, { message: 'the shortcut row sits at the top of the keyboard' }).toBeLessThan(2);
    await page.evaluate(() => {
      const viewport = window.visualViewport!;
      Reflect.deleteProperty(viewport, 'height');
      viewport.dispatchEvent(new Event('resize'));
    });

    await page.setViewportSize({ width: viewport.width, height: Math.round(viewport.height * 0.6) });
    await expect.poll(async () => {
      const shell = await shellSize(page, `short${Date.now()}`);
      const pane = await paneFit(page);
      return { shellMatchesScreen: shell.rows === pane.rows, screenFillsPane: Math.abs(pane.rows - pane.fits) <= 1 };
    }, { message: 'the shell follows the smaller pane', timeout: 15_000 })
      .toEqual({ shellMatchesScreen: true, screenFillsPane: true });
  });
});
