/** Which page load owns a record kept on this device. Each page load holds a
 *  Web Lock named after itself for its whole life. Another tab reads the held
 *  locks to learn which records still have a live owner, and adopts only the
 *  rest. A leaf: no store imports. */

import { WORKSPACE_ID } from '../utils/basePath';
import { generateUuid } from '../utils/uuid';
import { deviceDatabaseName } from './deviceDatabase';

/** This page load. Each record names the page that owns it. */
export const pageOwnerId = generateUuid();

/** Named after the pending-upload database, which introduced the lock. A tab
 *  still running that build then counts as live here too. */
export const PAGE_OWNER_LOCK_PREFIX = `${deviceDatabaseName('lucidos-pending-uploads', WORKSPACE_ID)}:owner:`;

let lockState: 'unrequested' | 'requested' | 'held' | 'unavailable' = 'unrequested';

/** Hold this page's lock. Idempotent, so every store's restore may call it. */
export function holdPageOwnerLock(): void {
  if (lockState !== 'unrequested' || typeof navigator === 'undefined' || !navigator.locks) return;
  lockState = 'requested';
  const unavailable = (err: unknown) => {
    lockState = 'unavailable';
    // Startup capability probe, not user intent. Restore recovery treats owners
    // as unknown and retains originals for explicit review instead of adopting.
    console.warn('page owner: could not hold the owner lock', err);
  };
  try {
    void navigator.locks.request(PAGE_OWNER_LOCK_PREFIX + pageOwnerId, () => {
      lockState = 'held';
      return new Promise<never>(() => {});
    }).then(() => unavailable(new Error('owner lock released')), unavailable);
  } catch (err) { unavailable(err); }
}

/** The owners another open tab still holds, or null when this browser cannot
 *  tell. Ordinary replay-safe adoption may proceed on null; restore composer
 *  reconstruction must retain unknown owners until explicit review. */
export async function liveOwnerIds(): Promise<ReadonlySet<string> | null> {
  if (typeof navigator === 'undefined' || !navigator.locks) return null;
  holdPageOwnerLock();
  let snapshot: LockManagerSnapshot;
  try { snapshot = await navigator.locks.query(); }
  catch { return null; }
  // Inspection can succeed where request is refused. Absence from that
  // snapshot proves no orphanhood until this page can participate itself.
  if (lockState !== 'held') return null;
  // A just-started page may be queued to acquire its lifetime lock. It is
  // already alive; treating that window as an orphan can freeze stale content.
  const locks = [...(snapshot.held ?? []), ...(snapshot.pending ?? [])];
  const owners = new Set<string>();
  for (const lock of locks) {
    if (lock.name?.startsWith(PAGE_OWNER_LOCK_PREFIX)) owners.add(lock.name.slice(PAGE_OWNER_LOCK_PREFIX.length));
  }
  return owners;
}

/** Whether a record belongs to another tab that is still open. */
export function ownedByAnotherLiveTab(ownerId: string, liveOwners: ReadonlySet<string> | null): boolean {
  return ownerId !== pageOwnerId && liveOwners?.has(ownerId) === true;
}
