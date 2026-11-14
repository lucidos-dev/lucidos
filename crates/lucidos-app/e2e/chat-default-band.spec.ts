import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, newThread, pickComposeDestination } from './helpers';
import estimates from '../../lucidos-engine/src/llm/rung_estimates.json' with { type: 'json' };

// Route reads as a fresh workspace without changing the suite's preferences.
test.use({ serviceWorkers: 'block' });

test('the chat pill shows the shipped default band with unset preferences', async ({ page }) => {
  await assertHealthy(page);
  await page.route('**/api/v1/preferences**', async (route) => {
    if (route.request().method() !== 'GET') return route.fallback();
    const response = await route.fetch();
    const body = await response.json();
    delete body.preferences.chat_model;
    delete body.preferences.chat_reasoning_efforts;
    await route.fulfill({ response, json: body });
  });
  await page.route('**/api/v1/intelligence-ladder**', async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    if (body.surface === 'chat') {
      body.rungs = body.rungs.map((rung: { model: string; effort: string }) => ({
        ...rung,
        on: estimates.surfaces.chat.some((p) => p.model === rung.model && p.effort === rung.effort && p.on),
      }));
    }
    await route.fulfill({ response, json: body });
  });
  try {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page, 'Lucidos Agent');
    const pill = page.locator('.rung-control:visible');
    await expect(pill.locator('.rung-name')).toContainText('Opus 5.5');
    await expect(pill.locator('.rung-intelligence')).toHaveText('1.0×');
    await pill.locator('.rung-trigger').click();
    await expect(page.locator('.control-option-current:visible')).toContainText('Opus 5.5 · medium');
  } finally {
    await page.unrouteAll({ behavior: 'ignoreErrors' });
  }
});
