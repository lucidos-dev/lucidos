
import { WORKSPACE_ID } from '../utils/basePath';
import { generateUuid } from '../utils/uuid';
import { PENDING_UPLOADS_DATABASE, deviceDatabaseName } from './deviceDatabase';

export const pageOwnerId = generateUuid();

export const PAGE_OWNER_LOCK_PREFIX = `${deviceDatabaseName(PENDING_UPLOADS_DATABASE, WORKSPACE_ID)}:owner:`;

let lockState: 'unrequested' | 'requested' | 'held' | 'unavailable' = 'unrequested';

export function holdPageOwnerLock(): void {
  if (lockState !== 'unrequested' || typeof navigator === 'undefined' || !navigator.locks) return;
  lockState = 'requested';
  const unavailable = (err: unknown) => {
    lockState = 'unavailable';
    console.warn('page owner: could not hold the owner lock', err);
  };
  try {
    void navigator.locks.request(PAGE_OWNER_LOCK_PREFIX + pageOwnerId, () => {
      lockState = 'held';
      return new Promise<never>(() => {});
    }).then(() => unavailable(new Error('owner lock released')), unavailable);
  } catch (err) { unavailable(err); }
}

export async function liveOwnerIds(): Promise<ReadonlySet<string> | null> {
  if (typeof navigator === 'undefined' || !navigator.locks) return null;
  holdPageOwnerLock();
  let snapshot: LockManagerSnapshot;
  try { snapshot = await navigator.locks.query(); }
  catch { return null; }
  if (lockState !== 'held') return null;
  const locks = [...(snapshot.held ?? []), ...(snapshot.pending ?? [])];
  const owners = new Set<string>();
  for (const lock of locks) {
    if (lock.name?.startsWith(PAGE_OWNER_LOCK_PREFIX)) owners.add(lock.name.slice(PAGE_OWNER_LOCK_PREFIX.length));
  }
  return owners;
}

export function ownedByAnotherLiveTab(ownerId: string, liveOwners: ReadonlySet<string> | null): boolean {
  return ownerId !== pageOwnerId && liveOwners?.has(ownerId) === true;
}
