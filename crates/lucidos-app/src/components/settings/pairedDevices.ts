/**
 * The pairing half of a device row: who may reach this machine at all.
 *
 * The list itself is shared state (`store/actions/pairedDevices.ts`). This
 * module is the Devices page's side of it: the revoke flow and how a row
 * describes its pairing. `deviceList.ts` joins it onto the engine's rows.
 *
 * This was a whole section under Settings -> Access. That is how Settings ended
 * up with a *device* in two places that were not the same device. It is now the
 * data behind one column of one list.
 */

import { useEffect, useState } from 'preact/hooks';
import { revokePairedDevice, type PairedDevice } from '../../api/client/control';
import {
  loadPairedDevices,
  pairedDevices,
  type PairedDevicesLoadable,
} from '../../store/actions/pairedDevices';
import { pairingSession } from '../../api/client/pairing';
import { showConfirm, showToast } from '../../store/store';

/**
 * What the confirm says, which turns entirely on who is being revoked.
 *
 * Revoke is one click, and the device it cuts off cannot undo it. So the two
 * cases that lock somebody out have to say so first. Revoking the device you
 * are reading this on signs you out here. Revoking the last one leaves nothing
 * paired, and only the machine itself can let the next device in.
 */
export function revokeConfirmMessage(device: {
  label: string;
  isSelf: boolean;
  isLast: boolean;
}): string {
  const fromTheMachine =
    'Getting back in starts on the machine itself: open the Lucidos desktop app, '
    + 'which pairs itself, or run lucidos pair there.';
  if (device.isSelf && device.isLast) {
    return (
      'This is the device you are using, and the only one paired. Revoking it '
      + 'signs you out with nothing left that can reach this machine.\n\n'
      + fromTheMachine
    );
  }
  if (device.isSelf) {
    return (
      'This is the device you are using. Revoking it signs you out here, and '
      + 'getting back in takes a new pairing code.\n\n'
      + 'Your other paired devices keep working.'
    );
  }
  if (device.isLast) {
    return (
      `${device.label} is the only paired device. Revoking it leaves this `
      + 'machine with nothing paired.\n\n'
      + fromTheMachine
    );
  }
  return `Lucidos will stop answering ${device.label}. You can pair it again whenever you like.`;
}

/** How long ago a timestamp was, in the coarsest unit that still says
 *  something. An exact timestamp is noise on a list read at a glance.
 *
 *  `''` for anything unreadable, so a caller can drop the clause rather than
 *  print a `NaN`. */
export function timeAgo(iso: string, now: number): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return '';
  const days = Math.floor((now - then) / 86_400_000);
  if (days <= 0) return 'today';
  if (days === 1) return 'yesterday';
  if (days < 30) return `${days} days ago`;
  const months = Math.floor(days / 30);
  return months === 1 ? 'a month ago' : `${months} months ago`;
}

/** The pairing clause under a device's name.
 *
 *  Two facts, and the second is the one worth having: `paired_at` alone makes
 *  every row look alike, so a phone in daily use reads the same as a laptop you
 *  sold. Last-seen is dropped, not faked, when the gateway has never recorded
 *  one or the value will not parse. */
export function deviceDetails(
  device: { paired_at: string; last_seen_at?: string },
  now: number,
): string {
  const paired = timeAgo(device.paired_at, now);
  const seen = device.last_seen_at ? timeAgo(device.last_seen_at, now) : '';
  const clauses = [paired && `Paired ${paired}`, seen && `last seen ${seen}`].filter(Boolean);
  return clauses.join(', ');
}

export interface PairedDevicesState {
  paired: PairedDevicesLoadable;
  /** Which row is this browser. `null` while unknown, and for a local process,
   *  which is nobody's row. An unknown answer only costs the sharper wording. */
  selfId: string | null;
  reload: () => void;
}

/** The Devices page's view of the shared list. Opening the page refetches, so
 *  it shows what the gateway holds now. */
export function usePairedDevices(): PairedDevicesState {
  const [selfId, setSelfId] = useState<string | null>(null);

  useEffect(() => {
    void loadPairedDevices();
    pairingSession()
      .then((s) => setSelfId(s.device_id ?? null))
      .catch(() => setSelfId(null));
  }, []);

  return { paired: pairedDevices.value, selfId, reload: () => void loadPairedDevices() };
}

/**
 * Cut a device off the machine, after saying what that costs.
 *
 * Machine-global, unlike Remove beside it: this reaches every workspace at
 * once. Revoking the device you are on clears its cookie in the same response.
 * The page reloads into the pairing screen, rather than quietly 401ing at
 * everything.
 */
export async function revokePaired(
  device: PairedDevice,
  state: { name: string; selfId: string | null; count: number; reload: () => void },
): Promise<void> {
  const isSelf = state.selfId !== null && device.id === state.selfId;
  const confirmed = await showConfirm(
    revokeConfirmMessage({ label: state.name, isSelf, isLast: state.count === 1 }),
    'Revoke',
    { title: 'Revoke this device', variant: 'danger' },
  );
  if (!confirmed) return;

  try {
    await revokePairedDevice(device.id);
    if (isSelf) {
      window.location.reload();
      return;
    }
    showToast(`${state.name} can no longer reach this machine`, 'success');
    state.reload();
  } catch (e) {
    showToast(e instanceof Error ? e.message : 'Could not revoke that device', 'error');
  }
}
