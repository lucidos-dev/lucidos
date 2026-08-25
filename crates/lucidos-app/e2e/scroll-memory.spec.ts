import { test, expect } from './fixtures';
import { navigateToApp, gotoWithRetry, waitForEventStream } from './helpers';

// Regression: ResizeObserver alone doesn't fire when only the scroll
// container's INNER content (not its own box) grows, which is the typical
// case for flex:1 lists with async-loaded children. The hook needs a
// MutationObserver fallback so the restore eventually fires.
//
// We seed a saved scroll, navigate, and assert the hook restores once content
// loads. The seed-then-load approach exercises the restore path without
// depending on real user-driven scroll save (which is covered separately).
//
// Two things shape how the view is reached. Each Settings sub-section is its
// own content-pane view (`contentViewKey`), so the pane's scroll key is
// `settings:<subview>` and nothing is stored against a bare `settings`.
//
// And the app is deep-linked STRAIGHT into the sub-section rather than walking
// the Settings home list. `SettingsView` kicks its loaders off on mount, so
// arriving via the home list resolves them first. The sub-section's own scroll
// memory would then attach with nothing left to wait on.
//
// Models is the sub-section, because its list is the seeded builtin model
// registry. That is reliably taller than the short viewport forced below, at a
// desktop width as well as a phone one.

const SUBVIEW = 'models';
const SUBVIEW_SCROLL_KEY = `lucidos-scroll-content-settings:${SUBVIEW}`;

test.describe('scroll position restore', () => {
  test('content pane restores saved scroll after async content renders', async ({ page }) => {
    // Force a short viewport so the panel always overflows: the test is about
    // the restore hook, not viewport sizing, and a non-overflow height makes
    // scrollTop a constant 0 regardless of the hook's behavior.
    const currentSize = page.viewportSize();
    if (currentSize) {
      await page.setViewportSize({ width: currentSize.width, height: 320 });
    }
    await gotoWithRetry(page, '/');
    // A modest offset: the bug isn't about exact pixel position but about
    // whether ANY restore occurs once the `Loadable<T>` model list renders.
    // Without the MutationObserver fallback, the ResizeObserver on a flex:1
    // container never fires for inner-content growth and scrollTop stays at 0.
    await page.evaluate((key) => {
      localStorage.setItem(key, '40');
    }, SUBVIEW_SCROLL_KEY);

    await navigateToApp(page);
    await waitForEventStream(page);

    // Land on the sub-section. The engine fans NavigationRequested out over
    // SSE, and the page routes it through `handleNavigationRequest` into
    // `openSettingsSubview`. That helper reveals the content pane on mobile
    // too, so this covers both layouts without driving either one's chrome.
    const res = await page.request.post('/api/v1/ui/navigate', {
      headers: { 'content-type': 'application/json' },
      data: { target: 'settings', params: { settings_view: SUBVIEW } },
    });
    expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();

    // Give content time to load and the hook to fire.
    await page.waitForTimeout(4000);

    const result = await page.evaluate(() => {
      const els = document.querySelectorAll('.content-pane-body');
      for (const el of els) {
        const rect = el.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          const max = Math.max(0, el.scrollHeight - el.clientHeight);
          return { found: true, scrollTop: el.scrollTop, max };
        }
      }
      return { found: false, scrollTop: -1, max: 0 };
    });
    expect(result.found).toBe(true);
    // Overflow forced above: absence of scroll capacity is a layout regression.
    expect(result.max).toBeGreaterThan(0);
    expect(result.scrollTop).toBeGreaterThan(0);
  });
});
