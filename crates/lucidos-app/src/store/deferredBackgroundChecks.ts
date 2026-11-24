import { lazyModule } from '../utils/lazyComponent';
import { prefetchWhenIdle } from '../utils/idlePrefetch';
import { showToast, removeToast } from './store';
import { errorDetail } from '../utils/errorDetail';

const background = lazyModule(() => import('./backgroundChecks'));
type Checks = ReturnType<typeof import('./backgroundChecks')['startBackgroundChecks']>;

export function startDeferredBackgroundChecks() {
  let checks: Checks | undefined;
  let stopped = false;
  let loading: Promise<void> | null = null;
  const preload = () => {
    if (stopped || checks) return Promise.resolve();
    return loading ??= initialize().catch((error) => {
      if (stopped) return;
      showToast(`Background checks could not start: ${errorDetail(error)}`, 'error', {
        key: 'background-checks-startup', action: { label: 'Retry', onClick: () => { void preload(); } },
      });
    }).finally(() => { loading = null; });
  };
  async function initialize() {
    if (!await background.preload()) throw new Error('Code could not load. Retry or refresh Lucidos.');
    if (stopped) return;
    checks = background.get().startBackgroundChecks();
    removeToast('background-checks-startup');
  }
  prefetchWhenIdle({ preload });
  return {
    refreshEngine: () => checks?.refreshEngine(),
    refreshRelease: () => checks?.refreshRelease(),
    refreshSlowness: () => checks?.refreshSlowness(),
    stop: () => { stopped = true; checks?.stop(); },
  };
}
