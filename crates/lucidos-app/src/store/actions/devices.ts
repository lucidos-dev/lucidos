import { signal } from '@preact/signals';
import type { Loadable } from '../types';
import { toFailed, setLoadingIfFresh } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';
import type { DeviceInfo } from '../../api/types';
import type { HandOverOutcome } from '../../api/client/settings';
import { registerDevice as apiRegisterDevice, listDevices as apiListDevices, renameDevice as apiRenameDevice, setDevicePush as apiSetDevicePush, deleteDevice as apiDeleteDevice, handOverDevice, getCallerDevice, setPreference } from '../../api/client';
import { pairingSession } from '../../api/client/pairing';
import { showToast, showConfirm } from '../store';
import { errorDetail } from '../../utils/errorDetail';
import { postClientLog } from '../../utils/clientLog';
import { isTauri, registrationUserAgent } from '../../utils/platform';
import { trackDeviceRegistration } from '../../utils/deviceRegistration';
import { getOrCreateDeviceId, previousDeviceId, rememberDeviceId } from '../../utils/tauri';
import { generateUuid } from '../../utils/uuid';
import { deviceName } from '../../utils/deviceFriendlyName';
import { pairedDevices, pairedRows } from './pairedDevices';
// The key is owned by the leaf that also builds the request header, so the id
// this module mints and the id every API call sends cannot drift apart.
import { DEVICE_ID_KEY, getRelayedDeviceId, setRelayedDeviceId } from '../../utils/deviceIdHeader';

/** Upper bound on the native-store reconcile so a wedged IPC can't hold the boot
 *  splash — past it, boot proceeds on the existing localStorage value. */
const NATIVE_DEVICE_ID_TIMEOUT_MS = 1500;

/** Upper bound on the gateway-identity adoption, for the same reason and with a
 *  wider window: it is a loopback HTTP round trip rather than an IPC call, and
 *  a migration load spends a second one on the hand-over.
 *
 *  Bounded because this is the first await every client pays on boot, browser
 *  and PWA included. A gateway that accepts the connection and stalls would
 *  hold the splash forever. Timing out costs only a page load, since nothing
 *  commits until the row has moved. */
const GATEWAY_DEVICE_ID_TIMEOUT_MS = 4000;

/** Race `work` against `ms`, resolving to `onTimeout` if it wins.
 *
 *  Clears the timer either way, so a resolved boot step does not keep the event
 *  loop warm for the rest of the window. */
async function withDeadline<T>(work: Promise<T>, ms: number, onTimeout: T): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<T>((resolve) => {
    timer = setTimeout(() => resolve(onTimeout), ms);
  });
  try {
    return await Promise.race([work, deadline]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

export const devices = signal<Loadable<DeviceInfo[]>>({ status: 'not-loaded' });

/**
 * How a turn names the device it came from, by [`deviceName`].
 *
 * An event stores the device's id only, so the name comes from the two lists
 * startup loads: the engine's devices and the gateway's pairings. Those are the
 * rows the Devices page reads, so both name a device alike, and a rename
 * reaches every older turn. The reader's own device says so, as its row does.
 * A turn recorded under an id a hand-over replaced takes the device it became.
 *
 * Empty until both lists land, rather than flashing a lesser name first.
 */
export function turnDeviceName(deviceId: string, options?: { knownOnly?: boolean }): string {
  const pending = (s: string) => s === 'not-loaded' || s === 'loading';
  const list = devices.value;
  if (pending(list.status) || pending(pairedDevices.value.status)) return '';
  const device = list.status === 'loaded'
    ? list.data.find((d) => d.id === deviceId) ?? list.data.find((d) => d.replaced_ids?.includes(deviceId))
    : undefined;
  const id = device?.id ?? deviceId;
  const paired = pairedRows(pairedDevices.value)?.find((p) => p.id === id);
  if (options?.knownOnly && !device && !paired) return '';
  const name = deviceName(id, device, paired);
  return id === getDeviceId() ? `${name} (this device)` : name;
}

/** Get or create the device ID for this browser. Used as the `device_id` query
 *  param on per-device API calls (preferences, push subscriptions, etc.). */
export function getDeviceId(): string {
  const relayed = getRelayedDeviceId();
  if (relayed) return relayed;
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

/**
 * Take the device id the *workspace gateway* authenticated us as, and move this
 * workspace's row onto it.
 *
 * One identity instead of two. The gateway already minted an id when this
 * browser paired, and the engine keys push, preferences and attribution on
 * whatever `x-lucidos-device-id` says. Writing the gateway's id into the same
 * `localStorage` slot keeps every synchronous `getDeviceId()` caller unchanged.
 *
 * Returns whether a gateway answered. `false` means nothing was adopted, and
 * the `localStorage` id stays authoritative: reaching an engine port directly
 * there is no paired-devices list either, so nothing can be confused with it.
 *
 * Every write happens AFTER `handOver` resolves, so a failure forgets nothing
 * and the next load retries. `handOver` must REJECT for that to hold, which is
 * why the engine answers a failed transaction with a 500. Rationale in
 * `docs/plans/2026-08-22-one-device-identity-minted-at-the-gateway.md`.
 *
 * A relayed route passes no `handOver`. The engine sees that route's
 * `<pairing id>:<device id>` header, so it refuses a hand-over of bare ids.
 * The relayed adoption moves the row there instead.
 *
 * Pure/injectable, so every branch is testable without a DOM or a gateway.
 */
export async function adoptGatewayDeviceIdentity(deps: {
  session: () => Promise<{ device_id?: string }>;
  storage: Pick<Storage, 'getItem' | 'setItem'>;
  handOver?: (oldId: string, newId: string) => Promise<unknown>;
  /** Desktop only, and READ-ONLY: the id this window last used. A reinstall
   *  empties `localStorage`, so this is the only memory left. */
  previousId?: () => Promise<string | null>;
  /** Desktop only: commit the new id natively, once the row has moved. */
  remember?: (id: string) => Promise<unknown>;
}): Promise<boolean> {
  const { session, storage, handOver, previousId, remember } = deps;
  const gatewayId = (await session()).device_id?.trim();
  if (!gatewayId) return false;

  const stored = storage.getItem(DEVICE_ID_KEY);
  // The steady state, on every load after the first. Nothing to read, move or
  // write, so it costs one comparison.
  if (stored === gatewayId) return true;

  if (handOver) {
    const previous = await readPreviousDeviceId(storage, previousId);
    if (previous && previous !== gatewayId) {
      await handOver(previous, gatewayId);
    }
  }
  storage.setItem(DEVICE_ID_KEY, gatewayId);
  if (remember) await remember(gatewayId);
  return true;
}

/** The bare id this page last used. This webview's own storage is the more
 *  direct answer, so it wins. The native store answers only when storage has
 *  been emptied under us, as a desktop update or reinstall does. */
async function readPreviousDeviceId(
  storage: Pick<Storage, 'getItem'>,
  previousId?: () => Promise<string | null>,
): Promise<string | null> {
  return storage.getItem(DEVICE_ID_KEY) ?? (previousId ? await previousId() : null);
}

/**
 * Production entry for the adoption above. Returns whether the gateway named
 * us, so the caller knows whether the desktop native reconcile still applies.
 *
 * Best-effort in the telemetry sense (`.claude/rules/frontend.md`): it runs on
 * every load with no user intent behind it. A failure re-runs on the next one,
 * because nothing is committed until the hand-over lands. A toast here would
 * fire on every direct-engine page load, where no gateway is meant to answer.
 */
export async function adoptGatewayDeviceId(workspace: string | null, relayed = false): Promise<boolean> {
  const onDesktop = isTauri() && workspace !== null;
  const adopt = (async () => {
    try {
      return await adoptGatewayDeviceIdentity({
        session: pairingSession,
        storage: localStorage,
        handOver: relayed ? undefined : handOverWithBreadcrumb,
        previousId: onDesktop ? () => previousDeviceId(workspace) : undefined,
        remember: onDesktop ? (id) => rememberDeviceId(workspace, id) : undefined,
      });
    } catch (e) {
      console.warn('[Devices] could not adopt the gateway device id', e);
      return false;
    }
  })();
  // `false` on a timeout, which is honest: nothing was adopted, and on desktop
  // it lets the native reconcile answer instead.
  return withDeadline(adopt, GATEWAY_DEVICE_ID_TIMEOUT_MS, false);
}

/** The engine's hand-over, leaving a breadcrumb when it rejects. */
async function handOverWithBreadcrumb(oldId: string, newId: string): Promise<HandOverOutcome | undefined> {
  try {
    return (await handOverDevice(oldId, newId)).outcome;
  } catch (e) {
    // Telemetry carve-out (`.claude/rules/frontend.md`). Boot runs this with no
    // user intent and the next load retries it, so a toast would fire on a
    // hiccup nobody asked about. A toast is also the wrong surface for the
    // failure that matters: a hand-over refused on EVERY load strands the
    // migration, and that reached the user as two rows per device. The
    // breadcrumb puts it in `engine.log`, where it is greppable without a console.
    postClientLog('devices', 'hand_over_failed', {
      old_device_id: oldId,
      device_id: newId,
      reason: errorDetail(e),
    });
    throw e;
  }
}

/** The caller id this relayed route last adopted. Workspace storage prefixes
 *  every non-global key with the workspace id, which is `<slug>@<gateway>` on a
 *  relayed route, so this plain key is already per route. */
const RELAYED_DEVICE_ID_KEY = 'lucidos-relayed-device-id';

/**
 * Take the id the relaying gateway namespaces this browser's device to, and
 * move the plain row onto it.
 *
 * A remote gateway asserts `<gateway>:<device>` to the engine, so the engine
 * keeps the row under that id. A page that sent its plain id would register a
 * second device, and its push, preferences and attribution would split.
 * The id is held in memory for this page load (`setRelayedDeviceId`), never in
 * `lucidos-device-id`, which the page's direct routes still own.
 *
 * Returns whether a caller id was named. Like the gateway adoption, the
 * override and the key are set AFTER `handOver` resolves. Until then the page
 * keeps its old id. So no row is registered under the caller id before the old
 * row moves there, and a failure retries on the next load.
 *
 * The row lives under the id this route last adopted. A changed caller id (a
 * re-paired gateway) therefore hands over from that id, not the plain one.
 *
 * With no adopted id (an update emptied storage), the old row's id is guessed
 * from the bare previous id: first in its relayed form, then bare. A guess with
 * no row answers `no-such-device`, so the next one is tried and none is harmful.
 *
 * Pure/injectable, so every branch is testable without a DOM or a gateway.
 */
export async function adoptRelayedDeviceIdentity(deps: {
  caller: () => Promise<{ device_id: string | null }>;
  storage: Pick<Storage, 'getItem' | 'setItem'>;
  handOver: (oldId: string, newId: string) => Promise<HandOverOutcome | undefined>;
  /** The bare id this page used before the gateway adoption replaced it. */
  barePrevious?: string | null;
}): Promise<boolean> {
  const { caller, storage, handOver } = deps;
  const callerId = (await caller()).device_id?.trim();
  if (!callerId) return false;

  const adopted = storage.getItem(RELAYED_DEVICE_ID_KEY);
  if (adopted !== callerId) {
    const bare = deps.barePrevious ?? storage.getItem(DEVICE_ID_KEY);
    const candidates = adopted ? [adopted] : relayedHandOverCandidates(callerId, bare);
    for (const oldId of candidates) {
      if (oldId === callerId) continue;
      if ((await handOver(oldId, callerId)) !== 'no-such-device') break;
    }
    storage.setItem(RELAYED_DEVICE_ID_KEY, callerId);
  }
  setRelayedDeviceId(callerId);
  return true;
}

/** Where the old row may live, most likely first: under the caller's pairing
 *  prefix (everything before its last `:`, ADR 0591), then under the bare id. */
function relayedHandOverCandidates(callerId: string, bare: string | null): string[] {
  if (!bare) return [];
  const sep = callerId.lastIndexOf(':');
  const relayed = sep > 0 ? `${callerId.slice(0, sep)}:${bare}` : null;
  return relayed && relayed !== bare ? [relayed, bare] : [bare];
}

/**
 * Production entry for the adoption above, for a relayed route only. Same
 * best-effort carve-out and deadline as `adoptGatewayDeviceId`: it runs on
 * every load with no user intent, and the next load retries.
 */
export async function adoptRelayedDeviceId(barePrevious: string | null): Promise<boolean> {
  const adopt = (async () => {
    try {
      return await adoptRelayedDeviceIdentity({
        caller: getCallerDevice,
        storage: localStorage,
        handOver: handOverWithBreadcrumb,
        barePrevious,
      });
    } catch (e) {
      console.warn('[Devices] could not adopt the relayed device id', e);
      return false;
    }
  })();
  return withDeadline(adopt, GATEWAY_DEVICE_ID_TIMEOUT_MS, false);
}

/** Where this page reaches its workspace, which decides how it adopts an id. */
export interface DeviceRoute {
  /** The workspace id, or `null` when the page does not know it. */
  workspace: string | null;
  /** The workspace sits on a remote gateway, which relays a namespaced id. */
  relayed: boolean;
}

/** Adopt the id the gateway names this page by, and the relayed id on a
 *  relayed route. Returns whether the workspace gateway named us.
 *
 *  On a relayed route only the relayed adoption hands over. The gateway
 *  adoption overwrites the bare previous id, locally and natively, so it runs
 *  only once the relayed row has moved. A failed hand-over then retries from
 *  the same old id on the next load. */
export async function adoptRoutedDeviceId(route: DeviceRoute): Promise<boolean> {
  if (!route.relayed) return await adoptGatewayDeviceId(route.workspace);
  const barePrevious = await readBarePreviousDeviceId(route.workspace);
  if (!(await adoptRelayedDeviceId(barePrevious))) return false;
  return await adoptGatewayDeviceId(route.workspace, true);
}

/** Production read of the bare previous id, bounded like the native reconcile.
 *  A failure only costs the after-update guess, so it never blocks boot. */
async function readBarePreviousDeviceId(workspace: string | null): Promise<string | null> {
  const native = isTauri() && workspace !== null ? () => previousDeviceId(workspace) : undefined;
  const read = readPreviousDeviceId(localStorage, native).catch((e: unknown) => {
    console.warn('[Devices] could not read the previous device id', e);
    return null;
  });
  return withDeadline(read, NATIVE_DEVICE_ID_TIMEOUT_MS, null);
}

/** Register this device with the backend, at page load */
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
  // `attempt` swallows its own errors, so it never rejects. Every mutation the
  // app sends waits on it while it runs: see `registrationToAwait`.
  return trackDeviceRegistration(attempt);
}

/** Load all devices from the backend */
export const devicesSlice: SliceOf<'devices'> = {
  begin: () => setLoadingIfFresh(devices),
  apply: (res) => { devices.value = { status: 'loaded', data: res.devices }; },
  fail: (e) => { devices.value = toFailed(e); },
};

export function loadDevices(): Promise<void> {
  return loadSlice('devices', devicesSlice, () => apiListDevices());
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
  const ok = await showConfirm('Remove this device? Its push subscription and preferences will be deleted.', 'Remove', { variant: 'danger' });
  if (!ok) return;
  try {
    await apiDeleteDevice(deviceId);
    await loadDevices();
  } catch (e) {
    showToast('Failed to remove device: ' + errorDetail(e), 'error');
  }
}
