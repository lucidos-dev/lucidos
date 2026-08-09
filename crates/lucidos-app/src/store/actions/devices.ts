import { signal } from '@preact/signals';
import type { Loadable } from '../types';
import { toFailed, setLoadingIfFresh } from '../types';
import type { DeviceInfo } from '../../api/types';
import { registerDevice as apiRegisterDevice, listDevices as apiListDevices, renameDevice as apiRenameDevice, setDevicePush as apiSetDevicePush, deleteDevice as apiDeleteDevice, setPreference } from '../../api/client';
import { showToast, showConfirm } from '../store';
import { errorDetail } from '../../utils/errorDetail';
import { isTauri, registrationUserAgent } from '../../utils/platform';
import { getOrCreateDeviceId } from '../../utils/tauri';
import { generateUuid } from '../../utils/uuid';
// The key is owned by the leaf that also builds the request header, so the id
// this module mints and the id every API call sends cannot drift apart.
import { DEVICE_ID_KEY } from '../../utils/deviceIdHeader';

/** Upper bound on the native-store reconcile so a wedged IPC can't hold the boot
 *  splash — past it, boot proceeds on the existing localStorage value. */
const NATIVE_DEVICE_ID_TIMEOUT_MS = 1500;

export const devices = signal<Loadable<DeviceInfo[]>>({ status: 'not-loaded' });

/** Get or create the device ID for this browser. Used as the `device_id` query
 *  param on per-device API calls (preferences, push subscriptions, etc.). */
export function getDeviceId(): string {
  let id = localStorage.getItem(DEVICE_ID_KEY);
  if (!id) {
    id = generateUuid();
    localStorage.setItem(DEVICE_ID_KEY, id);
  }
  return id;
}

/**
 * Desktop-only: reconcile the per-workspace device id with the durable NATIVE store
 * (a JSON file in the App Support dir) so it survives a DMG reinstall. The
 * WKWebView's `localStorage` is re-bucketed by a new bundle's code signature, which
 * is why the packaged app used to register a brand-new device on every update.
 *
 * Must run BEFORE the first API call — the device id rides every request as
 * `x-lucidos-device-id`, so the synchronous `getDeviceId()` must already return the
 * durable id, not a fresh UUID that would register a spurious device.
 *
 * Candidate = the existing `localStorage` id if present (so an already-installed user
 * keeps their current id with zero churn), else a freshly minted UUID. When storage
 * is empty (a reinstall re-buckets it) we seed the candidate SYNCHRONOUSLY before the
 * await: if the native call is ever slow enough that boot proceeds first (the
 * timeout path in `reconcileDesktopDeviceId`), a concurrent synchronous
 * `getDeviceId()` then returns this same id instead of minting a *different*
 * throwaway UUID and registering a spurious device. The native store returns the
 * stored id for the slug when one exists; we upgrade storage to it only when it
 * differs from the candidate already there. Best-effort: any failure leaves the
 * seeded/existing value in place and resolves, so boot is never blocked.
 *
 * Pure/injectable (deps passed in) so it's unit-testable without `window`/Tauri.
 */
export async function reconcileDeviceIdWithNativeStore(deps: {
  workspace: string;
  getOrCreate: (workspace: string, candidate: string) => Promise<string>;
  storage: Pick<Storage, 'getItem' | 'setItem'>;
  randomUUID: () => string;
}): Promise<void> {
  const { workspace, getOrCreate, storage, randomUUID } = deps;
  try {
    const existing = storage.getItem(DEVICE_ID_KEY);
    const candidate = existing ?? randomUUID();
    if (existing === null) {
      // Seed before the await so `getDeviceId()` can't mint a divergent id mid-boot.
      storage.setItem(DEVICE_ID_KEY, candidate);
    }
    const durable = await getOrCreate(workspace, candidate);
    if (durable && durable !== candidate) {
      storage.setItem(DEVICE_ID_KEY, durable);
    }
  } catch (e) {
    // Best-effort: a hostile/slow/failed storage or native store must never block
    // boot (this whole body, incl. the seed, is guarded so it can't reject the
    // awaited boot path). The app falls back to the seeded/existing localStorage id,
    // exactly as before this durability layer existed.
    console.warn('[Devices] native device-id reconcile failed; using localStorage', e);
  }
}

/**
 * Production entry for the reconcile above: wires the real Tauri command +
 * `localStorage` + `crypto`, bounded by a timeout so a wedged IPC can't hold the
 * boot splash. Call only when isTauri() and a workspace slug is known.
 */
export async function reconcileDesktopDeviceId(workspace: string): Promise<void> {
  const reconcile = reconcileDeviceIdWithNativeStore({
    workspace,
    getOrCreate: getOrCreateDeviceId,
    storage: localStorage,
    randomUUID: () => generateUuid(),
  });
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<void>((resolve) => {
    timer = setTimeout(() => {
      console.warn('[Devices] native device-id reconcile timed out; using localStorage');
      resolve();
    }, NATIVE_DEVICE_ID_TIMEOUT_MS);
  });
  await Promise.race([reconcile, timeout]);
  if (timer) clearTimeout(timer);
}

/** This page load's registration attempt, resolving when it has settled either
 *  way. Set by `registerCurrentDevice`; read via `pendingDeviceRegistration`. */
let registration: Promise<void> | null = null;
/** Whether [`registration`] has already settled, readable SYNCHRONOUSLY. The
 *  send path needs to know without awaiting: see `pendingDeviceRegistration`. */
let registrationHasSettled = false;

/** Register this device with the backend on startup */
export async function registerCurrentDevice(): Promise<void> {
  const deviceId = getDeviceId();
  const attempt = (async () => {
    try {
      // Tag the registered UA with the desktop-app token when in Tauri, so the
      // engine (and the Lucidos Agent's device context) can tell the native
      // desktop client from a browser and give the right notification advice.
      await apiRegisterDevice(deviceId, registrationUserAgent(navigator.userAgent, isTauri()));
    } catch (e) {
      // Telemetry carve-out (.claude/rules/frontend.md): startup probe runs on
      // every page load without user intent. A toast on every transient backend
      // hiccup would be too noisy; the device retries on next reload, and
      // user-facing features that need a registered device (push, per-device
      // prefs) surface their own toasts when they fail.
      console.warn('[Devices] Failed to register device:', e);
    }
  })();
  registrationHasSettled = false;
  // `attempt` swallows its own errors, so this never rejects.
  registration = attempt.finally(() => {
    registrationHasSettled = true;
  });
  return registration;
}

/** How long a send will wait on registration before going anyway. */
const REGISTRATION_WAIT_MS = 3000;

/** What a send must wait for before claiming `mode: 'human'`, or `null` when
 *  there is nothing to wait for.
 *
 *  `useStartup` fires `registerCurrentDevice()` without awaiting it, and the
 *  engine refuses a human-mode chat POST whose device id is not in `devices`
 *  (ADR 0050). A send issued in the first moments of a page load could
 *  therefore race its own registration and come back 403, which for a real
 *  person typing is both wrong and unexplainable.
 *
 *  Returns `null` (rather than an already-resolved promise) once registration
 *  has settled, which is the state every send but the very first is in.
 *  That is not an optimisation: `sendMessage` must dispatch a lone send inside
 *  the caller's synchronous turn, because callers assert on the fetch mock
 *  right after `sendFollowup` without awaiting, and awaiting even a resolved
 *  promise costs a microtask and breaks them. Same shape as the send chain's
 *  `waitForTurn` for the same reason, and pinned by
 *  `dispatches a lone send synchronously, without deferring a microtask`.
 *
 *  The returned promise never rejects and is capped, so a registration that
 *  hangs delays a send by at most [`REGISTRATION_WAIT_MS`]. Going ahead
 *  unregistered is the honest fallback: the send may then be refused, which is
 *  the right answer if this client genuinely cannot register. */
export function pendingDeviceRegistration(): Promise<void> | null {
  if (!registration || registrationHasSettled) return null;
  return Promise.race([
    registration,
    new Promise<void>((resolve) => setTimeout(resolve, REGISTRATION_WAIT_MS)),
  ]);
}

/** Load all devices from the backend */
export async function loadDevices(): Promise<void> {
  setLoadingIfFresh(devices);
  try {
    const res = await apiListDevices();
    devices.value = { status: 'loaded', data: res.devices };
  } catch (e) {
    devices.value = toFailed(e);
  }
}

/** Rename a device */
export async function updateDeviceName(deviceId: string, name: string | null): Promise<void> {
  try {
    await apiRenameDevice(deviceId, name);
    await loadDevices();
  } catch (e) {
    showToast('Failed to rename device: ' + errorDetail(e), 'error');
  }
}

/** Optimistically set one loaded device's `push_enabled`. Returns the previous
 *  value (so the caller can revert on failure), or `undefined` when the list
 *  isn't loaded or the device isn't present. */
function patchDevicePush(deviceId: string, enabled: boolean): boolean | undefined {
  const cur = devices.value;
  if (cur.status !== 'loaded') return undefined;
  let prev: boolean | undefined;
  devices.value = {
    status: 'loaded',
    data: cur.data.map((d) => {
      if (d.id !== deviceId) return d;
      prev = d.push_enabled;
      return { ...d, push_enabled: enabled };
    }),
  };
  return prev;
}

/** Toggle push for a device — sets both the per-device preference and devices.push_enabled.
 *  The toggle is a controlled checkbox bound to `device.push_enabled`, so it can't
 *  move until that signal updates. Flip it optimistically first so the slider
 *  responds instantly, then reconcile with the server (reverting on failure)
 *  instead of leaving the user staring at an unmoved toggle for two round-trips. */
export async function toggleDevicePush(deviceId: string, enabled: boolean): Promise<void> {
  const prev = patchDevicePush(deviceId, enabled);
  try {
    await Promise.all([
      apiSetDevicePush(deviceId, enabled),
      setPreference('push_notifications', enabled ? 'enabled' : 'declined', deviceId),
    ]);
    await loadDevices();
  } catch (e) {
    if (prev !== undefined) patchDevicePush(deviceId, prev);
    showToast('Failed to update push setting: ' + errorDetail(e), 'error');
  }
}

/** Turn push OFF for several devices in one go, with a single list reload at the
 *  end: `toggleDevicePush` in a loop refetches the whole device list once per
 *  device. Same optimistic flip so every slider moves together.
 *
 *  On failure it reconciles against the server rather than reverting its own
 *  guesses, because a partial failure leaves some devices genuinely off and
 *  restoring the pre-flip state for all of them would show the user something
 *  untrue. */
export async function disablePushForDevices(deviceIds: string[]): Promise<void> {
  if (deviceIds.length === 0) return;
  for (const id of deviceIds) patchDevicePush(id, false);
  // `allSettled`, never `all`: `all` rejects on the FIRST failure while the
  // other writes are still in flight, so the reload below would read the server
  // mid-batch and reconcile a device back to "on" that goes off a moment later,
  // with nothing to correct it until something else reloads the list.
  const results = await Promise.allSettled(
    deviceIds.flatMap((id) => [
      apiSetDevicePush(id, false),
      setPreference('push_notifications', 'declined', id),
    ]),
  );
  const failure = results.find((r): r is PromiseRejectedResult => r.status === 'rejected');
  if (failure) {
    showToast('Failed to turn push off: ' + errorDetail(failure.reason), 'error');
  }
  await loadDevices();
}

/** Remove a device */
export async function removeDevice(deviceId: string): Promise<void> {
  const ok = await showConfirm('Remove this device? Its push subscription and preferences will be deleted.', 'Remove');
  if (!ok) return;
  try {
    await apiDeleteDevice(deviceId);
    await loadDevices();
  } catch (e) {
    showToast('Failed to remove device: ' + errorDetail(e), 'error');
  }
}
