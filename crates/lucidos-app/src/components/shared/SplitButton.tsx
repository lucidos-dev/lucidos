import type { ComponentChildren } from 'preact';
import { useRef } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Overlay } from './Overlay';
import { ChevronUpIcon } from './icons';
import { useTouchActivated } from '../../hooks/useTouchActivated';

/** A one-tap primary face joined to a caret that opens an upward Overlay menu
 *  of secondary actions. The face + caret read as a single pill (the CSS
 *  `.split-button-*` classes strip the touching radii and add a hairline
 *  divider). When open, the wrapper gets `.open`: the menu (above) and a
 *  `.split-button.open::before` backing (behind the pills) form one slightly
 *  translucent frosted frame wrapping the button. Both are out of flow, so the
 *  button stays a steady base — opening changes only paint, never layout (see
 *  `.split-button.open` / `.split-button-menu` in host-components.css).
 *  The caret is the Overlay `anchor`, so re-tapping it closes via its
 *  own handler while a tap on the (inert-while-open) primary face dismisses the
 *  menu rather than firing the primary action by accident.
 *
 *  Reused by the change-action banner (Apply + Discard/Archive; the Diff button
 *  sits outside this cluster) and the chat prompt's multi-select answer control
 *  (Submit + Cancel). Pass the same class on `primaryClassName`/`caretClassName`
 *  to keep the pill one colour. */
export interface SplitButtonMenuItem {
  /** Stable key for the rendered <button>. */
  key: string;
  label: ComponentChildren;
  /** Full class incl. the `.action-btn` variant, e.g. `action-btn action-btn-danger`. */
  className: string;
  tooltip?: string;
  onClick: () => void;
}

export interface SplitButtonProps {
  primaryLabel: ComponentChildren;
  /** Full class incl. the `.action-btn` variant for the primary face. */
  primaryClassName: string;
  onPrimary: () => void;
  primaryTooltip?: string;
  primaryAriaLabel?: string;
  primaryDisabled?: boolean;
  /** Also fire the primary face on `touchend`, for a face the user reaches with
   *  the mobile keyboard up. Opt-in: WebKit drops the synthetic click when the
   *  tap blurs a focused field, and the composer's Submit is the face that
   *  lives against the keyboard.
   *
   *  The change-action banner's Apply leaves this OFF by choice, not by reach.
   *  It renders into the same `.prompt-actions-row`, and Diff beside it was
   *  reported dead with the keyboard up. But the touch path gives up the
   *  did-the-finger-stay-on-the-button half, so a press sliding off Apply would
   *  merge a branch. Diff only opens a view. See `touchActivated`. */
  primaryTouchActivate?: boolean;
  /** Full class for the caret button — usually identical to primaryClassName. */
  caretClassName: string;
  caretAriaLabel: string;
  menuItems: SplitButtonMenuItem[];
}

export function SplitButton(props: SplitButtonProps) {
  const open = useSignal(false);
  const caretRef = useRef<HTMLButtonElement>(null);
  const hasMenu = props.menuItems.length > 0;
  const close = () => { open.value = false; };
  // A disabled button dispatches no click. WebKit still dispatches touch events
  // on it, so the disabled state has to gate the touch path by hand.
  const primaryActivate = useTouchActivated(
    () => props.onPrimary(),
    !!props.primaryTouchActivate && !props.primaryDisabled,
  );
  return (
    <div class={`split-button${open.value ? ' open' : ''}`} data-row-item>
      <button
        class={`${props.primaryClassName} split-button-primary`}
        data-tooltip={props.primaryTooltip}
        aria-label={props.primaryAriaLabel}
        disabled={props.primaryDisabled}
        onTouchEnd={primaryActivate.onTouchEnd}
        onClick={primaryActivate.onClick}
      >
        {props.primaryLabel}
      </button>
      {hasMenu && (
        <button
          ref={caretRef}
          class={`${props.caretClassName} split-button-caret${open.value ? ' open' : ''}`}
          aria-label={props.caretAriaLabel}
          aria-haspopup="menu"
          aria-expanded={open.value}
          onClick={() => { open.value = !open.value; }}
        >
          <ChevronUpIcon size="1rem" />
        </button>
      )}
      <Overlay
        open={open.value && hasMenu}
        onClose={close}
        anchor={caretRef.current}
        backdrop={false}
        panelClass="split-button-menu"
        panelRole="menu"
      >
        {props.menuItems.map((item) => (
          <button
            key={item.key}
            role="menuitem"
            class={item.className}
            data-tooltip={item.tooltip}
            onClick={() => { close(); item.onClick(); }}
          >
            {item.label}
          </button>
        ))}
      </Overlay>
    </div>
  );
}
