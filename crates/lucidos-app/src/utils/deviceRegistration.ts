
let registration: Promise<void> | null = null;
let settled = false;

const REGISTRATION_WAIT_MS = 3000;

export function trackDeviceRegistration(attempt: Promise<void>): Promise<void> {
  settled = false;
  const tracked = attempt.finally(() => {
    settled = true;
  });
  registration = tracked;
  return tracked;
}

export function registrationToAwait(url: string, method = 'GET'): Promise<void> | null {
  if (!registration || settled) return null;
  const verb = method.toUpperCase();
  if (verb === 'GET' || verb === 'HEAD' || url.endsWith('/devices/register')) return null;
  return Promise.race([
    registration,
    new Promise<void>((resolve) => setTimeout(resolve, REGISTRATION_WAIT_MS)),
  ]);
}
