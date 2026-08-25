import { useMemo, useRef } from 'preact/hooks';
import { touchActivated } from '../utils/tapGesture';

/** Bind `touchActivated` to a component.
 *
 *  The handler pair must be STABLE, because the helper's twin window is
 *  per-instance state. Rebuilding it each render would forget the touch it just
 *  served, and a browser that dispatched the suppressed click anyway would run
 *  the action twice. The action and the enable flag must NOT be stable, since
 *  both are read off the current render. So the pair is memoized once and reads
 *  the latest of each through a ref.
 *
 *  `enabled` gates the touch path only. See `touchActivated`. */
export function useTouchActivated(action: () => void, enabled = true) {
  const latest = useRef({ action, enabled });
  latest.current = { action, enabled };
  return useMemo(
    () => touchActivated(
      () => latest.current.action(),
      { enabled: () => latest.current.enabled },
    ),
    [],
  );
}
