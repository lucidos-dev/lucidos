import { REMOTE_GATEWAY, WORKSPACE_ID, withBase } from './basePath';

/** Only A supplies executable worker bytes at A's origin, including for push
 * subscriptions whose notification links target a remote workspace. */
export function workspaceServiceWorkerUrl(): string {
  return REMOTE_GATEWAY === null
    ? withBase('/sw.js')
    : `/~/sw.js?workspace=${encodeURIComponent(WORKSPACE_ID!)}`;
}
