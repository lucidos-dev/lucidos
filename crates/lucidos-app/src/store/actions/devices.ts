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
import { DEVICE_ID_KEY, getRelayedDeviceId, setRelayedDeviceId } from '../../utils/deviceIdHeader';

const NATIVE_DEVICE_ID_TIMEOUT_MS = 1500;

const GATEWAY_DEVICE_ID_TIMEOUT_MS = 4000;

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
      storage.setItem(DEVICE_ID_KEY, candidate);
    }
    const durable = await getOrCreate(workspace, candidate);
    if (durable && durable !== candidate) {
      storage.setItem(DEVICE_ID_KEY, durable);
    }
  } catch (e) {
    console.warn('[Devices] native device-id reconcile failed; using localStorage', e);
  }
}

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

export async function adoptGatewayDeviceIdentity(deps: {
  session: () => Promise<{ device_id?: string }>;
  storage: Pick<Storage, 'getItem' | 'setItem'>;
  handOver?: (oldId: string, newId: string) => Promise<unknown>;
  previousId?: () => Promise<string | null>;
  remember?: (id: string) => Promise<unknown>;
}): Promise<boolean> {
  const { session, storage, handOver, previousId, remember } = deps;
  const gatewayId = (await session()).device_id?.trim();
  if (!gatewayId) return false;

  const stored = storage.getItem(DEVICE_ID_KEY);
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

async function readPreviousDeviceId(
  storage: Pick<Storage, 'getItem'>,
  previousId?: () => Promise<string | null>,
): Promise<string | null> {
  return storage.getItem(DEVICE_ID_KEY) ?? (previousId ? await previousId() : null);
}

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
  return withDeadline(adopt, GATEWAY_DEVICE_ID_TIMEOUT_MS, false);
}

async function handOverWithBreadcrumb(oldId: string, newId: string): Promise<HandOverOutcome | undefined> {
  try {
    return (await handOverDevice(oldId, newId)).outcome;
  } catch (e) {
    // BEST-EFFORT: telemetry carve-out (`.claude/rules/frontend.md`). Boot runs this with no
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

const RELAYED_DEVICE_ID_KEY = 'lucidos-relayed-device-id';

export async function adoptRelayedDeviceIdentity(deps: {
  caller: () => Promise<{ device_id: string | null }>;
  storage: Pick<Storage, 'getItem' | 'setItem'>;
  handOver: (oldId: string, newId: string) => Promise<HandOverOutcome | undefined>;
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

function relayedHandOverCandidates(callerId: string, bare: string | null): string[] {
  if (!bare) return [];
  const sep = callerId.lastIndexOf(':');
  const relayed = sep > 0 ? `${callerId.slice(0, sep)}:${bare}` : null;
  return relayed && relayed !== bare ? [relayed, bare] : [bare];
}

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

export interface DeviceRoute {
  workspace: string | null;
  relayed: boolean;
}

export async function adoptRoutedDeviceId(route: DeviceRoute): Promise<boolean> {
  if (!route.relayed) return await adoptGatewayDeviceId(route.workspace);
  const barePrevious = await readBarePreviousDeviceId(route.workspace);
  if (!(await adoptRelayedDeviceId(barePrevious))) return false;
  return await adoptGatewayDeviceId(route.workspace, true);
}

async function readBarePreviousDeviceId(workspace: string | null): Promise<string | null> {
  const native = isTauri() && workspace !== null ? () => previousDeviceId(workspace) : undefined;
  const read = readPreviousDeviceId(localStorage, native).catch((e: unknown) => {
    console.warn('[Devices] could not read the previous device id', e);
    return null;
  });
  return withDeadline(read, NATIVE_DEVICE_ID_TIMEOUT_MS, null);
}

export async function registerCurrentDevice(): Promise<void> {
  const deviceId = getDeviceId();
  const attempt = (async () => {
    try {
      await apiRegisterDevice(deviceId, registrationUserAgent(navigator.userAgent, isTauri()));
    } catch (e) {
      console.warn('[Devices] Failed to register device:', e);
    }
  })();
  return trackDeviceRegistration(attempt);
}

export const devicesSlice: SliceOf<'devices'> = {
  begin: () => setLoadingIfFresh(devices),
  apply: (res) => { devices.value = { status: 'loaded', data: res.devices }; },
  fail: (e) => { devices.value = toFailed(e); },
};

export function loadDevices(): Promise<void> {
  return loadSlice('devices', devicesSlice, () => apiListDevices());
}

export async function updateDeviceName(deviceId: string, name: string | null): Promise<void> {
  try {
    await apiRenameDevice(deviceId, name);
    await loadDevices();
  } catch (e) {
    showToast('Failed to rename device: ' + errorDetail(e), 'error');
  }
}

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

export async function disablePushForDevices(deviceIds: string[]): Promise<void> {
  if (deviceIds.length === 0) return;
  for (const id of deviceIds) patchDevicePush(id, false);
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
