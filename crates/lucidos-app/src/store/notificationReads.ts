import type { Loadable, Notification } from './types';

/** A read the server has not yet confirmed, a *pending intent* (ADR 0437).
 *  `owner` names the request that holds it, so a later request on the same row
 *  takes it over. `ackedAt` is the load epoch current when that request was
 *  acknowledged, or null while it is still in flight. */
export interface PendingRead {
  owner: number;
  ackedAt: number | null;
}

/** Every pending read, by notification id. */
export type PendingReads = ReadonlyMap<string, PendingRead>;

/** Whether a list served by the load at `servedEpoch` still owes `ack`'s read
 *  a mask. Any load started after the acknowledgement has a higher epoch and
 *  already reflects the read. */
export function readIsPending(ack: number | null, servedEpoch: number): boolean {
  return ack === null || servedEpoch <= ack;
}

/** The served unread set without the reads it does not yet reflect. Returns
 *  the served value itself when nothing changes, so its identity holds. */
export function withoutPendingReads(
  served: Loadable<Notification[]>,
  reads: PendingReads,
  servedEpoch: number,
): Loadable<Notification[]> {
  if (served.status !== 'loaded') return served;
  const data = served.data.filter((n) => !isMasked(n, reads, servedEpoch));
  return data.length === served.data.length ? served : { status: 'loaded', data };
}

/** The served browse list with every pending read shown as read. Returns the
 *  served value itself when nothing changes. */
export function withPendingReadsShown(
  served: Loadable<Notification[]>,
  reads: PendingReads,
  servedEpoch: number,
): Loadable<Notification[]> {
  if (served.status !== 'loaded' || !served.data.some((n) => isMasked(n, reads, servedEpoch))) return served;
  return {
    status: 'loaded',
    data: served.data.map((n) => (isMasked(n, reads, servedEpoch) ? { ...n, read: true } : n)),
  };
}

function isMasked(n: Notification, reads: PendingReads, servedEpoch: number): boolean {
  const read = reads.get(n.id);
  return !n.read && read !== undefined && readIsPending(read.ackedAt, servedEpoch);
}
