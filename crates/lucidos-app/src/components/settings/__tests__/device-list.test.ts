import { describe, expect, it } from 'vitest';
import {
  buildDeviceRows,
  deviceDisplayName,
  deviceRowSummary,
  submittedDeviceName,
} from '../deviceList';
import type { DeviceInfo } from '../../../api/types';
import type { PairedDevice } from '../../../api/client/control';

function device(id: string, over: Partial<DeviceInfo> = {}): DeviceInfo {
  return {
    id,
    name: null,
    user_agent: 'UA',
    push_enabled: false,
    last_seen_at: '2026-08-01T00:00:00Z',
    created_at: '2026-08-01T00:00:00Z',
    ...over,
  } as DeviceInfo;
}

function paired(id: string, over: Partial<PairedDevice> = {}): PairedDevice {
  return { id, label: `label-${id}`, paired_at: '2026-08-01T00:00:00Z', ...over };
}

const describeUa = (ua?: string) => (ua ? `parsed:${ua}` : '');
// Fixed, so the relative times the summary prints are deterministic.
const NOW = Date.parse('2026-08-21T00:00:00Z');

describe('buildDeviceRows', () => {
  it('joins the two halves on the shared id, so one phone is one row', () => {
    // The whole point of the change: before the ids unified, this same phone
    // was a row in Access AND a different row in Devices.
    const rows = buildDeviceRows([device('phone')], [paired('phone')], 'laptop');
    expect(rows).toHaveLength(1);
    expect(rows[0].device?.id).toBe('phone');
    expect(rows[0].paired?.id).toBe('phone');
  });

  it('keeps a paired device that has never opened this workspace', () => {
    // The gateway list is machine-global and the engine list is per workspace,
    // so a phone paired from another workspace belongs here with no engine row.
    const rows = buildDeviceRows([], [paired('phone')], 'laptop');
    expect(rows).toHaveLength(1);
    expect(rows[0].device).toBeUndefined();
    expect(rows[0].paired?.id).toBe('phone');
  });

  it('keeps a device that never paired', () => {
    // A client on a direct engine port never went through the gateway.
    const rows = buildDeviceRows([device('local')], [], 'laptop');
    expect(rows).toHaveLength(1);
    expect(rows[0].paired).toBeUndefined();
  });

  it('puts the current device first, whichever half it came from', () => {
    const rows = buildDeviceRows(
      [device('other', { push_enabled: true }), device('me')],
      [paired('other')],
      'me',
    );
    expect(rows.map((r) => r.id)).toEqual(['me', 'other']);
  });

  it('ranks paired above push-enabled, and both above the rest', () => {
    const rows = buildDeviceRows(
      [device('plain'), device('pushy', { push_enabled: true }), device('bonded')],
      [paired('bonded')],
      'nobody',
    );
    expect(rows.map((r) => r.id)).toEqual(['bonded', 'pushy', 'plain']);
  });

  it('treats a missing gateway as no pairing information at all', () => {
    // `null` is "nothing answered", which must not read as "nothing is paired"
    // in a way that reorders or drops rows.
    const rows = buildDeviceRows([device('a'), device('b')], null, 'a');
    expect(rows.map((r) => r.id)).toEqual(['a', 'b']);
    expect(rows.every((r) => r.paired === undefined)).toBe(true);
  });
});

describe('deviceRowSummary', () => {
  it('says a device is paired once a gateway has answered', () => {
    const [row] = buildDeviceRows([device('a')], [paired('a')], 'a');
    // Not just "Paired": WHEN is what tells a phone in daily use from a laptop
    // you sold, and it is what to read before revoking.
    expect(deviceRowSummary(row, describeUa, true, NOW)).toEqual([
      'parsed:UA',
      'Paired 20 days ago',
    ]);
  });

  it('says a device is not paired, which is a real answer', () => {
    const [row] = buildDeviceRows([device('a')], [], 'a');
    expect(deviceRowSummary(row, describeUa, true, NOW)).toEqual(['parsed:UA', 'Not paired']);
  });

  it('drops the pairing clause entirely when no gateway answered', () => {
    // With no gateway there is nothing for the clause to be true or false
    // about, so inventing one would be a lie either way.
    const [row] = buildDeviceRows([device('a')], null, 'a');
    expect(deviceRowSummary(row, describeUa, false, NOW)).toEqual(['parsed:UA']);
  });

  it('says a paired device is not set up here, without claiming it never was', () => {
    // The absence of an engine row does NOT prove the device never opened this
    // workspace: Remove deletes that row, and the id hand-over leaves the two
    // halves apart until the device reloads. Both would put a flat falsehood
    // under the user's main machine.
    const [row] = buildDeviceRows([], [paired('a')], 'b');
    expect(deviceRowSummary(row, describeUa, true, NOW)).toEqual([
      'Not set up in this workspace',
      'Paired 20 days ago',
    ]);
  });
});

describe('deviceDisplayName', () => {
  it('uses the name the user gave it', () => {
    const [row] = buildDeviceRows([device('a', { name: 'My MacBook' })], [], 'a');
    expect(deviceDisplayName(row)).toBe('My MacBook');
  });

  it('shortens an unnamed device rather than titling it with a uuid', () => {
    // A 36-character uuid wraps onto two lines as a row heading. This is the
    // engine's own `resolve_device_name` shape, so the list and an actor chip
    // call the same device the same thing.
    const [row] = buildDeviceRows([device('0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9')], [], 'x');
    expect(deviceDisplayName(row)).toBe('device-0a1b2c3d');
  });

  it('falls back to the gateway label when there is no engine row to name', () => {
    const [row] = buildDeviceRows([], [paired('a', { label: 'Chrome localhost' })], 'x');
    expect(deviceDisplayName(row)).toBe('Chrome localhost');
  });

  it('prefers the name over the gateway label, so a rename sticks', () => {
    const [row] = buildDeviceRows([device('a', { name: 'Work laptop' })], [paired('a')], 'x');
    expect(deviceDisplayName(row)).toBe('Work laptop');
  });
});

describe('submittedDeviceName', () => {
  it('writes nothing when an unnamed device is opened and blurred', () => {
    // The regression this exists for. The field prefills with the DERIVED
    // name. So a check against the raw id reads that prefill as a rename and
    // stores `device-<8>` for real, which a later hand-over makes stale.
    expect(submittedDeviceName(null, 'device-0a1b2c3d', 'device-0a1b2c3d')).toBeUndefined();
  });

  it('writes nothing when a named device is opened and blurred', () => {
    expect(submittedDeviceName('My MacBook', 'My MacBook', 'My MacBook')).toBeUndefined();
  });

  it('stores what the user typed', () => {
    expect(submittedDeviceName(null, 'device-0a1b2c3d', 'Kitchen iPad')).toBe('Kitchen iPad');
  });

  it('trims what the user typed', () => {
    expect(submittedDeviceName(null, 'device-0a1b2c3d', '  Kitchen iPad  ')).toBe('Kitchen iPad');
  });

  it('clears the stored name when the field is emptied', () => {
    // A real edit: the row falls back to the derived name from then on.
    expect(submittedDeviceName('My MacBook', 'My MacBook', '')).toBeNull();
    expect(submittedDeviceName('My MacBook', 'My MacBook', '   ')).toBeNull();
  });

  it('writes nothing when an unnamed device is emptied, having nothing to clear', () => {
    expect(submittedDeviceName(null, 'device-0a1b2c3d', '')).toBeUndefined();
  });
});
