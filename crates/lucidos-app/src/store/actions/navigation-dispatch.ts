/** Direct browser interactions stay synchronous; other destinations load on demand. */
import { navigateToNewChat, navigateToUrl } from './navigationDirect';
import { lazyModule } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';
import { showToast } from '../store';

type Navigate = typeof import('./navigation-request')['handleNavigationRequest'];
const router = lazyModule(() => import('./navigation-request'));
prefetchWhenIdle(router);

export const handleNavigationRequest: Navigate = (nav, options) => {
  if (nav.target === 'url') return navigateToUrl(nav, options?.source);
  if (nav.target === 'new-chat') return navigateToNewChat(nav, options?.source);
  if (router.isLoaded()) return router.get().handleNavigationRequest(nav, options);
  return router.preload().then((loaded) => {
    if (loaded) return router.get().handleNavigationRequest(nav, options);
    showToast('Navigation code could not load. Your destination is still waiting.', 'error', {
      key: 'navigation-code-load', action: { label: 'Retry', onClick: () => { void handleNavigationRequest(nav, options); } },
    });
  });
};
