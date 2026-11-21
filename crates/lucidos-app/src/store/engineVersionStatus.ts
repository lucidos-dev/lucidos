import { gapRefreshedVersion } from './eventGap';
import { effect, untracked } from '@preact/signals';

/** An event or stream gap invalidates the authoritative version snapshot. */
export const engineVersionStatusVersion = gapRefreshedVersion();

/** Startup owns this subscription. Read on invalidation, when preferences
 *  first settle, or when the switch dismissal changes. A preferences reload
 *  delivers a dismissal, after an event or a stream gap. A read that landed
 *  before that reload drew the toast against the old value. */
export function subscribeEngineVersionStatus(
  refresh: () => void,
  ready: () => boolean = () => true,
  dismissal: () => unknown = () => undefined,
): () => void {
  let observedVersion = engineVersionStatusVersion.peek();
  let observedReady = untracked(ready);
  let observedDismissal = untracked(dismissal);
  return effect(() => {
    const version = engineVersionStatusVersion.value;
    const dismissed = dismissal();
    if (!ready()) return;
    if (observedReady && version === observedVersion && dismissed === observedDismissal) return;
    observedReady = true;
    observedVersion = version;
    observedDismissal = dismissed;
    untracked(refresh);
  });
}
