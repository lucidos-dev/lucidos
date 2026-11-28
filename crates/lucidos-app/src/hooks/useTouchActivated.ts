import { useLayoutEffect, useMemo, useRef } from 'preact/hooks';
import { touchActivated, type ActivationGate } from '../utils/tapGesture';

export function useTouchActivated(
  action: () => void,
  enabled = true,
  gate?: ActivationGate,
  destructive = false,
  shownAt?: () => number | null,
) {
  const latest = useRef({ action, enabled, gate, destructive, shownAt });
  latest.current = { action, enabled, gate, destructive, shownAt };
  return useMemo(
    () => touchActivated(
      () => latest.current.action(),
      {
        enabled: () => latest.current.enabled,
        destructive: () => latest.current.destructive,
        gate: {
          pass: () => latest.current.gate?.pass() ?? true,
          spend: () => latest.current.gate?.spend(),
          aborted: () => latest.current.gate?.aborted?.() ?? false,
        },
        ...(shownAt ? { shownAt: () => latest.current.shownAt?.() ?? null } : {}),
      },
    ),
    [],
  );
}

export function useFaceShownAt(face: string): () => number | null {
  const shownAt = useRef<number | null>(null);
  useLayoutEffect(() => {
    shownAt.current = null;
    let task: ReturnType<typeof setTimeout> | null = null;
    const frame = requestAnimationFrame(() => {
      task = setTimeout(() => { shownAt.current = performance.now(); }, 0);
    });
    return () => {
      cancelAnimationFrame(frame);
      if (task !== null) clearTimeout(task);
    };
  }, [face]);
  return useMemo(() => () => shownAt.current, []);
}
