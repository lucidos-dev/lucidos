
import { useEffect, useState } from 'preact/hooks';

export function secondsLeft(expiresAt: number, now: number): number {
  return Math.max(0, Math.ceil((expiresAt - now) / 1000));
}

export function useCodeCountdown(expiresAt: number | null): number {
  const [tick, setTick] = useState(() => ({ at: expiresAt, now: Date.now() }));
  const now = tick.at === expiresAt ? tick.now : Date.now();
  const left = expiresAt === null ? 0 : secondsLeft(expiresAt, now);
  const live = left > 0;

  useEffect(() => {
    setTick({ at: expiresAt, now: Date.now() });
    if (!live) return;
    const id = window.setInterval(() => setTick({ at: expiresAt, now: Date.now() }), 1000);
    return () => window.clearInterval(id);
  }, [expiresAt, live]);

  useEffect(() => {
    const onVisibility = () => {
      if (document.visibilityState !== 'hidden') setTick({ at: expiresAt, now: Date.now() });
    };
    document.addEventListener('visibilitychange', onVisibility);
    return () => document.removeEventListener('visibilitychange', onVisibility);
  }, [expiresAt]);

  return left;
}
