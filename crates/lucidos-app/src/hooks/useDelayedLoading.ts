import { useState, useEffect } from 'preact/hooks';
import type { Loadable } from '../store/types';

/** Standard delay before a loading indicator becomes visible — fast loads
 *  finish inside this window and never flash a loader. The smooth exit (so a
 *  skeleton that DID appear doesn't hard-snap to content) is handled separately
 *  by `<LoadingFade>`, not by withholding content with a minimum-visible floor. */
export const SPINNER_DELAY_MS = 300;

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
 * like ThreadView's 8s "Taking too long? Tap to reload" timeout. The loader's
 * smooth EXIT (no hard snap from a shown skeleton to content) is handled by
 * `<LoadingFade>`, not by a minimum-visible floor — withholding ready content to
 * satisfy a minimum makes every slow load feel sluggish.
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
