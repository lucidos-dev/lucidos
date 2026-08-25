import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { deviceDetails, timeAgo, revokeConfirmMessage } from './pairedDevices';

const day = 86_400_000;
const now = Date.parse('2026-08-19T12:00:00Z');

describe('timeAgo', () => {
  it('reads a date at a glance, in the coarsest unit that still says something', () => {
    expect(timeAgo('2026-08-19T09:00:00Z', now)).toBe('today');
    expect(timeAgo(new Date(now - day).toISOString(), now)).toBe('yesterday');
    expect(timeAgo(new Date(now - 5 * day).toISOString(), now)).toBe('5 days ago');
    expect(timeAgo(new Date(now - 31 * day).toISOString(), now)).toBe('a month ago');
    expect(timeAgo(new Date(now - 70 * day).toISOString(), now)).toBe('2 months ago');
  });

  it('says nothing rather than NaN for an unreadable timestamp', () => {
    expect(timeAgo('not a date', now)).toBe('');
  });
});

describe('deviceDetails', () => {
  const pairedAt = new Date(now - 40 * day).toISOString();

  it('tells a live device from one you have not opened in a month', () => {
    const live = deviceDetails({ paired_at: pairedAt, last_seen_at: new Date(now).toISOString() }, now);
    expect(live).toBe('Paired a month ago, last seen today');

    const stale = deviceDetails(
      { paired_at: pairedAt, last_seen_at: new Date(now - 35 * day).toISOString() },
      now,
    );
    expect(stale).toBe('Paired a month ago, last seen a month ago');
  });

  it('drops the clause for a device paired before the gateway recorded it', () => {
    // Every device paired before this shipped has no value, and it fills in on
    // that device's next request. Faking one would misreport it as stale.
    expect(deviceDetails({ paired_at: pairedAt }, now)).toBe('Paired a month ago');
  });

  it('drops a clause it cannot read rather than printing NaN', () => {
    expect(deviceDetails({ paired_at: pairedAt, last_seen_at: 'nonsense' }, now))
      .toBe('Paired a month ago');
    expect(deviceDetails({ paired_at: 'nonsense', last_seen_at: 'nonsense' }, now)).toBe('');
  });
});

describe('revokeConfirmMessage', () => {
  it('warns that revoking the device you are using signs you out', () => {
    const message = revokeConfirmMessage({ label: 'My iPhone', isSelf: true, isLast: false });
    expect(message).toMatch(/device you are using/);
    expect(message).toMatch(/signs you out/);
    // Other devices are fine, and saying so is what stops this reading worse
    // than it is.
    expect(message).toMatch(/other paired devices keep working/i);
  });

  it('warns that revoking the last device leaves nothing paired', () => {
    // The one that strands a user: with nothing paired, no browser anywhere can
    // mint a code, so recovery has to start on the machine.
    const message = revokeConfirmMessage({ label: 'My iPhone', isSelf: false, isLast: true });
    expect(message).toMatch(/only paired device/);
    expect(message).toMatch(/desktop app/);
    expect(message).toMatch(/lucidos pair/);
  });

  it('says both when the last device is the one you are on', () => {
    const message = revokeConfirmMessage({ label: 'My iPhone', isSelf: true, isLast: true });
    expect(message).toMatch(/device you are using, and the only one paired/);
    expect(message).toMatch(/desktop app/);
  });

  it('names the device, and promises nothing dramatic, for any other row', () => {
    const message = revokeConfirmMessage({ label: 'My iPhone', isSelf: false, isLast: false });
    expect(message).toMatch(/My iPhone/);
    expect(message).toMatch(/pair it again/);
    expect(message).not.toMatch(/signs you out/);
    expect(message).not.toMatch(/lucidos pair/);
  });

  it('splits its paragraphs on a blank line, which is what the dialog renders', () => {
    // `showConfirm` takes one string, and `DialogMessage` makes a paragraph per
    // blank-line block. A single newline would collapse to a space.
    const message = revokeConfirmMessage({ label: 'My iPhone', isSelf: true, isLast: false });
    expect(message.split('\n\n')).toHaveLength(2);
  });
});

describe('where the list sits', () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const page: string = readFileSync(resolve(here, './MobileAccessPage.tsx'), 'utf8');
  const settings: string = readFileSync(resolve(here, './SettingsView.tsx'), 'utf8');

  it('is one list, on the Devices page', () => {
    // Two lists under one word made the same phone read as two devices. The
    // Devices page joins the halves; Access keeps only Add a device.
    expect(settings).toMatch(/usePairedDevices/);
    expect(settings).toMatch(/buildDeviceRows/);
    expect(page).toMatch(/<AddDeviceSection/);
  });

  it('does not put a second device list back on Access', () => {
    expect(page).not.toMatch(/PairedDevices/);
    expect(page).not.toMatch(/listPairedDevices/);
  });

  it('still points Access at where Revoke went', () => {
    // The list moved, so browsing has to be told. A search hit only helps
    // somebody who already knows the word. The list was pulled up beside Add a
    // device because people looking for Revoke did not find it.
    expect(page).toMatch(/openSettingsSubview\('devices'\)/);
    const add = page.indexOf('<AddDeviceSection');
    const pointer = page.indexOf("openSettingsSubview('devices')");
    expect(add).toBeGreaterThan(-1);
    expect(pointer).toBeGreaterThan(add);
  });

  it('carries the anchor Search Everywhere scrolls to', () => {
    expect(settings).toMatch(/data-search-anchor="devices:list"/);
  });
});


/**
 * The pairing half is async-fetched, so it is a `Loadable`. All four states
 * have to stay distinct: a gateway that BROKE must never render as a
 * deployment that never had one, which is what a shared `null` did.
 */
describe('the pairing half as a Loadable', () => {
  it('is a loaded list when the gateway answers', async () => {
    const { pairedRows, pairingIsKnown } = await import('./pairedDevices');
    const loaded = {
      status: 'loaded' as const,
      data: [{ id: 'a', label: 'My iPhone', paired_at: '2026-08-01T00:00:00Z' }],
    };
    expect(pairedRows(loaded)).toHaveLength(1);
    expect(pairingIsKnown(loaded)).toBe(true);
  });

  it('knows nothing while the fetch is in flight', async () => {
    // Rendering "Not paired" here would be a guess, and it would flip a moment
    // later when the real answer lands.
    const { pairedRows, pairingIsKnown } = await import('./pairedDevices');
    expect(pairedRows({ status: 'loading' })).toBe(null);
    expect(pairingIsKnown({ status: 'loading' })).toBe(false);
    expect(pairedRows({ status: 'not-loaded' })).toBe(null);
    expect(pairingIsKnown({ status: 'not-loaded' })).toBe(false);
  });

  it('knows nothing when the gateway failed, and says nothing about pairing', async () => {
    const { pairedRows, pairingIsKnown } = await import('./pairedDevices');
    const failed = { status: 'failed' as const, error: 'gateway exploded' };
    expect(pairedRows(failed)).toBe(null);
    expect(pairingIsKnown(failed)).toBe(false);
  });

  it('treats "no gateway serves this page" as a loaded fact, not a failure', async () => {
    // A direct engine port answers 404 for `/~/`. There is nothing wrong, and
    // there is also no pairing to report.
    const { pairedRows, pairingIsKnown, NO_GATEWAY } = await import('./pairedDevices');
    const absent = { status: 'loaded' as const, data: NO_GATEWAY };
    expect(pairedRows(absent)).toBe(null);
    expect(pairingIsKnown(absent)).toBe(false);
  });
});
