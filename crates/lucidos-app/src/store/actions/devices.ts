import { signal } from '@preact/signals';
import type { Loadable } from '../types';
import { toFailed } from '../types';
import type { DeviceInfo } from '../../api/types';
import { registerDevice as apiRegisterDevice, listDevices as apiListDevices, renameDevice as apiRenameDevice, setDevicePush as apiSetDevicePush, deleteDevice as apiDeleteDevice, setPreference } from '../../api/client';
import { showToast, showConfirm } from '../store';
import { errorDetail } from '../../utils/errorDetail';

const DEVICE_ID_KEY = 'lucidos-device-id';

export const devices = signal<Loadable<DeviceInfo[]>>({ status: 'not-loaded' });

/** Get or create the device ID for this browser */
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
    console.error('[Devices] Failed to register device:', e);
  }
}

/** Load all devices from the backend */
export async function loadDevices(): Promise<void> {
  devices.value = { status: 'loading' };
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

/** Toggle push for a device — sets both the per-device preference and devices.push_enabled */
export async function toggleDevicePush(deviceId: string, enabled: boolean): Promise<void> {
  try {
    await Promise.all([
      apiSetDevicePush(deviceId, enabled),
      setPreference('push_notifications', enabled ? 'enabled' : 'declined', deviceId),
    ]);
    await loadDevices();
  } catch (e) {
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
