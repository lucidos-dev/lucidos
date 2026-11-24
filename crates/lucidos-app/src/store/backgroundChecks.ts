import { refreshReleaseCheck, resumeUpdateRelay } from './actions/app-update';
import { startEngineUpdateChecks, checkEngineVersion, cancelEngineVersionReads } from './actions/engine-update';
import { refreshSlowness, startSlownessChecks, stopSlownessChecks } from './actions/slowness';

export function startBackgroundChecks() {
  void refreshReleaseCheck();
  resumeUpdateRelay();
  startEngineUpdateChecks();
  startSlownessChecks();
  return {
    refreshEngine: () => { void checkEngineVersion(); },
    refreshRelease: () => { void refreshReleaseCheck(); },
    refreshSlowness: () => { void refreshSlowness(); },
    stop: () => {
      cancelEngineVersionReads();
      stopSlownessChecks();
    },
  };
}
