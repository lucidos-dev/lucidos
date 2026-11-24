import { useState, useEffect, useLayoutEffect, useRef } from 'preact/hooks';
import type { Loadable } from '../store/types';

export const SPINNER_DELAY_MS = 300;

export const LOADER_MIN_VISIBLE_MS = 500;

export function delayedFlagEffect(
  active: boolean,
  setShow: (v: boolean) => void,
  delayMs: number,
): (() => void) | undefined {
  if (active) {
    const timer = setTimeout(() => setShow(true), delayMs);
    return () => clearTimeout(timer);
  }
  setShow(false);
  return undefined;
}

export function useDelayedFlag(active: boolean, delayMs = SPINNER_DELAY_MS): boolean {
  const [show, setShow] = useState(false);

  useEffect(() => delayedFlagEffect(active, setShow, delayMs), [active, delayMs]);

  return show;
}

export function useDelayedLoading(
  loadable: Loadable<unknown>,
  delayMs = SPINNER_DELAY_MS,
): boolean {
  return useDelayedFlag(loadable.status === 'loading', delayMs);
}

export function lingeringFlagEffect(
  active: boolean,
  setLingering: (v: boolean) => void,
  lingerMs: number,
): (() => void) | undefined {
  if (active) {
    setLingering(true);
    return undefined;
  }
  const timer = setTimeout(() => setLingering(false), lingerMs);
  return () => clearTimeout(timer);
}

export function useLingeringFlag(active: boolean, lingerMs: number): boolean {
  const [lingering, setLingering] = useState(active);

  useEffect(() => lingeringFlagEffect(active, setLingering, lingerMs), [active, lingerMs]);

  return active || lingering;
}

export type FloorTimer = { current: ReturnType<typeof setTimeout> | undefined };

export function minimumVisibleFlagEffect(
  active: boolean,
  setHeld: (v: boolean) => void,
  minMs: number,
  floor: FloorTimer,
): void {
  if (!active || floor.current !== undefined) return;
  setHeld(true);
  floor.current = setTimeout(() => {
    floor.current = undefined;
    setHeld(false);
  }, minMs);
}

export function useMinimumVisibleFlag(active: boolean, minMs = LOADER_MIN_VISIBLE_MS): boolean {
  const [held, setHeld] = useState(active);
  const floor = useRef<FloorTimer['current']>(undefined) as FloorTimer;

  useLayoutEffect(() => minimumVisibleFlagEffect(active, setHeld, minMs, floor), [active, minMs]);
  useEffect(() => () => clearTimeout(floor.current), []);

  return active || held;
}
