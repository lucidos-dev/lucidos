import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy } from './helpers';

// While ANY overlay is open, the UI behind it is made inert — no hover
// highlight, no activation; a click just lands on `.app-shell` and dismisses the
// overlay (the hover analog of the outside-click swallow). The reported case: a
// backdrop-less popover (the connection/control panel) left the header icons
// behind it still hover-highlighting. Driven by `data-overlay-open` on <html> +
// CSS (`.app-shell > *` pointer-events:none, overlay panels re-enabled via
// `[data-overlay-panel]`, the opening toggle via `[data-overlay-anchor]`). Probed
// here via computed `pointer-events`, which is what both the hover and the
// activation key off. The anchor MUST stay `auto` so re-activating it closes via
// its own handler — that exemption is what the message-route-panel / cc-slash
// "second click closes the panel" specs depend on.
test.describe('Overlay makes the UI behind it inert', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('opening a backdrop-less popover disables hover/clicks on the UI behind it but keeps the anchor live', async ({ page }) => {
    await navigateToApp(page);

    // A header icon behind the popover. Interactive to begin with. The thread
    // drawer toggle rather than the Search everywhere one, because search moved
    // into the Lucidos mark's menu on mobile and this spec runs on the mobile
    // projects too; the drawer toggle is on the header of both layouts.
    const behind = page.locator('.thread-toggle:visible').first();
    const pe = (loc: typeof behind) => loc.evaluate((el) => getComputedStyle(el).pointerEvents);
    expect(await pe(behind)).not.toBe('none');

    // Open the compose-destination picker — a backdrop-less popover, the same
    // kind as the connection panel the user reported.
    const anchor = page.locator('.compose-destination-picker .dropdown-trigger:visible').first();
    await anchor.click();
    // The menu is portaled to <body> (so it clears the header's stacking
    // context), hence located on its own class rather than under the picker.
    const menu = page.locator('.dropdown-menu:visible').first();
    await expect(menu).toBeVisible();

    // While it's open: <html data-overlay-open>; the icons behind are inert; the
    // panel AND the opening toggle stay interactive. Poll rather than read once:
    // the markers are set in layout effects, so they land in the menu's own
    // commit, but the driver's click resolves before that commit runs.
    const overlayOpen = () => page.evaluate(() => document.documentElement.hasAttribute('data-overlay-open'));
    await expect.poll(overlayOpen).toBe(true);
    await expect.poll(() => pe(behind)).toBe('none');
    await expect.poll(() => pe(menu)).toBe('auto');
    // The anchor toggle is inside `.app-shell` but must NOT go inert — otherwise
    // re-clicking it to close can't fire its own handler (the regression that
    // wedged the route-badge / Codex-controls toggles).
    await expect.poll(() => pe(anchor)).toBe('auto');
    // `.app-shell` itself stays a hit target so an outside click lands on it.
    const appShellPe = await page.evaluate(
      () => getComputedStyle(document.querySelector('.app-shell')!).pointerEvents,
    );
    expect(appShellPe).not.toBe('none');

    // Dismiss via Escape; the behind UI is live again. Poll for the same reason
    // as above: the cleanup runs in the unmount commit, which the driver's
    // keypress resolves ahead of.
    await page.keyboard.press('Escape');
    await expect(page.locator('.dropdown-menu')).toHaveCount(0);
    await expect.poll(overlayOpen).toBe(false);
    await expect.poll(() => pe(behind)).not.toBe('none');
  });
});
