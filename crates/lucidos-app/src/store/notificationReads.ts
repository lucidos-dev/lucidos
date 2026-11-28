import type { Loadable, Notification } from './types';

export interface PendingRead {
  owner: number;
  ackedAt: number | null;
}

export type PendingReads = ReadonlyMap<string, PendingRead>;

export function readIsPending(ack: number | null, servedEpoch: number): boolean {
  return ack === null || servedEpoch <= ack;
}

export function withoutPendingReads(
  served: Loadable<Notification[]>,
  reads: PendingReads,
  servedEpoch: number,
): Loadable<Notification[]> {
  if (served.status !== 'loaded') return served;
  const data = served.data.filter((n) => !isMasked(n, reads, servedEpoch));
  return data.length === served.data.length ? served : { status: 'loaded', data };
}

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
