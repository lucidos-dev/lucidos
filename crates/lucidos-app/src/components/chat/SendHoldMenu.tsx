import { signal } from '@preact/signals';
import { useRef } from 'preact/hooks';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { Overlay } from '../shared/Overlay';

/** The Send button a hold opened this menu from, or null while it is shut. */
export const sendHoldMenuAnchor = signal<HTMLElement | null>(null);

/** What a long press (or a right-click) on Send offers: ask the composer's
 *  contents as a side question, with no `/btw` typed.
 *
 *  A hold opens it, so no control toggles it and the overlay takes no anchor
 *  (`.claude/rules/frontend.md` § Modals & Popovers). A tap on Send while it
 *  is open only dismisses it. */
export function SendHoldMenu({ onAskSideQuestion, onClosed }: {
  onAskSideQuestion: () => void;
  /** Runs whenever the menu shuts, however it was dismissed. */
  onClosed: () => void;
}) {
  const anchor = sendHoldMenuAnchor.value;
  const panelRef = useRef<HTMLDivElement>(null);
  const pos = useAnchoredPosition(anchor, panelRef, '.thread-pane', 'end');
  const close = () => {
    sendHoldMenuAnchor.value = null;
    onClosed();
  };
  return (
    <Overlay
      open={anchor !== null}
      onClose={close}
      anchor={null}
      backdrop={false}
      panelClass="surface-box send-hold-menu"
      panelStyle={pos ? { top: `${pos.top}px`, left: `${pos.left}px` } : { visibility: 'hidden' }}
      panelRef={panelRef}
    >
      <button
        type="button"
        data-role="ask-side-question"
        onClick={() => {
          close();
          onAskSideQuestion();
        }}
      >
        Ask as side question
      </button>
    </Overlay>
  );
}
