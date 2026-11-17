import { test, expect } from './fixtures';
import {
  apiRequest, assertHealthy, clickVisibleElement, ensureMobileView,
  navigateToApp, sendMessage, waitForResponse,
} from './helpers';

test.use({ reducedMotion: 'no-preference' });

test('Discuss keeps an already docked composer still while starting a thread', async ({ page }) => {
  await assertHealthy(page);
  await navigateToApp(page);
  await sendMessage(page, 'Reply with just OK.');
  await waitForResponse(page);
  await ensureMobileView(page, 'content');
  const result = await apiRequest(page).post('/api/v1/notifications', {
    data: { title: 'Composer position regression', message: 'A notification to discuss.' },
  });
  expect(result.ok()).toBeTruthy();
  await clickVisibleElement(page, '.notifications-bell');
  await page.locator('.notification-item:visible').filter({ hasText: 'Composer position regression' }).first().click();
  const discuss = page.locator('.notification-detail-actions:visible button').filter({ hasText: /^Discuss$/ }).first();
  await expect(discuss).toBeVisible();

  // Keep the thread-creation round trip in the recording window. The optimistic
  // send must already have its final composer layout during that wait.
  await page.route('**/api/v1/threads', async (route) => {
    if (route.request().method() === 'POST') await new Promise((resolve) => setTimeout(resolve, 400));
    await route.continue();
  });
  await page.evaluate(() => {
    const frames: Array<{ top: number; height: number; text: string }> = [];
    const state = window as unknown as {
      composerFrames: typeof frames; recordingComposer: boolean; missingComposerFrames: number;
    };
    state.composerFrames = frames;
    state.recordingComposer = true;
    state.missingComposerFrames = 0;
    function sample() {
      const input = [...document.querySelectorAll<HTMLTextAreaElement>('[data-role="prompt-input"]')]
        .find((el) => el.offsetWidth > 0 && el.closest('.thread-pane'));
      if (input) {
        const box = input.getBoundingClientRect();
        frames.push({ top: box.top, height: box.height, text: input.value });
      } else {
        state.missingComposerFrames++;
      }
      if (state.recordingComposer) requestAnimationFrame(sample);
    }
    requestAnimationFrame(sample);
  });
  try {
    const created = page.waitForResponse((response) =>
      new URL(response.url()).pathname === '/api/v1/threads' && response.request().method() === 'POST');
    await discuss.click();
    expect((await created).ok(), 'Discuss creates its thread during the recording').toBe(true);
    await page.waitForTimeout(1500);
  } finally {
    await page.evaluate(() => {
      (window as unknown as { recordingComposer: boolean }).recordingComposer = false;
    });
  }
  const frames = await page.evaluate(() => (window as unknown as {
    composerFrames: Array<{ top: number; height: number; text: string }>;
  }).composerFrames);
  expect(frames.length).toBeGreaterThan(10);
  const missing = await page.evaluate(() =>
    (window as unknown as { missingComposerFrames: number }).missingComposerFrames);
  expect(missing, 'the docked composer stays mounted throughout thread creation').toBe(0);
  // Ignore horizontal pane navigation: all samples belong to the same docked
  // composer geometry, including when the phone's thread pane is offscreen.
  const tops = frames.map((frame) => frame.top);
  expect(Math.max(...tops) - Math.min(...tops), JSON.stringify(frames)).toBeLessThanOrEqual(2);
  const heights = frames.map((frame) => frame.height);
  expect(Math.max(...heights) - Math.min(...heights), JSON.stringify(frames)).toBeLessThanOrEqual(2);
  expect(frames.every((frame) => frame.text === ''), 'the seeded prompt is sent without flashing in the box').toBe(true);
});
