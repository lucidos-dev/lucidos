/**
 * End-to-end verification that the Lucidos Agent's `navigate_ui` tool can
 * deep-link the running UI into a Settings sub-view, using Settings > Backup as
 * the primary case.
 *
 * Full path under test (engine → SSE → page):
 *   navigate_ui LLM tool / POST /api/v1/ui/navigate
 *     → engine emits ThreadEvent::NavigationRequested (payload = JSON args)
 *     → SSE frame { type: "ThreadEvent",
 *                   data: { event: { type: "NavigationRequested", payload } } }
 *     → thread-sync.ts handleTransientSideEffects → handleNavigationRequest
 *     → switchMenuItem('settings') + openSettingsSubview('backup')
 *     → SettingsView renders <BackupSection/>.
 *
 * We drive the same POST /api/v1/ui/navigate endpoint the SDK's
 * `lucidos.ui.navigate(...)` uses — it emits NavigationRequested through the
 * EventBus exactly like the LLM tool handler (execute_navigate_ui in
 * engine/tools/mod.rs), so this exercises the real SSE-delivered navigation
 * path, not a direct store poke.
 *
 * Desktop-only (`-desktop.spec.ts`, chromium project only): the mobile
 * pane-swipe for `target: settings` is already pinned by
 * navigation-pane-reveal-mobile.spec.ts. Here we assert the SUB-VIEW renders
 * (the Backup panel) and the Settings menu item carries the active state in the
 * nav drawer — both layout-stable on desktop.
 */
import { test, expect } from '@playwright/test';
import { assertHealthy, navigateToApp, clickVisibleElement } from './helpers';

/** Section title unique to BackupSection ("Restore from backup"). It renders
 *  unconditionally — independent of whether the backup provider list has
 *  loaded — so it's a stable "the Backup panel rendered" marker. */
const BACKUP_RESTORE_ANCHOR = '[data-search-anchor="backup:restore"]';

test.describe('navigate_ui → Settings > Backup', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('POST /api/v1/ui/navigate { settings, backup } renders the Backup panel and activates the Settings menu item', async ({ page }) => {
    await navigateToApp(page);

    // Emit NavigationRequested via the engine → it fans out over SSE → the
    // page's thread-sync handler routes it through handleNavigationRequest.
    const res = await page.request.post('/api/v1/ui/navigate', {
      headers: { 'content-type': 'application/json' },
      data: { target: 'settings', params: { settings_view: 'backup' } },
    });
    expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();

    // 1. The Backup panel renders: the content pane switches to the settings
    //    panel showing the BackupSection sub-view. We require the
    //    "Restore from backup" anchor to be a visible descendant of
    //    `.settings-panel` so we're asserting the rendered sub-view, not a
    //    stray search-index node.
    await page.waitForFunction((sel) => {
      const panel = document.querySelector('.settings-panel');
      if (!panel) return false;
      const anchor = panel.querySelector(sel);
      if (!anchor) return false;
      const rect = anchor.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    }, BACKUP_RESTORE_ANCHOR, { timeout: 15_000 });

    // The content-pane header title is derived from
    // activeMenuItem === 'settings' && settingsSubview === 'backup'
    // (getContentTitle), so "Backup" doubles as proof the router landed on the
    // correct sub-view — not just somewhere inside settings.
    await expect(page.locator('.pane-header-content-title').first())
      .toContainText('Backup', { timeout: 10_000 });

    // 2. The Settings menu item is active: open the nav drawer (hamburger) and
    //    confirm the Settings row carries `.active`
    //    (activeMenuItem === 'settings'). The drawer only mounts while open, so
    //    we click the hamburger first, then poll for the active row.
    await clickVisibleElement(page, '.hamburger-panel');
    await page.waitForFunction(() => {
      const active = Array.from(document.querySelectorAll('.drawer-item.active'));
      return active.length > 0 && active.every((el) => (el.textContent ?? '').trim() === 'Settings');
    }, undefined, { timeout: 10_000 });
  });
});
