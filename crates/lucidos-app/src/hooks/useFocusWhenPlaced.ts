import { useEffect, useLayoutEffect } from 'preact/hooks';
import { focusIfNeeded } from '../utils/dom';

/** Focus what `target` names, in a panel that may not be placed yet.
 *
 *  An anchored panel stays `visibility: hidden` until its position lands, in
 *  an effect after paint, and a hidden element cannot take focus. So this asks
 *  three times. In the commit, which wins on a panel already on screen: a step
 *  change then moves the keyboard inside the tap that caused it. After paint,
 *  and a frame after that, which win on a panel that has just opened. */
export function useFocusWhenPlaced(target: () => HTMLElement | null | undefined, deps: unknown[]): void {
  useLayoutEffect(() => focusIfNeeded(target()), deps);
  useEffect(() => {
    focusIfNeeded(target());
    const id = requestAnimationFrame(() => focusIfNeeded(target()));
    return () => cancelAnimationFrame(id);
  }, deps);
}
