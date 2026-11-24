
import type { APIRequestContext } from '@playwright/test';

export const HARNESS_DEVICE_ID = 'e2e-browser-harness';

const registrations = new Map<string, Promise<void>>();

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

export function registerHarnessDevice(ctx: APIRequestContext): Promise<void> {
  return ensureRegistered(ctx, HARNESS_DEVICE_ID, 'lucidos-e2e-browser/1');
}

export function ensurePageDeviceRegistered(
  ctx: APIRequestContext,
  deviceId: string,
): Promise<void> {
  return ensureRegistered(ctx, deviceId);
}
