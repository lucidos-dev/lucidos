/** Background reads own their initial snapshot, so pre-load refresh requests
 * need no queue: installation reads the latest state after paint. */
import { refreshReleaseCheck, resumeUpdateRelay } from './actions/app-update';
import { startEngineUpdateChecks, stopEngineUpdateChecks, checkEngineVersion } from './actions/engine-update';
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
      stopEngineUpdateChecks();
      stopSlownessChecks();
    },
  };
}
