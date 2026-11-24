import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, uniqueMessage,
  assertHealthy, pickComposeDestination, sendFollowUp,
  waitForActionPanel, newThread, waitForCCToFinish,
  assertUserMessagesVisible, userMessageBody, USER_MSG_SELECTOR,
  waitForVisibleInput,
} from './helpers';
import { psql } from './db-helpers';

test.describe('Claude Code interaction', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('send initial message via Claude Code mode', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    await pickComposeDestination(page);
    await expect(page.locator('.compose-destination-picker .dropdown-sizer > :first-child:visible').first()).toHaveText('Lucidos source');

    const msg = uniqueMessage('cc-init');
    await sendMessage(page, `Say exactly: "hello ${msg}" and nothing else. Do not create any files.`);

    await expect(userMessageBody(page)).toContainText(msg, { timeout: 15_000 });

    await page.waitForFunction(() => {
      const els = document.querySelectorAll('.response-content');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').length > 0;
      });
    }, undefined, { timeout: 120_000 });

    await waitForCCToFinish(page);

    const response = page.locator('.response-content:visible').first();
    const text = await response.textContent();
    expect(text!.trim().length).toBeGreaterThan(0);
  });

  test('a fresh session names its branch from the session context', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const since = psql(`SELECT now()`).trim();
    const msg = uniqueMessage('cc-branch');
    await sendMessage(page, `Name your session's branch, exactly as written and nothing else. Do not create any files. (${msg})`);
    await expect(userMessageBody(page)).toContainText(msg, { timeout: 15_000 });
    await waitForCCToFinish(page);

    const branch = psql(
      `SELECT payload->>'branch' FROM events WHERE event_type='SessionStarted' ` +
      `AND created > '${since}'::timestamptz ORDER BY sequence DESC LIMIT 1`,
    ).trim();
    expect(branch).toMatch(/^lucidos-/);
    await expect(page.locator('.response-content:visible').first()).toContainText(branch, { timeout: 120_000 });
  });

  test('send follow-up to idle Claude Code session', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const msg1 = uniqueMessage('cc-idle-1');
    await sendMessage(page, `Say exactly: "first ${msg1}" and nothing else. Do not create any files.`);

    await waitForActionPanel(page, 'Archive', 120_000);

    const msg2 = uniqueMessage('cc-idle-2');
    await sendFollowUp(page, `Say exactly: "second ${msg2}" and nothing else. Do not create any files.`);

    await waitForCCToFinish(page);

    await assertUserMessagesVisible(page, [msg2]);
  });

  test('dismiss idle Claude Code session with Done', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const msg = uniqueMessage('cc-dismiss');
    await sendMessage(page, `Say exactly: "dismiss ${msg}" and nothing else. Do not create any files.`);

    const panel = await waitForActionPanel(page, 'Archive', 120_000);

    const doneBtn = panel.locator('button.action-btn:has-text("Archive")');
    await expect(doneBtn).toBeVisible();
    await doneBtn.click();

    await page.waitForFunction(({ marker, sel }) => {
      return !Array.from(document.querySelectorAll(sel)).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').includes(marker);
      });
    }, { marker: msg, sel: USER_MSG_SELECTOR }, { timeout: 10_000 });
  });

  test('typing first then toggling to Claude still routes to Claude Code', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg = uniqueMessage('cc-toggle-after-type');
    const fullText = `Say exactly: "toggled ${msg}" and nothing else. Do not create any files.`;

    const input = await waitForVisibleInput(page, 15_000);
    await input.fill(fullText);

    await pickComposeDestination(page);
    await expect(page.locator('.compose-destination-picker .dropdown-sizer > :first-child:visible').first()).toHaveText('Lucidos source');

    await input.press('Enter');

    await waitForActionPanel(page, 'Archive', 120_000);
  });

  test('CC thread shows reply placeholder when idle', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const msg = uniqueMessage('cc-placeholder');
    await sendMessage(page, `Say exactly: "placeholder ${msg}" and nothing else. Do not create any files.`);

    await waitForActionPanel(page, 'Archive', 120_000);

    const input = page.locator('[data-role="prompt-input"]:visible').first();
    const placeholder = await input.getAttribute('placeholder');
    expect(placeholder).toBe('Post a follow up…');
  });
});
