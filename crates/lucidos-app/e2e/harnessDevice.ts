/**
 * The device the browser harness speaks as when it is not the page.
 *
 * `api::mutating_gate` refuses a mutating call carrying no identity (ADR 0169),
 * and three of Playwright's four request surfaces carry none: the `request`
 * fixture, a `request.newContext()` a spec builds, and `page.request`. None of
 * them reads the page's `localStorage`, which is where the app keeps its id.
 *
 * The harness is a legitimate external client, so it registers a device and
 * presents it, rather than the gate being narrowed. Same answer the API suite's
 * `user_client` gives, for the same reason.
 *
 * `page.request` is the exception and does NOT use this. `helpers.apiRequest`
 * borrows the PAGE's own id there, so a harness call names the device the test
 * drives, and device-scoped preferences stay per-test.
 */

import type { APIRequestContext } from '@playwright/test';

/** Stable across the run. Registration is an upsert, and `DeviceRegistered`
 *  fires only on the genuine first insert. */
export const HARNESS_DEVICE_ID = 'e2e-browser-harness';

/** One registration per device id per worker process, however many contexts ask
 *  for it. Keyed by id so the harness id and a borrowed page id are tracked
 *  apart. */
const registrations = new Map<string, Promise<void>>();

/**
 * Idempotent `POST /api/v1/devices/register`, the one bootstrap route the
 * mutating gate exempts, which is what lets this run through the very context it
 * is vouching for.
 *
 * Never throws. A failure here should surface as the actual request's refusal,
 * with its own message, rather than as a fixture that could not set itself up.
 */
function ensureRegistered(
  ctx: APIRequestContext,
  deviceId: string,
  userAgent?: string,
): Promise<void> {
  let inFlight = registrations.get(deviceId);
  if (!inFlight) {
    const data: { device_id: string; user_agent?: string } = { device_id: deviceId };
    if (userAgent) data.user_agent = userAgent;
    inFlight = ctx
      .post('/api/v1/devices/register', { data })
      .then(() => undefined)
      .catch(() => undefined);
    registrations.set(deviceId, inFlight);
  }
  return inFlight;
}

/**
 * Put [`HARNESS_DEVICE_ID`] in the `devices` table, so the header naming it is
 * evidence rather than a string anyone could type.
 */
export function registerHarnessDevice(ctx: APIRequestContext): Promise<void> {
  return ensureRegistered(ctx, HARNESS_DEVICE_ID, 'lucidos-e2e-browser/1');
}

/**
 * Ensure the PAGE's own device id is in `devices` before the harness borrows it
 * for a mutating call.
 *
 * The app mints the id into `localStorage` synchronously at boot but registers
 * it server-side fire-and-forget (`useStartup` does not await
 * `registerCurrentDevice`). `api::mutating_gate` refuses a device id that names
 * no row (ADR 0169). So a mutating call fired right after navigation can beat
 * the registration and come back 401. That is the mobile-webkit
 * notifications.spec.ts flake, where a loaded host widens the window.
 *
 * No user_agent, so the upsert's COALESCE keeps whatever the app registered.
 */
export function ensurePageDeviceRegistered(
  ctx: APIRequestContext,
  deviceId: string,
): Promise<void> {
  return ensureRegistered(ctx, deviceId);
}
