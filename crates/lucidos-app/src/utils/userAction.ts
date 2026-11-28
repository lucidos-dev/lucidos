
export const USER_ACTION_EVENTS = ['wheel', 'touchmove', 'pointerdown', 'keydown'] as const;

export function watchUserAction(handler: () => void): () => void {
  if (typeof document === 'undefined' || !document.addEventListener) return () => {};
  for (const type of USER_ACTION_EVENTS) {
    document.addEventListener(type, handler, { capture: true, passive: true });
  }
  return () => {
    for (const type of USER_ACTION_EVENTS) {
      document.removeEventListener(type, handler, { capture: true } as EventListenerOptions);
    }
  };
}
