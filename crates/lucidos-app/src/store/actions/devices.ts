import { signal } from '@preact/signals';
import type { Loadable } from '../types';
import { toFailed, setLoadingIfFresh } from '../types';
import type { DeviceInfo } from '../../api/types';
import { registerDevice as apiRegisterDevice, listDevices as apiListDevices, renameDevice as apiRenameDevice, setDevicePush as apiSetDevicePush, deleteDevice as apiDeleteDevice, setPreference } from '../../api/client';
import { showToast, showConfirm } from '../store';
import { errorDetail } from '../../utils/errorDetail';

const DEVICE_ID_KEY = 'lucidos-device-id';

export const devices = signal<Loadable<DeviceInfo[]>>({ status: 'not-loaded' });

/** Get or create the device ID for this browser. Used as the `device_id` query
 *  param on per-device API calls (preferences, push subscriptions, etc.). */
export function getDeviceId(): string {
  let id = localStorage.getItem(DEVICE_ID_KEY);
  if (!id) {
    id = crypto.randomUUID();
    localStorage.setItem(DEVICE_ID_KEY, id);
  }
  return id;
}

/** Register this device with the backend on startup */
export async function registerCurrentDevice(): Promise<void> {
  const deviceId = getDeviceId();
  try {
    await apiRegisterDevice(deviceId, navigator.userAgent);
  } catch (e) {
    // Telemetry carve-out (.claude/rules/frontend.md): startup probe runs on
    // every page load without user intent. A toast on every transient backend
    // hiccup would be too noisy; the device retries on next reload, and
    // user-facing features that need a registered device (push, per-device
    // prefs) surface their own toasts when they fail.
    console.warn('[Devices] Failed to register device:', e);
  }
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
