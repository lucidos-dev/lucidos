/**
 * Seconds left on a minted pairing code, ticking while it lives.
 *
 * Shared by "Add a device" and "Pair a gateway", which mint the same kind of
 * five-minute code. A countdown is the honest version of "expires in five
 * minutes": a code left on screen stops working silently.
 */

import { useEffect, useState } from 'preact/hooks';

/** Pure: how long is left, in whole seconds, never below zero. */
export function secondsLeft(expiresAt: number, now: number): number {
  return Math.max(0, Math.ceil((expiresAt - now) / 1000));
}

/**
 * Seconds left until `expiresAt`, or 0 with no code.
 *
 * The clock is read on the render that first sees a new `expiresAt`. A clock
 * stamped earlier would open the countdown long, at "5:02" after a slow mint.
 */
export function useCodeCountdown(expiresAt: number | null): number {
  const [tick, setTick] = useState(() => ({ at: expiresAt, now: Date.now() }));
  const now = tick.at === expiresAt ? tick.now : Date.now();
  const left = expiresAt === null ? 0 : secondsLeft(expiresAt, now);
  const live = left > 0;

  // Ticks only while a live code is on screen, and stops once it is dead.
  useEffect(() => {
    setTick({ at: expiresAt, now: Date.now() });
    if (!live) return;
    const id = window.setInterval(() => setTick({ at: expiresAt, now: Date.now() }), 1000);
    return () => window.clearInterval(id);
  }, [expiresAt, live]);

  // A hidden tab throttles the interval, so `now` comes back stale and the
  // countdown would show time a dead code no longer has. Re-read on the way in.
  useEffect(() => {
    const onVisibility = () => {
      if (document.visibilityState !== 'hidden') setTick({ at: expiresAt, now: Date.now() });
    };
    document.addEventListener('visibilitychange', onVisibility);
    return () => document.removeEventListener('visibilitychange', onVisibility);
  }, [expiresAt]);

  return left;
}
