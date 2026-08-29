/**
 * A call reaches the engine and is refused in plain English.
 *
 * The whole hop, and the only test that covers it: the toggle, the microphone,
 * the socket, the gateway, the engine's refusal, and the teardown. It switches
 * the OpenAI provider off first, so what comes back is a sentence rather than a
 * conversation.
 *
 * The switch is a veto, so the refusal holds whether or not this machine has a
 * key. Blanking the voice model does NOT: an unset model resolves to the
 * catalog default, which is the whole point of `voice::build::talker_model`.
 *
 * A conversation is not testable here at all. It would need a real provider, a
 * real voice and a real ear, so the plan keeps it manual.
 *
 * `-desktop` is about the DEVICE, not the viewport. Only Chromium can be handed
 * a fake microphone by Playwright. The suffix is what keeps this file out of
 * the two mobile projects, one of which is WebKit.
 */
import { test, expect } from '@playwright/test';
import { gotoWithRetry, waitForVisibleInput } from './helpers';

// A fake capture device, so `getUserMedia` resolves with no hardware and no
// consent prompt. Both flags are needed: one supplies the device, the other
// answers the permission dialog headless Chromium would otherwise sit on.
test.use({
  launchOptions: {
    args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'],
  },
  permissions: ['microphone'],
});

const OPENAI_SWITCH_KEY = 'provider_enabled_openai';
const CALL_TOGGLE = '[data-role="call-toggle"]';

/** Flip the OpenAI provider switch. Off means the engine has no talker. */
async function setOpenAiProvider(
  page: import('@playwright/test').Page,
  on: boolean,
): Promise<void> {
  const res = await page.request.put(`/api/v1/preferences?key=${OPENAI_SWITCH_KEY}`, {
    data: { value: String(on) },
  });
  expect(res.ok()).toBeTruthy();
}

test.describe('a call the engine cannot take', () => {
  // The switch is GLOBAL, and the e2e database resets only between projects. A
  // spec that left OpenAI off would take voice away from every later one, so
  // `afterAll` switches it back on.
  test.beforeAll(async ({ browser }) => {
    const page = await browser.newPage();
    await setOpenAiProvider(page, false);
    await page.close();
  });

  test.afterAll(async ({ browser }) => {
    const page = await browser.newPage();
    await setOpenAiProvider(page, true);
    await page.close();
  });

  test('says why in plain English, and leaves no call behind', async ({ page }) => {
    await gotoWithRetry(page);
    await waitForVisibleInput(page);

    const toggle = page.locator(CALL_TOGGLE).first();
    await expect(toggle).toBeVisible();
    await expect(toggle).toHaveAttribute('aria-pressed', 'false');

    await toggle.click();

    // The engine sends an `error` frame and closes. The strip goes with the
    // call, so the reason arrives as a toast. That is the whole point of
    // routing it there rather than onto a surface that has gone.
    const toast = page.locator('.toast-error:visible').first();
    await expect(toast).toBeVisible({ timeout: 30_000 });
    await expect(toast).toContainText('voice model');

    // No provider name, no model id, no status code. The frame's own contract.
    const said = (await toast.textContent()) ?? '';
    expect(said).not.toMatch(/openai|gpt-|realtime|\b[45]\d\d\b/i);

    // Back to idle: the toggle is off and the strip is gone, so nothing is
    // holding the microphone.
    await expect(toggle).toHaveAttribute('aria-pressed', 'false');
    await expect(page.locator('[data-role="call-strip"]')).toHaveCount(0);
  });

  test('the toggle is offered on the compose view, like every prompt input', async ({ page }) => {
    await gotoWithRetry(page);
    await waitForVisibleInput(page);
    const toggle = page.locator(CALL_TOGGLE).first();
    await expect(toggle).toBeVisible();
    await expect(toggle).toHaveAccessibleName('Start a call');
  });
});
