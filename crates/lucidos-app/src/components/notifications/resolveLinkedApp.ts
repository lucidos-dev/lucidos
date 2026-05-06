import type { App, Loadable } from '../../store/types';

export type LinkedAppResult =
  | { kind: 'linked'; app: App }
  | { kind: 'unknown'; appId: string }
  | { kind: 'pending' }
  | { kind: 'none' };

export function resolveLinkedApp(
  appId: string | null | undefined,
  apps: Loadable<App[]>,
): LinkedAppResult {
  if (!appId) return { kind: 'none' };
  // Without a loaded apps list we can't tell "stale id" from "haven't fetched yet" —
  // surfacing 'unknown' here would falsely flag every linked notification on cold-start
  // deep-link / push paths that race the initial loadApps() call.
  if (apps.status !== 'loaded') return { kind: 'pending' };
  const app = apps.data.find((a) => a.id === appId);
  return app ? { kind: 'linked', app } : { kind: 'unknown', appId };
}
