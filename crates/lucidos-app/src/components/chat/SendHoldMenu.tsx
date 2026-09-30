import { signal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import { Overlay } from '../shared/Overlay';
import { useTouchActivated } from '../../hooks/useTouchActivated';

/** What opened the split pill, or null while it is shut. */
export type SendHoldMenuOpener = 'hold' | 'shortcut';

export const sendHoldMenuOpener = signal<SendHoldMenuOpener | null>(null);

/** The half's slide in and out, `--duration-normal` at 1x. */
export const SEND_HOLD_SLIDE_MS = 200;
/** Fixed margin past the slide, so the leaving half never cuts off early. */
export const SEND_HOLD_SLIDE_SLACK_MS = 50;

/** The Side question half of the split pill Send or Stop opens into. A long
 *  press or a right-click on either opens it, and so does the Side question
 *  shortcut. From Send it asks the composer's contents as a side question,
 *  with no `/btw` typed. From Stop the box is empty, so it starts a `/btw`.
 *
 *  Send is the pill's other half, so it is the overlay's `anchor`: a tap on it
 *  sends and shuts the pill, rather than only dismissing it. Anywhere else
 *  dismisses (`.claude/rules/frontend.md` § Modals & Popovers).
 *
 *  The shortcut focuses this half so Enter asks, and Tab moves on to Send. A
 *  hold must not take focus: on touch that drops the keyboard. */
export function SendHoldMenu({ anchor, leaving, onAskSideQuestion, onClosed }: {
  anchor: HTMLElement | null;
  /** The pill has shut and its half is sliding back behind Send. */
  leaving: boolean;
  onAskSideQuestion: () => void;
  /** Runs whenever the pill shuts, however it was shut. */
  onClosed: (opener: SendHoldMenuOpener) => void;
}) {
  const opener = sendHoldMenuOpener.value;
  const buttonRef = useRef<HTMLButtonElement>(null);
  const previousOpener = useRef(opener);
  useEffect(() => {
    const was = previousOpener.current;
    previousOpener.current = opener;
    if (was !== null && opener === null) onClosed(was);
    if (opener === 'shortcut') buttonRef.current?.focus({ preventScroll: true });
  }, [opener]);
  const close = () => { sendHoldMenuOpener.value = null; };
  // Inside the touch, so a side question started from Stop can raise the iOS
  // keyboard. A synthetic click comes too late for that.
  const activate = useTouchActivated(() => {
    close();
    onAskSideQuestion();
  });
  return (
    <>
      <Overlay
        open={opener !== null}
        onClose={close}
        anchor={anchor}
        backdrop={false}
        panelClass="send-hold-menu"
      >
        <button
          ref={buttonRef}
          type="button"
          class="action-btn"
          data-role="ask-side-question"
          onTouchEnd={activate.onTouchEnd}
          onClick={activate.onClick}
        >
          Side question
        </button>
      </Overlay>
      {/* A drawing of the half, outside the overlay: the pill is already shut,
          so nothing here takes a press or holds the UI behind it inert. */}
      {leaving && (
        <div class="send-hold-menu send-hold-menu-leaving" aria-hidden="true">
          <span class="action-btn">Side question</span>
        </div>
      )}
    </>
  );
}
