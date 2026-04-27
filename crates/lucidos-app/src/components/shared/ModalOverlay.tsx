import { useEffect } from 'preact/hooks';
import type { ComponentChildren } from 'preact';

export function ModalOverlay({ onClose, class: cls, children }: {
  onClose?: () => void;
  class?: string;
  children: ComponentChildren;
}) {
  useEffect(() => {
    if (!onClose) return;
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose!();
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [onClose]);

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
