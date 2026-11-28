import { gapRefreshedVersion } from './eventGap';
import { effect, untracked } from '@preact/signals';

export const engineVersionStatusVersion = gapRefreshedVersion();

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
