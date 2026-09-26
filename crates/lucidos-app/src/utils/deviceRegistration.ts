/**
 * This page load's device registration, as a request to the engine waits on it.
 *
 * A leaf for the same reason as `deviceIdHeader.ts`: the API client and the
 * store both need it, and `store/actions/devices.ts` imports the client.
 *
 * `startClient` registers the device without awaiting it, and the engine refuses
 * a mutation from a device it cannot resolve (ADR 0169). So a first keystroke,
 * a presence report or a first send could race the registration and come back
 * 401. A refused compose start cleared the reader's draft.
 */

let registration: Promise<void> | null = null;
let settled = false;

/** How long a request waits on registration before going anyway. */
const REGISTRATION_WAIT_MS = 3000;

/** Record this page load's registration attempt. `attempt` must not reject. */
export function trackDeviceRegistration(attempt: Promise<void>): Promise<void> {
  settled = false;
  const tracked = attempt.finally(() => {
    settled = true;
  });
  registration = tracked;
  return tracked;
}

/** What a request must wait for before the engine can identify its device, or
 *  `null` when there is nothing to wait for.
 *
 *  Only a mutation needs an identity, and the registration's own request
 *  cannot wait on itself. Once registration settles this returns `null`, not a
 *  resolved promise, so a caller dispatches inside its synchronous turn. The
 *  send path relies on that. The wait is capped, so a hung registration delays
 *  a request by at most `REGISTRATION_WAIT_MS`. Going ahead unregistered then
 *  is the honest fallback. */
export function registrationToAwait(url: string, method = 'GET'): Promise<void> | null {
  if (!registration || settled) return null;
  const verb = method.toUpperCase();
  if (verb === 'GET' || verb === 'HEAD' || url.endsWith('/devices/register')) return null;
  return Promise.race([
    registration,
    new Promise<void>((resolve) => setTimeout(resolve, REGISTRATION_WAIT_MS)),
  ]);
}
