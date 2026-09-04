/**
 * `apiRequest` borrows the PAGE's device id for its calls (helpers.ts). The app
 * mints that id into `localStorage` at boot but registers it server-side
 * fire-and-forget, so a mutating call can fire before the id reaches `devices`.
 * `api::mutating_gate` refuses an id that names no row (ADR 0169), so the call
 * used to come back 401.
 *
 * That is the mobile-webkit notifications.spec.ts flake. A loaded host widens
 * the window between "id in localStorage" and "id in devices". So the seeding
 * `postNotification` beat the registration and failed on its first attempt.
 *
 * This pins the fix. It forces the exact state by replacing the app's id with
 * one it never registered, then makes a mutating `apiRequest` call. Without the
 * fix that is a guaranteed 401; with it, `apiRequest` registers the borrowed id
 * first. Deterministic on any host, so it holds where the flake did not repeat.
 */
import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, navigateToApp } from './helpers';

test.describe('Harness apiRequest registers a borrowed device id', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a mutating call with an unregistered borrowed page id is not refused', async ({ page }) => {
    await navigateToApp(page);

    // Replace the id the app registered with one it never will, so the id the
    // harness borrows is guaranteed absent from `devices`.
    const freshId = randomUUID();
    await page.evaluate((id) => localStorage.setItem('lucidos-device-id', id), freshId);

    // Set the pinned default, so the mutation itself leaks no state. The point
    // is the status code: a borrowed id the app never registered must still be
    // accepted, because apiRequest registers it before the call.
    const res = await apiRequest(page).put('/api/v1/preferences?key=mobile_header_sticky', {
      data: { value: 'true' },
    });
    expect(res.ok(), `PUT /api/v1/preferences -> ${res.status()}`).toBeTruthy();
  });
});
