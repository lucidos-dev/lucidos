import { useEffect, useRef } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { pushOverlay, removeOverlay } from '../../store/overlayStack';

let overlayIdCounter = 0;

/** A dismissable modal backdrop. Escape-to-dismiss is no longer a per-instance
 *  `document` listener (those raced each other and the global key dispatcher).
 *  Instead, a dismissable overlay registers its `onClose` into the central
 *  `overlayStack`; the one capture-phase Escape dispatcher in
 *  `useKeyboardShortcuts` pops the top entry. Click-outside-to-dismiss stays
 *  local here. */
export function ModalOverlay({ onClose, class: cls, children }: {
  onClose?: () => void;
  class?: string;
  children: ComponentChildren;
}) {
  const idRef = useRef<string>();
  if (idRef.current === undefined) idRef.current = `modal-${++overlayIdCounter}`;

  // Keep the latest onClose in a ref so the registered dismiss handler always
  // calls the current one without re-registering on every render (onClose is
  // usually an inline arrow with a fresh identity each render).
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  const hasOnClose = !!onClose;
  useEffect(() => {
    if (!hasOnClose) return; // non-dismissable overlay — don't register
    const id = idRef.current!;
    pushOverlay({ id, dismiss: () => onCloseRef.current?.() });
    return () => removeOverlay(id);
  }, [hasOnClose]);

  return (
    <div
      class={`modal-overlay${cls ? ` ${cls}` : ''}`}
      onClick={onClose ? (e: MouseEvent) => {
        if (e.target === e.currentTarget) onClose();
      } : undefined}
    >
      {children}
    </div>
  );
}
