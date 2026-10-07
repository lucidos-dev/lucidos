/**
 * Is a newer Lucidos published for this install, as a READ over the signals?
 *
 * A leaf beside them, for the reason `store/releaseNotices.ts` gives: the
 * action module next door (`actions/app-update.ts`) owns the toast, the Tauri
 * IPC and the install, and reaches the whole menu-action graph through them. A
 * surface that only wants to know whether an update exists must not drag that
 * in. The *System attention badge* asks exactly that, from the menu-drawer button.
 */
import { clientOfferedRelease, latestTauriAppVersion, releaseCheck } from './store';
import { isNewerVersion } from '../utils/version';

/**
 * The newer version available to install, or `null`.
 *
 * One derivation of "is there an update?", shared by the notice, the button
 * label, the button's action and the badge, so they cannot drift apart. It
 * reads the signals at call time, so calling it during render subscribes the
 * caller.
 *
 * The gateway's answer wins where there is one, because it covers every install
 * shape. Two fallbacks follow, each compared against a version of its own kind:
 * a client on an older gateway reads its own updater's release, and a dev client
 * reads its checkout's build id from the engine's `/health`.
 */
export function packagedUpdateVersion(): string | null {
  const announced = releaseCheck.value?.latest;
  if (announced) return announced.version;
  return newerThan(clientOfferedRelease.value, window.__LUCIDOS_APP_RELEASE__)
    ?? newerThan(latestTauriAppVersion.value, window.__LUCIDOS_APP_VERSION__);
}

function newerThan(latest: string | null, current: string | undefined): string | null {
  return latest && current && isNewerVersion(latest, current) ? latest : null;
}
