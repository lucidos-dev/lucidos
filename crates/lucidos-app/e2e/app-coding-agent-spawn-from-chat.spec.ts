import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy } from './helpers';
import { createIframeAppFixture } from './db-helpers';

test.describe('App coding-agent thread — compose destination picker', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  // The compose destination picker groups coding targets (Lucidos source /
  // apps / repositories) under one "Coding agent on…" header, below the
  // Lucidos Agent default. Once an app exists in the workspace, the dropdown
  // must surface it as `<name> · app`. End-to-end CC spawn from the dropdown
  // is exercised by app-coding-agent-wip-preview (which uses a seeded thread
  // to skip the LLM); this spec stays focused on the picker contract because
  // LLM-driven spawn from a chat message is non-deterministic.
  test('destination picker exposes installed apps as coding targets', async ({ page }) => {
    const appId = `e2e-scope-${Date.now()}`;
    const fixture = createIframeAppFixture(appId, {
      html: '<!doctype html><title>scope picker fixture</title>',
      js: '/* noop */',
      manifest: { id: appId, name: `${appId} fixture`, description: 'scope picker e2e' },
    });
    try {
      // The chip default is the workspace-persisted `coding_agent_default`
      // preference, which survives across test runs in the e2e workspace —
      // pin it so a stray Codex pick (manual debugging, a future spec) can't
      // poison the 'Claude Code' assertion below.
      const prefReset = await page.request.put('/api/v1/preferences?key=coding_agent_default', {
        data: { value: 'claude-code' },
      });
      expect(prefReset.ok()).toBeTruthy();

      await navigateToApp(page);

      const trigger = page.locator('.compose-destination-picker .dropdown-trigger:visible').first();
      await expect(trigger).toBeVisible({ timeout: 10_000 });
      await trigger.click();

      // "Coding agent on…" is a disabled section header; the app rides below
      // it as a selectable `<name> · app` option. Asserting both proves the
      // grouping survived.
      const menu = page.locator('.dropdown-menu:visible').first();
      await expect(menu).toBeVisible({ timeout: 5_000 });
      const codingHeader = menu.locator('.dropdown-option-header', { hasText: 'Coding agent on…' });
      await expect(codingHeader).toBeVisible();
      const appOption = menu.locator('.dropdown-option', { hasText: `${appId} fixture · app` });
      await expect(appOption).toBeVisible();
      // The two fixed rows: the Lucidos Agent default and the Lucidos source
      // coding target.
      await expect(menu.locator('.dropdown-option', { hasText: 'Lucidos Agent' }).first()).toBeVisible();
      await expect(menu.locator('.dropdown-option', { hasText: 'Lucidos source' }).first()).toBeVisible();

      // Picking the app writes `{kind:'app',appId}` into localStorage under
      // the persisted scope key AND flips the input mode to the coding-agent
      // channel (one pick does both — that's the point of the destination
      // picker). Assert the writes so a regression in the option-value encoding
      // (repo: vs app: prefix) doesn't silently send the wrong scope on the
      // next send.
      await appOption.click();
      const stored = await page.evaluate(
        () => localStorage.getItem('lucidos-coding-agent-last-scope'),
      );
      expect(stored).toBeTruthy();
      const parsed = JSON.parse(stored!);
      expect(parsed.kind).toBe('app');
      expect(parsed.appId).toBe(appId);
      const mode = await page.evaluate(
        () => localStorage.getItem('lucidos-input-mode'),
      );
      expect(mode).toBeTruthy();
      expect(JSON.parse(mode!).type).toBe('coding_agent');

      // With a coding target picked, the coding-agent chip appears with the
      // remembered backend (Claude Code by default).
      await expect(
        page.locator('.compose-coding-agent-chip .dropdown-sizer > :first-child:visible').first(),
      ).toHaveText('Claude Code');
    } finally {
      fixture.cleanup();
    }
  });
});
