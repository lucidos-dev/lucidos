
import type { DeviceInfo } from '../../api/types';
import type { PairedDevice } from '../../api/client/control';
import { deviceDetails } from './pairedDevices';
import { deviceName } from '../../utils/deviceFriendlyName';

export interface DeviceRowModel {
  id: string;
  device?: DeviceInfo;
  paired?: PairedDevice;
  isCurrent: boolean;
}

export function buildDeviceRows(
  devices: DeviceInfo[],
  paired: PairedDevice[] | null,
  currentId: string,
): DeviceRowModel[] {
  const pairedById = new Map((paired ?? []).map((p) => [p.id, p]));
  const rows: DeviceRowModel[] = devices.map((device) => ({
    id: device.id,
    device,
    paired: pairedById.get(device.id),
    isCurrent: device.id === currentId,
  }));
  const seen = new Set(devices.map((d) => d.id));
  for (const p of paired ?? []) {
    if (seen.has(p.id)) continue;
    rows.push({ id: p.id, paired: p, isCurrent: p.id === currentId });
  }
  return rows.sort((a, b) => {
    if (a.isCurrent !== b.isCurrent) return a.isCurrent ? -1 : 1;
    const pairedRank = Number(Boolean(b.paired)) - Number(Boolean(a.paired));
    if (pairedRank !== 0) return pairedRank;
    return Number(Boolean(b.device?.push_enabled)) - Number(Boolean(a.device?.push_enabled));
  });
}

export function isUnusedDevice(row: DeviceRowModel): boolean {
  return !row.isCurrent && !row.paired && !row.device?.push_enabled;
}

const NO_ENGINE_ROW = 'Not set up in this workspace';

export function deviceRowName(row: DeviceRowModel): string {
  return deviceName(row.id, row.device, row.paired);
}

export function submittedDeviceName(
  stored: string | null,
  displayName: string,
  typed: string,
): string | null | undefined {
  const trimmed = typed.trim();
  const next = trimmed === displayName ? stored : (trimmed || null);
  return next === stored ? undefined : next;
}

export function deviceRowSummary(
  row: DeviceRowModel,
  describeUserAgent: (ua?: string) => string,
  gatewayAnswered: boolean,
  now: number,
): string[] {
  const clauses = [describeUserAgent(row.device?.user_agent ?? undefined)];
  if (!row.device) {
    clauses.push(NO_ENGINE_ROW);
  }
  if (gatewayAnswered) {
    clauses.push(row.paired ? pairedClause(row.paired, now) : 'Not paired');
  }
  return clauses.filter(Boolean);
}

function pairedClause(paired: PairedDevice, now: number): string {
  const details = deviceDetails(paired, now);
  return details || 'Paired';
}
