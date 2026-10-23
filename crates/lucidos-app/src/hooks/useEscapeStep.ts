import { useLayoutEffect, useRef } from 'preact/hooks';
import { pushOverlay, removeOverlay } from '../store/overlayStack';

let stepIdCounter = 0;

/** Answer Escape with a step back instead of a close, while `onStep` is set.
 *
 *  Only the central overlay stack can say this: the Escape dispatcher runs in
 *  the capture phase and stops propagation, so a keydown handler never sees the
 *  key. This pushes an Escape-only registrant, which owns no pixels. The stack
 *  is LIFO, and a step always begins after its panel opened, so the step sits
 *  above the panel and answers first.
 *
 *  Pre-paint, like <Overlay>'s own push, so an Escape pressed straight after
 *  the tap that opened the step already finds it. The latest `onStep` is read
 *  at dismiss time, so a step that changes target keeps one stack entry. */
export function useEscapeStep(onStep: (() => void) | null): void {
  // Seeded lazily: `useRef(expr)` evaluates `expr` on every render, so a
  // template in the argument would bump the counter forever.
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
