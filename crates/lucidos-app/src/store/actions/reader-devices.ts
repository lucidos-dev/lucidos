/**
 * The devices whose reads count as this page's (ADR 0623): on a team gateway,
 * its person's. `readRequestedForMe` reads them to keep a read request one
 * person saw on everyone else's Review group.
 */
import { readerDevices } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import { readerDevices as fetchReaderDevices } from '../../api/client';

export async function loadReaderDevices(): Promise<void> {
  setLoadingIfFresh(readerDevices);
  try {
    readerDevices.value = { status: 'loaded', data: await fetchReaderDevices() };
  } catch (error) {
    readerDevices.value = toFailed(error);
  }
}

/** Whether a read `actor` made counts as this page's person's. A page that
 *  reads as the team, or does not know its person yet, takes every read as
 *  its own, as before people existed. */
export function readIsMine(actor: unknown): boolean {
  const reader = readerDevices.value;
  const mine = reader.status === 'loaded' ? reader.data.devices : null;
  if (mine === null) return true;
  const origin = actor as { kind?: unknown; device_id?: unknown } | null | undefined;
  return origin?.kind === 'device' && typeof origin.device_id === 'string' && mine.includes(origin.device_id);
}
