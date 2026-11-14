import { useMemo, useRef } from 'preact/hooks';
import { touchActivated, type ActivationGate } from '../utils/tapGesture';

/** Bind `touchActivated` to a component.
 *
 *  The handlers must be STABLE, because the helper carries per-instance press
 *  state: the twin window. Rebuilding them each render would forget the touch it
 *  just served, and a browser that dispatched the suppressed click anyway would
 *  run the action twice. The action, the enable flag, the gate and the
 *  destructive flag must NOT be stable, since all four are read off the current
 *  render. So the set is memoized once and reads the latest of each through a
 *  ref.
 *
 *  `enabled` stands the touch path down; `gate` rules on the click path and is
 *  spent by the touch path, unless `destructive` makes that path ask it too.
 *  See `touchActivated`. */
export function useTouchActivated(
  action: () => void,
  enabled = true,
  gate?: ActivationGate,
  destructive = false,
) {
  const latest = useRef({ action, enabled, gate, destructive });
  latest.current = { action, enabled, gate, destructive };
  return useMemo(
    () => touchActivated(
      () => latest.current.action(),
      {
        enabled: () => latest.current.enabled,
        destructive: () => latest.current.destructive,
        gate: {
          pass: () => latest.current.gate?.pass() ?? true,
          spend: () => latest.current.gate?.spend(),
          aborted: () => latest.current.gate?.aborted?.() ?? false,
        },
      },
    ),
    [],
  );
}
