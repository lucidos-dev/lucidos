import { REMOTE_GATEWAY, WORKSPACE_ID, withBase } from './basePath';

export function workspaceServiceWorkerUrl(): string {
  return REMOTE_GATEWAY === null
    ? withBase('/sw.js')
    : `/~/sw.js?workspace=${encodeURIComponent(WORKSPACE_ID!)}`;
}
