import { useState, useEffect } from 'preact/hooks';
import type { Loadable } from '../store/types';

/**
 * Returns true only after the loadable has been in 'loading' state
 * for at least `delayMs` milliseconds. This avoids flashing a spinner
 * for fast loads.
 */
export function useDelayedLoading(
  loadable: Loadable<unknown>,
  delayMs = 300
): boolean {
  const [showLoading, setShowLoading] = useState(false);

  useEffect(() => {
    if (loadable.status === 'loading') {
      const timer = setTimeout(() => setShowLoading(true), delayMs);
      return () => clearTimeout(timer);
    }
    setShowLoading(false);
  }, [loadable.status, delayMs]);

  return showLoading;
}
