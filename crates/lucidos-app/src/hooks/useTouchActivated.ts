import { useMemo, useRef } from 'preact/hooks';
import { touchActivated, type ActivationGate } from '../utils/tapGesture';

/** Bind `touchActivated` to a component.
 *
 *  The handlers must be STABLE, because the helper carries per-instance press
 *  state: the twin window. Rebuilding them each render would forget the touch it
 *  just served, and a browser that dispatched the suppressed click anyway would
 *  run the action twice. The action, the enable flag and the gate must NOT be
 *  stable, since all three are read off the current render. So the set is
 *  memoized once and reads the latest of each through a ref.
 *
 *  `enabled` stands the touch path down; `gate` rules on the click path and is
 *  spent by the touch path. See `touchActivated`. */
export function useTouchActivated(action: () => void, enabled = true, gate?: ActivationGate) {
  const latest = useRef({ action, enabled, gate });
  latest.current = { action, enabled, gate };
  return useMemo(
    () => touchActivated(
      () => latest.current.action(),
      {
        enabled: () => latest.current.enabled,
        gate: {
          pass: () => latest.current.gate?.pass() ?? true,
          spend: () => latest.current.gate?.spend(),
        },
      },
    ),
    [],
  );
}
