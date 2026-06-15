import { test, expect, Page, Locator } from '@playwright/test';
import { navigateToApp, assertHealthy, clickVisibleElement, isMobileViewport } from './helpers';

/** The trigger form's "Allowed side-effects" round-trip (ADR 0002, Phase 5).
 *  Ticking a couple of side-effect checkboxes, saving, then reloading and
 *  reopening the trigger must show them still checked — i.e. the grant persists
 *  through create → reload → edit. Drives the real TriggerDetails form, so it
 *  also pins the checkbox ↔ `side_effect_grant` wiring end to end. */

// Labels mirror SIDE_EFFECT_CATEGORIES in
// crates/lucidos-app/src/components/triggers/TriggerDetails.tsx.
const EMAIL_LABEL = 'Send email or messages';
const CLOUD_LABEL = 'Cloud CLI mutations (gh / aws / gcloud)';
const EXTERNAL_LABEL = 'Call external APIs (mutating HTTP)';

/** Navigate to the Triggers panel. The nav drawer (with the menu items) is
 *  hidden by default on BOTH layouts and opened via the `.hamburger-panel`
 *  toggle, so open it first, then click 'Triggers'. On mobile the hamburger
 *  lives on the content pane, so swipe there first. Finally waits for the
 *  panel's "Add Trigger" card so callers never click a still-loading list. */
async function openTriggersPanel(page: Page): Promise<void> {
  if (isMobileViewport(page)) {
    await page.evaluate(() => {
      const dot = document.querySelector('button.mobile-dot[aria-label="content view"]');
      if (dot) (dot as HTMLElement).click();
    });
    await page.waitForTimeout(300);
  }
  await clickVisibleElement(page, '.hamburger-panel');
  await page.waitForFunction(() => {
    const items = document.querySelectorAll('.drawer-item');
    return Array.from(items).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, undefined, { timeout: 5_000 });
  await clickVisibleElement(page, '.drawer-item', 'Triggers');
  // The Add Trigger card only renders once the triggers list has loaded — wait
  // for it so the create flow doesn't race the projection fetch.
  await expect(addTriggerCard(page)).toBeVisible({ timeout: 10_000 });
}

/** The visible copy of the inline trigger form (ContentPane renders one for
 *  desktop + one for mobile; only one is on-screen at a time). */
function visibleForm(page: Page): Locator {
  return page.locator('.inline-form:visible').first();
}

function addTriggerCard(page: Page): Locator {
  return page.locator('.list-row-add-card:visible', { hasText: 'Add Trigger' }).first();
}

/** The label row for a side-effect identified by its text. */
function sideEffectRow(page: Page, label: string): Locator {
  return visibleForm(page)
    .locator('.form-checkbox-list label.form-checkbox-row', { hasText: label });
}

function sideEffectCheckbox(page: Page, label: string): Locator {
  return sideEffectRow(page, label).locator('input[type="checkbox"]');
}

/** Tick a side-effect by clicking its label text — NOT the `<input>`. The
 *  checkbox is a native input wrapped in a `<label>`; clicking the input
 *  directly double-fires (input toggle + the label's synthesized toggle) and
 *  nets no change, so Playwright's `.check()` reports "did not change its state".
 *  Clicking the label's span forwards exactly one toggle to the control. */
async function tickSideEffect(page: Page, label: string): Promise<void> {
  const checkbox = sideEffectCheckbox(page, label);
  if (!(await checkbox.isChecked())) {
    await sideEffectRow(page, label).locator('span').click();
  }
  await expect(checkbox).toBeChecked();
}

test.describe('Trigger form — Allowed side-effects', () => {
  let triggerName: string;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    triggerName = `e2e-side-effects-${Date.now()}`;
    // create_trigger reads the timezone preference; set it so the create path
    // never has to fall back to its UTC default mid-test.
    await page.request.put('/api/v1/preferences?key=timezone', { data: { value: 'UTC' } });
  });

  test.afterEach(async ({ page }) => {
    // Delete the trigger we created so it doesn't pollute sibling tests' /triggers
    // polls within the same Playwright project (the DB only resets between projects).
    try {
      const res = await page.request.get('/api/v1/triggers');
      const body = await res.json();
      const mine = (body.triggers ?? []).find((t: { name: string; id: string }) => t.name === triggerName);
      if (mine) await page.request.delete(`/api/v1/triggers?id=${mine.id}`);
    } catch {
      /* best-effort cleanup */
    }
  });

  test('ticked side-effects persist through create → reload → edit', async ({ page }) => {
    await navigateToApp(page);
    await openTriggersPanel(page);

    // Open the create-trigger form.
    await addTriggerCard(page).click();
    const form = visibleForm(page);
    await expect(form).toBeVisible({ timeout: 10_000 });

    // Minimal valid trigger: name + one cron + an intent. (formType defaults to
    // 'schedule', runType to 'intent', so the cron + intent fields are shown.)
    await form.locator('input[placeholder="e.g. Morning Brief"]').fill(triggerName);
    await form.locator('input[placeholder="0 0 8 * * *"]').fill('0 0 8 * * *');
    await form.locator('.prompt-textarea').fill('Send me a hello every morning');

    // Tick two distinct side-effects; leave the rest off.
    await tickSideEffect(page, EMAIL_LABEL);
    await tickSideEffect(page, CLOUD_LABEL);

    // Save — the form closes and the new trigger lands in the list.
    await form.locator('.btn-save').click();
    await expect(page.locator('.trigger-row .list-row-name', { hasText: triggerName }).first())
      .toBeVisible({ timeout: 10_000 });

    // Fresh load, reopen the trigger for editing.
    await navigateToApp(page);
    await openTriggersPanel(page);
    // Click the row's name (bubbles to the row's openEditTrigger handler) — more
    // robust on narrow mobile widths than clicking the row's center, which can
    // land on the Delete/Pause buttons.
    await page.locator('.trigger-row .list-row-name', { hasText: triggerName }).first().click();

    const editForm = visibleForm(page);
    await expect(editForm).toBeVisible({ timeout: 10_000 });
    // The two ticked grants persisted; an un-ticked one stayed off.
    await expect(sideEffectCheckbox(page, EMAIL_LABEL)).toBeChecked();
    await expect(sideEffectCheckbox(page, CLOUD_LABEL)).toBeChecked();
    await expect(sideEffectCheckbox(page, EXTERNAL_LABEL)).not.toBeChecked();
  });
});
