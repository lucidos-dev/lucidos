import { test, expect } from '@playwright/test';
import {
  navigateToApp, sendMessage, uniqueMessage,
  assertHealthy, switchToClaudeMode, sendFollowUp,
  waitForActionPanel, newThread, waitForCCToFinish,
  assertUserMessagesVisible, userMessageBody, USER_MSG_SELECTOR,
  isMobileViewport, clickVisibleElement, waitForVisibleInput,
} from './helpers';

test.describe('Claude Code interaction', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('send initial message via Claude Code mode', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    await switchToClaudeMode(page);
    await expect(page.locator('button.segmented-btn.active:visible').first()).toHaveText('Claude');

    const msg = uniqueMessage('cc-init');
    await sendMessage(page, `Say exactly: "hello ${msg}" and nothing else. Do not create any files.`);

    await expect(userMessageBody(page)).toContainText(msg, { timeout: 15_000 });

    // Wait for CC to produce some response content
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

  test('send follow-up to idle Claude Code session', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

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
    await switchToClaudeMode(page);

    const msg = uniqueMessage('cc-dismiss');
    await sendMessage(page, `Say exactly: "dismiss ${msg}" and nothing else. Do not create any files.`);

    const panel = await waitForActionPanel(page, 'Archive', 120_000);

    const doneBtn = panel.locator('button.action-btn:has-text("Archive")');
    await expect(doneBtn).toBeVisible();
    await doneBtn.click();

    // After dismiss, our thread should no longer be focused — verify our
    // unique message is gone from the visible content (handleArchiveThread
    // focuses the next review thread or unfocuses entirely).
    await page.waitForFunction(({ marker, sel }) => {
      return !Array.from(document.querySelectorAll(sel)).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').includes(marker);
      });
    }, { marker: msg, sel: USER_MSG_SELECTOR }, { timeout: 10_000 });
  });

  // Regression: the prompt toggle must decide routing at SEND time, not at
  // draft-creation time. Typing first stamps thread.meta.channel from the
  // current toggle ('chat'), so toggling to Claude afterwards has to update
  // the channel binding before sendMessage reads it — otherwise the message
  // routes through Lucidos despite the UI showing Claude. Mirrors the user
  // report: "I selected Claude Code in the toggle but it started Lucidos".
  test('typing first then toggling to Claude still routes to Claude Code', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg = uniqueMessage('cc-toggle-after-type');
    const fullText = `Say exactly: "toggled ${msg}" and nothing else. Do not create any files.`;

    // Type FIRST — this is what creates the optimistic compose thread with
    // channel='chat' (because the toggle is still Lucidos at this moment).
    const input = await waitForVisibleInput(page, 15_000);
    await input.fill(fullText);

    // Now toggle to Claude. Pre-fix this only updated draft.mode + inputMode
    // but left thread.meta.channel stale → the send routed through Lucidos.
    await switchToClaudeMode(page);
    await expect(page.locator('button.segmented-btn.active:visible').first()).toHaveText('Claude');

    // Send via the same viewport gate sendMessage uses: Enter on desktop,
    // click Send on mobile (mobile disables Enter-to-submit so the soft
    // keyboard can insert newlines).
    if (isMobileViewport(page)) {
      await clickVisibleElement(page, 'button[aria-label="Send message"]');
    } else {
      await input.press('Enter');
    }

    // The CC-specific "Archive" action panel only appears for Claude Code
    // threads — Lucidos chats stream a response and stop, no action panel.
    // If the bug were back, this would time out waiting for an Archive button.
    await waitForActionPanel(page, 'Archive', 120_000);
  });

  test('CC thread shows reply placeholder when idle', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    const msg = uniqueMessage('cc-placeholder');
    await sendMessage(page, `Say exactly: "placeholder ${msg}" and nothing else. Do not create any files.`);

    await waitForActionPanel(page, 'Archive', 120_000);

    const input = page.locator('[data-role="prompt-input"]:visible').first();
    const placeholder = await input.getAttribute('placeholder');
    expect(placeholder).toBe('Post a follow up…');
  });
});
