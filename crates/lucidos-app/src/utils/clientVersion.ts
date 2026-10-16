import { CLIENT_BUILD_ID } from 'virtual:build-id';
import { formatBuildId } from './buildId';

/** The version of LUCIDOS the user is running, as this client can honestly say
 *  it.
 *
 *  What identifies the client differs by platform. The Tauri shell updates as a
 *  versioned unit, so its app version is a real version with a real updater
 *  behind it. The web client has no such version: it is identified by the build
 *  that produced the code executing right now (`CLIENT_BUILD_ID`), which is
 *  exactly what the refresh badge compares against the served build. The live
 *  dev server leaves that placeholder un-stamped, and showing the placeholder
 *  verbatim is noise, so it reads "dev".
 *
 *  Deliberately NOT the engine's CalVer, which is the other number in Settings >
 *  System and belongs to the workspace's binary rather than to this client.
 *  Baking that in froze it at bundle-build time and drifted from the running
 *  engine on every engine-only Apply, showing two disagreeing numbers no user
 *  action could reconcile (see `settings/clientVersionSource.test.ts`, which
 *  fails if it ever comes back).
 *
 *  One reader: the System page's Client row. The Lucidos menu's identity row
 *  used to be the second, and deliberately is not any more: it names the PRODUCT
 *  rather than this device's copy of it, so it shows the umbrella release
 *  (`utils/lucidosVersion.ts`), which reads the same on every platform. */
export function clientVersionLabel(): string {
  const packaged = typeof window !== 'undefined' ? window.__LUCIDOS_APP_VERSION__ : undefined;
  if (packaged) return packaged;
  return formatBuildId(CLIENT_BUILD_ID);
}
