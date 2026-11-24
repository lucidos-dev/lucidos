
import { useEffect, useState } from 'preact/hooks';
import { revokePairedDevice, type PairedDevice } from '../../api/client/control';
import {
  loadPairedDevices,
  pairedDevices,
  type PairedDevicesLoadable,
} from '../../store/actions/pairedDevices';
import { pairingSession } from '../../api/client/pairing';
import { showConfirm, showToast } from '../../store/store';

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
  selfId: string | null;
  reload: () => void;
}

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
