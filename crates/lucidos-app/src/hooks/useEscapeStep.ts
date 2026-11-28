import { useLayoutEffect, useRef } from 'preact/hooks';
import { pushOverlay, removeOverlay } from '../store/overlayStack';

let stepIdCounter = 0;

export function useEscapeStep(onStep: (() => void) | null): void {
  const id = useRef('');
  if (!id.current) id.current = `escape-step-${++stepIdCounter}`;
  const latest = useRef(onStep);
  latest.current = onStep;
  const active = onStep !== null;

  useLayoutEffect(() => {
    if (!active) return;
    const entryId = id.current;
    pushOverlay({ id: entryId, dismiss: () => latest.current?.(), hasPanel: false });
    return () => removeOverlay(entryId);
  }, [active]);
}
