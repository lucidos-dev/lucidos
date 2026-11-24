import { useEffect, useLayoutEffect } from 'preact/hooks';
import { focusIfNeeded } from '../utils/dom';

export function useFocusWhenPlaced(target: () => HTMLElement | null | undefined, deps: unknown[]): void {
  useLayoutEffect(() => focusIfNeeded(target()), deps);
  useEffect(() => {
    focusIfNeeded(target());
    const id = requestAnimationFrame(() => focusIfNeeded(target()));
    return () => cancelAnimationFrame(id);
  }, deps);
}
