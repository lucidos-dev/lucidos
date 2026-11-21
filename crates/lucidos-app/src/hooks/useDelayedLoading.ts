import { useState, useEffect, useLayoutEffect, useRef } from 'preact/hooks';
import type { Loadable } from '../store/types';

/** Standard delay before a loading indicator becomes visible — fast loads
 *  finish inside this window and never flash a loader. */
export const SPINNER_DELAY_MS = 300;

/** Minimum time a loader the delay gate let through stays opaque, measured from
 *  when it appeared. Without it a load ending just past `SPINNER_DELAY_MS` flashes
 *  a skeleton for a few frames. The exit owners (`<LoadingFade>`,
 *  `ThreadSkeletonOverlay`) hold it; a legibility floor, so the animation-speed
 *  slider does not scale it. */
export const LOADER_MIN_VISIBLE_MS = 500;

/** The effect body behind useDelayedFlag, extracted so tests can drive it
 *  with fake timers (see useDelayedLoading.test.ts). Returns the cleanup
 *  that cancels a still-pending timer. */
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

/**
 * Returns true only after `active` has been true for at least `delayMs`
 * milliseconds; resets to false immediately when `active` drops. Backs
 * `useDelayedLoading` and any hook-gated loader/skeleton, plus longer-fuse flags
 * like ThreadView's 8s "Taking too long? Tap to reload" timeout. A loader's EXIT
 * (smooth fade, minimum-visible floor) belongs to `<LoadingFade>` through
 * `useMinimumVisibleFlag`, never to this gate: it also backs non-loader flags.
 */
export function useDelayedFlag(active: boolean, delayMs = SPINNER_DELAY_MS): boolean {
  const [show, setShow] = useState(false);

  useEffect(() => delayedFlagEffect(active, setShow, delayMs), [active, delayMs]);

  return show;
}

/**
 * Returns true only after the loadable has been in 'loading' state for at least
 * `delayMs` milliseconds. This avoids flashing a loader for fast loads. Pair the
 * returned flag with `<LoadingFade>` to smooth the skeleton→content exit.
 */
export function useDelayedLoading(
  loadable: Loadable<unknown>,
  delayMs = SPINNER_DELAY_MS,
): boolean {
  return useDelayedFlag(loadable.status === 'loading', delayMs);
}

/** The effect body behind useLingeringFlag, extracted so tests can drive it
 *  with fake timers (same pattern as delayedFlagEffect above). */
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

/**
 * The inverse of useDelayedFlag: returns true while `active`, and keeps
 * returning true for `lingerMs` after it flips false — so exit animations
 * can play before content unmounts (e.g. the thread drawer's list during
 * the drawer's width-collapse transition).
 */
export function useLingeringFlag(active: boolean, lingerMs: number): boolean {
  const [lingering, setLingering] = useState(active);

  useEffect(() => lingeringFlagEffect(active, setLingering, lingerMs), [active, lingerMs]);

  return active || lingering;
}

/** The pending floor timer, kept in a ref so the effect body stays pure. */
export type FloorTimer = { current: ReturnType<typeof setTimeout> | undefined };

/** The effect body behind useMinimumVisibleFlag, extracted so tests can drive it
 *  with fake timers (same pattern as delayedFlagEffect above). Runs when `active`
 *  changes and starts the floor on the rise; a drop schedules nothing, so the
 *  timer from the rise keeps running. */
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

/**
 * Holds a delay-gated loader flag true until it has been visible for `minMs`:
 * returns true while `active`, and after it drops stays true for whatever remains
 * of `minMs` since it rose. A load still running past `minMs` ends the moment
 * `active` drops. Never wrap the gate's input with this: pass the gate's OUTPUT,
 * so a loader that never appeared never holds anything. The rise starts its clock in a
 * layout effect, so a drop before the next paint cannot beat it.
 */
export function useMinimumVisibleFlag(active: boolean, minMs = LOADER_MIN_VISIBLE_MS): boolean {
  const [held, setHeld] = useState(active);
  const floor = useRef<FloorTimer['current']>(undefined) as FloorTimer;

  useLayoutEffect(() => minimumVisibleFlagEffect(active, setHeld, minMs, floor), [active, minMs]);
  useEffect(() => () => clearTimeout(floor.current), []);

  return active || held;
}
