import { test, expect } from './fixtures';
import type { Locator, Page } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, waitForResponse, assertHealthy,
  uniqueMessage, ensureOnThreadPane, waitForVisibleInput,
} from './helpers';


const row = (page: Page): Locator => page.locator('.prompt-actions-row:visible').first();
const cancelButton = (page: Page): Locator => row(page).locator('button[aria-label="Cancel"]');
const submitButton = (page: Page): Locator => row(page).locator('button[aria-label="Submit answer"]');
const pendingCard = (page: Page): Locator =>
  page.locator('.question-body:not(.question-body-answered):visible').first();

const X_ICON = 'svg path[d="M18 6 6 18"]';
const CHECK_ICON = 'svg polyline[points="20 6 9 17 4 12"]';
const SEND_ICON = 'svg polyline[points="4 11 12 3 20 11"]';

async function startThread(page: Page): Promise<void> {
  await navigateToApp(page);
  await ensureOnThreadPane(page);
  await sendMessage(page, uniqueMessage('first'));
  await waitForResponse(page);
}

async function askCard(page: Page, multiSelect: boolean): Promise<void> {
  const args = JSON.stringify({
    questions: [{
      question: 'Which ones?',
      options: [{ label: 'Red' }, { label: 'Green' }],
      multiSelect,
    }],
  });
  await sendFollowUp(page, `${uniqueMessage('ask')} MOCK_TOOL_CALL: ask_user_question ${args}`);
  await expect(pendingCard(page)).toBeVisible({ timeout: 15_000 });
}

async function expectCancelOnly(page: Page): Promise<void> {
  const cancel = cancelButton(page);
  await expect(cancel).toBeVisible({ timeout: 10_000 });
  await expect(cancel).toHaveClass(/action-btn-danger/);
  await expect(cancel).toHaveAttribute('data-tooltip', /^Cancel/);
  await expect(cancel.locator(X_ICON)).toHaveCount(1);
  await expect(submitButton(page)).toHaveCount(0);
}

async function expectSubmit(page: Page): Promise<Locator> {
  const submit = submitButton(page);
  await expect(submit).toBeVisible({ timeout: 10_000 });
  await expect(submit).toHaveClass(/action-btn-confirm/);
  await expect(submit).toHaveAttribute('data-tooltip', 'Submit answer');
  await expect(submit.locator(CHECK_ICON)).toHaveCount(1);
  await expect(submit.locator(SEND_ICON)).toHaveCount(0);
  await expect(cancelButton(page)).toHaveCount(0);
  return submit;
}

test.describe('The answer button on a question card', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('no card shows the normal Send', async ({ page }) => {
    await navigateToApp(page);
    await ensureOnThreadPane(page);
    const input = await waitForVisibleInput(page);
    await input.fill(uniqueMessage('draft'));
    const send = row(page).locator('button.send-cancel-morph[aria-label="Send message"]');
    await expect(send).toBeVisible();
    await expect(send.locator(SEND_ICON)).toHaveCount(1);
    await expect(submitButton(page)).toHaveCount(0);
  });

  test('a multi-select card shows Cancel until a pick or text makes an answer', async ({ page }) => {
    await startThread(page);
    await askCard(page, true);
    const options = pendingCard(page).locator('.question-option');

    await expectCancelOnly(page);

    await options.nth(0).click();
    const submit = await expectSubmit(page);
    await expect(submit).toContainText('Submit 1');

    await options.nth(0).click();
    await expectCancelOnly(page);

    const input = await waitForVisibleInput(page);
    await input.fill('Blue, please');
    await expectSubmit(page);
    await input.fill('');
    await expectCancelOnly(page);

    await cancelButton(page).click();
    await expect(page.locator('.question-cancel-picked:visible')).toHaveCount(1, { timeout: 15_000 });
  });

  test('a single-pick card shows Submit with the checkmark for a typed answer', async ({ page }) => {
    await startThread(page);
    await askCard(page, false);
    await expectCancelOnly(page);
    const input = await waitForVisibleInput(page);
    await input.fill('Neither');
    const submit = await expectSubmit(page);
    await expect(submit).toContainText('Submit');
    await submit.click();
    await expect(page.locator('.question-freetext-text:visible').filter({ hasText: 'Neither' }))
      .toHaveCount(1, { timeout: 15_000 });
  });
});
