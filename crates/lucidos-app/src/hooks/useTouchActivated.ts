import { useLayoutEffect, useMemo, useRef } from 'preact/hooks';
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
 *  `shownAt` is the getter `useFaceShownAt` returns. See `touchActivated`. */
export function useTouchActivated(
  action: () => void,
  enabled = true,
  gate?: ActivationGate,
  destructive = false,
  shownAt?: () => number | null,
) {
  const latest = useRef({ action, enabled, gate, destructive, shownAt });
  latest.current = { action, enabled, gate, destructive, shownAt };
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
        // Only when the caller tracks the face. An untracked face has no paint
        // time, and a null one would refuse every press.
        ...(shownAt ? { shownAt: () => latest.current.shownAt?.() ?? null } : {}),
      },
    ),
    [],
  );
}

/** When the face named by `face` was first painted, on the event `timeStamp`
 *  clock, as a stable getter. Null from the render that changes the face until
 *  the frame that paints it has finished.
 *
 *  Read in a task posted from the frame callback, which runs after that frame's
 *  layout and paint. The frame callback's own time is the frame's START, so a
 *  stall inside the paint would stamp the face before the user could see it. */
export function useFaceShownAt(face: string): () => number | null {
  const shownAt = useRef<number | null>(null);
  useLayoutEffect(() => {
    shownAt.current = null;
    let task: ReturnType<typeof setTimeout> | null = null;
    const frame = requestAnimationFrame(() => {
      task = setTimeout(() => { shownAt.current = performance.now(); }, 0);
    });
    return () => {
      cancelAnimationFrame(frame);
      if (task !== null) clearTimeout(task);
    };
  }, [face]);
  return useMemo(() => () => shownAt.current, []);
}
