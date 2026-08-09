/**
 * A toast with a long body stays a toast: it must not grow into a card that
 * covers the pane behind it.
 *
 * The regression (2026-08-09, iOS PWA at 390pt): a memory-watchdog notification,
 * whose body embeds a shell command and a full metrics dump, rendered from just
 * under the app header down to the composer, roughly 80% of the screen height.
 * `.toast` was capped at `100dvh - 6rem`, which is a cap in name only.
 *
 * The rule itself is pinned by the source scan in
 * `src/styles/__tests__/toast-height-cap.test.ts`, which also covers desktop
 * (the same rule, so the same unbounded growth). What only a browser can answer
 * is what the whole cascade RESOLVES to on a phone: the mobile `:root` font size,
 * `mobile.css`'s own `.toast` block, the real `env()` insets and the live
 * `--app-header-bottom`. So the assertion here is the rendered geometry, and it
 * is deliberately the same quantity the bug was reported in, a fraction of the
 * viewport.
 *
 * The toast is SPLICED into the live page rather than raised through the
 * notification path. Rendering a real in-app notification toast needs the
 * engine's push-suppression decision (`NotificationToastRequested`, which only
 * fires when a device pongs in), and that path already has unit coverage; what
 * is under test here is the box, which resolves identically however it got
 * mounted. Same technique, and the same reason, as the `.question-option` probe
 * in `prompt-transcript-alignment.spec.ts`.
 */
import { test, expect } from './fixtures';
import { assertHealthy, gotoWithRetry } from './helpers';

/** A body in the shape that produced the report: a couple of sentences of
 *  metrics, a shell command to run, and a verbatim sample line. Nothing about
 *  the assertion depends on the wording, only on it being far longer than the
 *  cap. */
const LONG_BODY = [
  'Memory is being squeezed: free 3.67 GB (18.32 GB reclaimable), compressor 11.40 GB, swap 0.08 GB, pressure critical.',
  'Nothing safe to reclaim automatically. Save your work and find the hogs:',
  '',
  'ps -Aww -o rss,command | sort -rn | head -20',
  '',
  'Latest: up 2 days | wired 3.80GB | comp 11.40GB | free 3.67GB | press critical | swap 0.08GB | wk 0/0.00GB',
].join('\n');

/** Above this the toast stops reading as an overlay and starts reading as a
 *  page. The reported card was ~0.8; the cap puts it around a quarter. Kept
 *  loose so the exact rem figure stays tunable. */
const MAX_VIEWPORT_FRACTION = 0.45;

test.describe('toast height cap on mobile', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a long body is clamped and scrolls, instead of covering the pane', async ({ page }) => {
    await gotoWithRetry(page, '/');
    // The toast container only exists while a toast does, so wait for the shell
    // instead: the probe needs the real `--app-header-bottom` and theme tokens.
    await expect(page.locator('.app-header').first()).toBeVisible({ timeout: 15_000 });

    const geom = await page.evaluate((body: string) => {
      // Mirrors the markup `renderToast` emits (Toast.tsx): the message inside a
      // scrolling `.toast-body`, with the actions row and the close X as its
      // SIBLINGS, which is what keeps them reachable under the cap.
      const container = document.createElement('div');
      container.className = 'toast-container';
      const column = document.createElement('div');
      column.className = 'toast-column';
      const toast = document.createElement('div');
      toast.className = 'toast toast-info';
      toast.innerHTML =
        '<div class="toast-body">' +
        '<svg class="toast-icon" viewBox="0 0 24 24"></svg>' +
        '<span class="toast-message"></span>' +
        '</div>' +
        '<div class="toast-actions button-group">' +
        '<button class="action-btn">Open</button>' +
        '</div>' +
        '<button class="icon-btn toast-close" aria-label="Dismiss"></button>';
      (toast.querySelector('.toast-message') as HTMLElement).textContent = body;
      column.appendChild(toast);
      container.appendChild(column);
      document.body.appendChild(container);

      const toastRect = toast.getBoundingClientRect();
      const messageBody = toast.querySelector('.toast-body') as HTMLElement;
      const open = toast.querySelector('.toast-actions .action-btn') as HTMLElement;
      const openRect = open.getBoundingClientRect();
      const result = {
        toastHeight: toastRect.height,
        toastBottom: toastRect.bottom,
        viewportHeight: window.innerHeight,
        // Proves the message really did overflow, so the cap is under test
        // rather than the body simply being short enough to fit.
        bodyOverflows: messageBody.scrollHeight > messageBody.clientHeight + 1,
        bodyScrolls: getComputedStyle(messageBody).overflowY === 'auto',
        openTop: openRect.top,
        openBottom: openRect.bottom,
        openHeight: openRect.height,
      };
      container.remove();
      return result;
    }, LONG_BODY);

    expect(
      geom.bodyOverflows,
      'the probe body did not overflow, so this asserts nothing about the cap',
    ).toBe(true);
    expect(geom.bodyScrolls, 'the clamped overflow must scroll, not be clipped away').toBe(true);

    const fraction = geom.toastHeight / geom.viewportHeight;
    expect(
      fraction,
      `a single toast takes ${(fraction * 100).toFixed(0)}% of the ${geom.viewportHeight}px viewport`,
    ).toBeLessThanOrEqual(MAX_VIEWPORT_FRACTION);

    // On screen in full, and with its action on screen with it: a capped toast
    // whose [Open] is below the fold would just move the problem.
    expect(geom.toastBottom).toBeLessThanOrEqual(geom.viewportHeight + 1);
    expect(geom.openHeight).toBeGreaterThan(0);
    expect(geom.openBottom).toBeLessThanOrEqual(geom.toastBottom + 1);
    expect(geom.openTop).toBeGreaterThanOrEqual(0);
  });
});
