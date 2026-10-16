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

let held = false;

/** Hold this page's lock. Idempotent, so every store's restore may call it. */
export function holdPageOwnerLock(): void {
  if (held || typeof navigator === 'undefined' || !navigator.locks) return;
  held = true;
  navigator.locks.request(PAGE_OWNER_LOCK_PREFIX + pageOwnerId, () => new Promise<never>(() => {})).catch((err) => {
    // Runs without user intent. Without the lock another tab may adopt this
    // page's records too. A pending image then uploads twice, which the blob
    // store absorbs. An unsent message shows in both tabs, and the engine's
    // repeat-event-id guard runs it once.
    console.warn('page owner: could not hold the owner lock', err);
  });
}

/** The owners another open tab still holds, or null when this browser cannot
 *  tell. Null means every record is free to adopt. */
export async function liveOwnerIds(): Promise<ReadonlySet<string> | null> {
  if (typeof navigator === 'undefined' || !navigator.locks) return null;
  const { held: locks = [] } = await navigator.locks.query();
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
