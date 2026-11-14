import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { scaledDurationMs } from '../../utils/motion';
import { PANE_TRANSITION_MS } from '../layout/splitHelpers';
import type { FrameState } from '../widgets/widgetFrameState';

/** How long a shown dashboard waits for its frames before it shows anyway. A
 *  late tile keeps its own cover and load bar. */
export const DASHBOARD_SETTLE_FUSE_MS = 3000;

/** A dashboard is settled when no widget tile is still on its way to being
 *  seen. A tile with no report yet is unknown, and holds it. An off-screen tile
 *  and an app shortcut, which has no frame, never do, unless `offScreenCounts`
 *  is false: a dashboard still sliding in reads off-screen wrongly. */
export function dashboardSettled(
  widgetKeys: readonly string[],
  states: ReadonlyMap<string, FrameState>,
  offScreenCounts: boolean,
): boolean {
  return widgetKeys.every((key) => {
    const state = states.get(key) ?? 'unknown';
    return state === 'revealed' || (state === 'off-screen' && offScreenCounts);
  });
}

/** Whether a dashboard has settled, once per mount: it never unsettles, so a
 *  tile added later or a widget reloading never blanks it again.
 *
 *  `ready` is the tile list having loaded. A hidden dashboard does not settle
 *  and ignores what its tiles report, because a hidden one mounts no frames.
 *  It starts afresh when shown, and trusts an off-screen answer only once its
 *  slide in has ended. Past `DASHBOARD_SETTLE_FUSE_MS` shown and ready, it
 *  settles whatever its tiles say. */
export function useDashboardSettled(ready: boolean, hidden: boolean, widgetKeys: readonly string[]) {
  const [states, setStates] = useState<ReadonlyMap<string, FrameState>>(new Map());
  const [arriving, setArriving] = useState(hidden);
  const [latched, setLatched] = useState(false);
  const hiddenRef = useRef(hidden);
  hiddenRef.current = hidden;

  const report = useCallback((key: string, state: FrameState) => {
    if (hiddenRef.current) return;
    setStates((prev) => (prev.get(key) === state ? prev : new Map(prev).set(key, state)));
  }, []);

  useEffect(() => {
    if (hidden) {
      setStates(new Map());
      setArriving(true);
      return undefined;
    }
    const arrived = setTimeout(() => setArriving(false), scaledDurationMs(PANE_TRANSITION_MS) + 100);
    return () => clearTimeout(arrived);
  }, [hidden]);

  const met = ready && !hidden && dashboardSettled(widgetKeys, states, !arriving);
  useEffect(() => {
    if (met) setLatched(true);
  }, [met]);

  const waiting = ready && !hidden && !latched;
  useEffect(() => {
    if (!waiting) return undefined;
    const fuse = setTimeout(() => setLatched(true), DASHBOARD_SETTLE_FUSE_MS);
    return () => clearTimeout(fuse);
  }, [waiting]);

  return { settled: latched || met, report };
}
