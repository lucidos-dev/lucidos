import { ApiError } from '../../api/client/_core';
import { getAppApi } from '../../api/client/widgets';
import { appById, appsById } from '../appsById';
import { toFailed, type App, type Loadable } from '../types';

function setRead(id: string, read: Loadable<App | null>): void {
  const reads = new Map(appsById.value);
  reads.set(id, read);
  appsById.value = reads;
}

/** Reads each id not yet read. Only a 404 records the app as gone: a transient
 *  failure is no verdict. */
export async function readAppsById(ids: readonly string[]): Promise<void> {
  await Promise.all(ids.filter((id) => appById(id).status === 'not-loaded').map(async (id) => {
    const loading: Loadable<App | null> = { status: 'loading' };
    setRead(id, loading);
    let read: Loadable<App | null>;
    try {
      read = { status: 'loaded', data: await getAppApi(id) };
    } catch (error) {
      read = error instanceof ApiError && error.httpCode === 404
        ? { status: 'loaded', data: null }
        : toFailed(error);
    }
    // Forgotten meanwhile: this answer may predate the change that forgot it.
    if (appsById.value.get(id) === loading) setRead(id, read);
  }));
}

/** The app was created, changed or deleted: the next reader reads it afresh. */
export function forgetAppById(id: string): void {
  if (!appsById.value.has(id)) return;
  const reads = new Map(appsById.value);
  reads.delete(id);
  appsById.value = reads;
}
