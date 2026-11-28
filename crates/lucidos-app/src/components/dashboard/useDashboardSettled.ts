import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { scaledDurationMs } from '../../utils/motion';
import { PANE_TRANSITION_MS } from '../layout/splitHelpers';
import type { FrameState } from '../widgets/widgetFrameState';

export const DASHBOARD_SETTLE_FUSE_MS = 3000;

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
