/** The send implementation is needed by an action, not by draft synchronization. */
import { lazyModule } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';
import { showToast } from '../store';
const sender = lazyModule(() => import('./chat'));
prefetchWhenIdle(sender);
type Send = typeof import('./chat')['sendMessage'];

export const sendMessage: Send = (...args) => {
  if (sender.isLoaded()) return sender.get().sendMessage(...args);
  return sender.preload().then((loaded) => {
    if (!loaded) {
      showToast('Message sending code could not load. Your text is back in the draft. Try Send again.', 'error');
      return 'dropped' as const;
    }
    return sender.get().sendMessage(...args);
  });
};
