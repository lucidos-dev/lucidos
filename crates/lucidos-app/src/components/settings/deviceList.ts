/**
 * One device list, from the two halves that used to be separate lists.
 *
 * Settings had a *device* in two places and they were not the same device. The
 * gateway minted one id when a browser paired, and the engine minted another in
 * `localStorage`. Both ids now come from the gateway, so the two rows can be
 * joined and shown as one thing.
 *
 * The halves still answer different questions, and the row keeps both:
 *
 * - **Pairing** is machine-global. It decides whether this client may reach the
 *   machine over the network at all, on every workspace. Revoke lives here.
 * - **The engine row** is per workspace. It decides where push goes, which
 *   per-device preferences apply, and who to credit on an actor chip.
 *
 * So the join is OUTER on both sides, and neither side missing is an error. A
 * device paired from another workspace has never opened this one, and a client
 * on a direct engine port never went through the gateway.
 */

import type { DeviceInfo } from '../../api/types';
import type { PairedDevice } from '../../api/client/control';
import { deviceDetails } from './pairedDevices';

/** One device, as the list shows it. */
export interface DeviceRowModel {
  id: string;
  /** The engine's row, absent for a device that never opened this workspace. */
  device?: DeviceInfo;
  /** The gateway's row, absent for a client that never paired. */
  paired?: PairedDevice;
  /** Is this the client reading the page? */
  isCurrent: boolean;
}

/**
 * Join the two halves into one list, current device first.
 *
 * After that, paired devices lead, then push-enabled ones. Within each group the
 * engine's `last_seen_at DESC` order survives, by `Array.prototype.sort`'s
 * stability. A device that is only paired sorts on its pairing alone, having no
 * engine row to be seen on.
 */
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

/**
 * What a row with no engine half says about itself.
 *
 * It states the PRESENT, and that is the whole point. The earlier wording
 * claimed the device had never opened this workspace, which the absence of a
 * row does not prove. Remove deletes the engine row of a device sitting right
 * there. And until a device reloads after the id hand-over, its two halves are
 * still separate. Both put a flat falsehood under the user's main machine.
 * What IS known is that this workspace holds nothing for it, so no push and no
 * per-device preferences, which is what the row says.
 */
const NO_ENGINE_ROW = 'Not set up in this workspace';

/**
 * What to call a device: the most human name available, in that order.
 *
 * The name someone typed wins. Failing that the gateway's pairing label, which
 * a person also chose, on the device itself. Only with neither does this fall
 * back to `device-<first 8>`, and never to the whole id: a 36-character uuid is
 * unreadable, and as a row heading it wraps onto two lines.
 *
 * That last rung is the engine's `resolve_device_name`, so an unnamed, unpaired
 * device is called the same thing here and on an actor chip. The middle rung is
 * deliberately NOT: the engine cannot see the pairing label, so an unnamed but
 * paired device reads as its label here and as `device-<first 8>` on a chip.
 * That is the better name winning where it is available, not a drift to fix by
 * showing the worse one in both places.
 */
export function deviceDisplayName(row: DeviceRowModel): string {
  const stored = row.device?.name;
  if (stored) return stored;
  if (row.paired?.label) return row.paired.label;
  return `device-${row.id.slice(0, 8)}`;
}

/**
 * What a submitted rename field means: the name to store, or `undefined` for
 * "nothing changed, write nothing".
 *
 * The field is prefilled with [`deviceDisplayName`], which is the stored name
 * when there is one and a DERIVED name when there is not. Submitting that
 * prefill untouched is not a rename either way. Check the raw id instead and a
 * click-and-blur stores `device-<8>` as a real name. That name then outlives
 * the id it came from, as soon as a hand-over moves the row.
 *
 * Clearing the field is a real edit, and it drops the stored name so the
 * derived one shows again.
 */
export function submittedDeviceName(
  stored: string | null,
  displayName: string,
  typed: string,
): string | null | undefined {
  const trimmed = typed.trim();
  const next = trimmed === displayName ? stored : (trimmed || null);
  return next === stored ? undefined : next;
}

/**
 * The line under a device's name, saying what this row IS as much as what it
 * did. A reader has to be able to tell "reachable from anywhere" from "seen
 * here once" without opening anything.
 *
 * The pairing clause carries WHEN, not just whether. `paired_at` alone makes
 * every row look alike. A phone in daily use then reads the same as a laptop
 * you sold, and last-seen is the only thing separating them. It is what to
 * read before revoking.
 *
 * A row with no engine half has no other time to show. The engine's own
 * `last_seen_at` is per workspace, and that device never opened this one.
 *
 * `gatewayAnswered` false is a different thing from a device that is not
 * paired. Then the clause is dropped rather than made up, because there is
 * nothing here for it to be true or false about.
 */
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

/** `Paired`, plus when the gateway last saw it, when it can say. */
function pairedClause(paired: PairedDevice, now: number): string {
  const details = deviceDetails(paired, now);
  return details || 'Paired';
}
