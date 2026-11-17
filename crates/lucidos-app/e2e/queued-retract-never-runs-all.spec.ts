import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import {
  assertHealthy, ensureOnThreadPane, isMobileViewport, navigateToApp, sendFollowUp,
  sendMessage, uniqueMessage, USER_MSG_SELECTOR, waitForResponse, waitForVisibleInput,
} from './helpers';
import { psql } from './db-helpers';

/** A queued message the user took back never reaches the agent, so its resend
 *  is the only copy (ADR 0706). The mock keeps a turn running with a scripted
 *  `run_python` sleep. Every project runs it: the incident was on an iPhone. */

const CANCEL = 'button.send-cancel-morph[aria-label="Cancel"]:not(:disabled):visible';

/** Deliveries of any recorded message whose text satisfies `textTest`: an
 *  injection naming it, or a turn anchored on it. */
function deliveriesOf(textTest: string): number {
  return Number(psql(
    `WITH sent AS (SELECT id::text AS id FROM events ` +
      `WHERE event_type = 'MessageReceived' AND ${textTest}) ` +
    `SELECT count(*) FROM events e JOIN sent s ON ` +
      `(e.event_type = 'PromptInjected' AND e.payload->>'injected_message_id' = s.id) ` +
      `OR (e.event_type = 'ChatTurnSettingsResolved' AND e.payload->>'request_event_id' = s.id)`,
  ));
}

/** Deliveries of a message whose text is exactly one of `texts`. */
function deliveries(texts: string[]): number {
  return deliveriesOf(`payload->>'text' IN (${texts.map((t) => `'${t}'`).join(', ')})`);
}

/** Deliveries of a message whose text contains `text`. The Cancel button
 *  cannot say a resend ran: it is gone for a moment right after the send,
 *  before the busy turn the resend queued behind has ended. */
function deliveriesContaining(text: string): number {
  return deliveriesOf(`strpos(payload->>'text', '${text}') > 0`);
}

async function startBusyTurn(page: Page, seconds: number) {
  await sendFollowUp(page, `${uniqueMessage('busy')} MOCK_RUN_PYTHON:import time; time.sleep(${seconds})`);
  await expect(page.locator(CANCEL)).toHaveCount(1, { timeout: 15_000 });
}

function userBubbles(page: Page, text: string) {
  return page.locator(`${USER_MSG_SELECTOR}:visible`).filter({ hasText: text });
}

async function send(page: Page) {
  const input = await waitForVisibleInput(page);
  if (isMobileViewport(page)) {
    await page.locator('button[aria-label="Send message"]:visible').first().click();
  } else {
    await input.press('Enter');
  }
}

test.describe('A queued message taken back never runs', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('Stop returns queued messages to compose, and only the resend runs', async ({ page }) => {
    await navigateToApp(page);
    await ensureOnThreadPane(page);
    await sendMessage(page, uniqueMessage('first'));
    await waitForResponse(page);

    await startBusyTurn(page, 20);
    const one = uniqueMessage('queued-one');
    const two = uniqueMessage('queued-two');
    await sendFollowUp(page, one);
    await sendFollowUp(page, two);
    // Two or more queued messages fold into one group, closed until opened.
    await page.locator('.queued-message-group-summary:visible').click();
    await expect(page.locator('.exchange-status-queued:visible')).toHaveCount(2, { timeout: 10_000 });

    await page.locator(CANCEL).first().click();
    const input = await waitForVisibleInput(page);
    await expect(input).toHaveValue(new RegExp(`${one}[\\s\\S]*${two}`), { timeout: 15_000 });
    await expect(page.locator(CANCEL)).toHaveCount(0, { timeout: 15_000 });

    await send(page);
    await expect.poll(() => deliveriesContaining(one), { timeout: 60_000 }).toBe(1);
    await expect(page.locator(CANCEL)).toHaveCount(0, { timeout: 60_000 });
    // Long enough for a removed original to have started its own turn.
    await page.waitForTimeout(3_000);
    await expect(userBubbles(page, one)).toHaveCount(1);
    await expect(userBubbles(page, two)).toHaveCount(1);
    expect(deliveries([one, two]), 'a removed original reached the agent').toBe(0);
  });

  test('Edit takes a queued message back, and its resend runs once', async ({ page }) => {
    await navigateToApp(page);
    await ensureOnThreadPane(page);
    await sendMessage(page, uniqueMessage('first'));
    await waitForResponse(page);

    await startBusyTurn(page, 10);
    const edited = uniqueMessage('edited');
    await sendFollowUp(page, edited);
    await expect(page.locator('.exchange-status-queued:visible')).toHaveCount(1, { timeout: 10_000 });

    await page.locator('.queued-message-edit:visible').first().click();
    const input = await waitForVisibleInput(page);
    await expect(input).toHaveValue(edited, { timeout: 10_000 });
    await expect(userBubbles(page, edited)).toHaveCount(0);

    await send(page);
    await expect.poll(() => deliveriesContaining(edited), { timeout: 60_000 }).toBe(1);
    await expect(page.locator(CANCEL)).toHaveCount(0, { timeout: 60_000 });
    await page.waitForTimeout(3_000);
    await expect(userBubbles(page, edited)).toHaveCount(1);
    expect(deliveries([edited]), 'the edited message ran more than once').toBe(1);
  });
});
