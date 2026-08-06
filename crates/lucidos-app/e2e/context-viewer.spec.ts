import { test, expect } from './fixtures';
import {
  navigateToApp,
  newThread,
  sendMessage,
  waitForResponse,
  assertHealthy,
} from './helpers';

/**
 * Phase 7.1 of the loaded-knowhow + context-viewer reorg
 * (`docs/plans/2026-05-15-loaded-knowhow-and-context-viewer-reorg.md`).
 *
 * The LLM Context Viewer used to render a flat list of sections. Phase 5–6
 * groups them by API role (system / prior_message / user) at the outer layer,
 * with a curated tier order ("Identity & profile", "Workspace inventory",
 * "Memory & history", "Loaded knowhow", "Active context", "System notices",
 * "The request") inside the user role.
 *
 * The mock LLM provider (`LUCIDOS_MODEL=mock`, set by `scripts/lib/e2e.sh`)
 * never issues tool calls, so a real `load_knowhow`-driven turn isn't
 * reproducible here. We exercise the rendering path with the sections the
 * mock turn DOES emit ("System Instructions", "Conversation History",
 * "User Message", …) and assert that the new role headers appear and that
 * sections nest underneath the right ones. The "Loaded knowhow" tier is
 * exercised by the unit tests in `contextGrouping.test.ts` and by the API
 * e2e in `crates/lucidos-e2e/tests/api_support/load_knowhow_dedup_test.rs`.
 */
test.describe('LLM Context Viewer two-layer grouping', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('groups sections by API role with inner tier nesting', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    await sendMessage(page, 'Say "hello world" and nothing else.');
    await waitForResponse(page);

    // Inline steps are collapsed by default — open them to surface the
    // context counter on the latest step, which is the viewer's door.
    const showStepsBtn = page
      .locator('button.details-toggle:visible', { hasText: 'Show steps' })
      .first();
    await expect(showStepsBtn).toBeVisible({ timeout: 30_000 });
    await showStepsBtn.click();

    const counter = page
      .locator('[data-role="inline-step"]:visible [data-role="step-context"]')
      .first();
    await expect(counter).toBeVisible({ timeout: 30_000 });
    await counter.click();

    const modal = page.locator('[data-role="context-captured-modal"]:visible');
    await expect(modal).toBeVisible();

    // Outer role headers — `.context-role-label` is the new class introduced
    // by Phase 6's `ContextRoleGroup`. The mock turn always produces a
    // non-empty system prompt and user message, so both rows must render.
    // The "Prior messages" row only appears when the thread has resume tool
    // blocks (i.e. follow-ups with prior tool calls), so it's gated below.
    await expect(modal.locator('.context-role-label', { hasText: 'System role' }))
      .toBeVisible();
    await expect(modal.locator('.context-role-label', { hasText: 'User message (this turn)' }))
      .toBeVisible();

    // Sanity: the role accordion is open by default (Phase 6 design), so at
    // least one inner tier under "User message" must be visible without
    // any clicks. "The request" is the user-typed prompt — present on every
    // chat turn — and lives in its own inner tier.
    await expect(
      modal.locator('.context-inner-label', { hasText: 'The request' })
    ).toBeVisible();

    // Inner tiers nest under the role: assert at least one
    // `.context-inner-group` exists inside the user role region. The exact
    // tiers vary with workspace state (Workspace inventory only appears if
    // the file list is non-empty, etc.) but at minimum "The request" is
    // always there.
    const innerGroupCount = await modal.locator('.context-inner-group').count();
    expect(innerGroupCount).toBeGreaterThanOrEqual(1);

    // Section rows live inside inner groups now (not directly under the
    // role). At least one section should be reachable via the new nesting.
    const nestedSections = modal.locator('.context-inner-group [data-role="section-row"]');
    expect(await nestedSections.count()).toBeGreaterThanOrEqual(1);
  });
});
