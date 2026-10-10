import type { App, Loadable } from '../../store/types';

export type LinkedAppResult =
  | { kind: 'linked'; app: App }
  | { kind: 'unknown'; appId: string }
  | { kind: 'pending' }
  | { kind: 'none' };

/** `read` is the `appsById` read of `appId`. An id the apps list lacks may be
 *  a widget (ADR 0402), so only that read's 404 makes it unknown. */
export function resolveLinkedApp(
  appId: string | null | undefined,
  apps: Loadable<App[]>,
  read: Loadable<App | null>,
): LinkedAppResult {
  if (!appId) return { kind: 'none' };
  // Without a loaded apps list we can't tell "stale id" from "haven't fetched yet" —
  // surfacing 'unknown' here would falsely flag every linked notification on cold-start
  // deep-link / push paths that race the initial loadApps() call.
  if (apps.status !== 'loaded') return { kind: 'pending' };
  const listed = apps.data.find((a) => a.id === appId);
  if (listed) return { kind: 'linked', app: listed };
  if (read.status !== 'loaded') return { kind: 'pending' };
  return read.data ? { kind: 'linked', app: read.data } : { kind: 'unknown', appId };
}
