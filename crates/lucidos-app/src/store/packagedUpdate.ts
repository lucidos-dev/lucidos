import { clientOfferedRelease, latestTauriAppVersion, releaseCheck } from './store';
import { isNewerBuildId, isNewerVersion } from '../utils/version';

export function packagedUpdateVersion(): string | null {
  const announced = releaseCheck.value?.latest;
  if (announced) return announced.version;
  return newerThan(clientOfferedRelease.value, window.__LUCIDOS_APP_RELEASE__)
    ?? (latestTauriAppVersion.value && window.__LUCIDOS_APP_VERSION__
      && isNewerBuildId(latestTauriAppVersion.value, window.__LUCIDOS_APP_VERSION__)
      ? latestTauriAppVersion.value : null);
}

function newerThan(latest: string | null, current: string | undefined): string | null {
  return latest && current && isNewerVersion(latest, current) ? latest : null;
}
