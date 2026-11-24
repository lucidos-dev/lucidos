import type { App, Loadable } from '../../store/types';

export type LinkedAppResult =
  | { kind: 'linked'; app: App }
  | { kind: 'unknown'; appId: string }
  | { kind: 'pending' }
  | { kind: 'none' };

export function resolveLinkedApp(
  appId: string | null | undefined,
  apps: Loadable<App[]>,
  read: Loadable<App | null>,
): LinkedAppResult {
  if (!appId) return { kind: 'none' };
  if (apps.status !== 'loaded') return { kind: 'pending' };
  const listed = apps.data.find((a) => a.id === appId);
  if (listed) return { kind: 'linked', app: listed };
  if (read.status !== 'loaded') return { kind: 'pending' };
  return read.data ? { kind: 'linked', app: read.data } : { kind: 'unknown', appId };
}
