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
 *  sits outside this cluster) and each pending row in the Changes panel (Apply +
 *  Set aside/Discard). Pass the same class on `primaryClassName`/`caretClassName`
 *  to keep the pill one colour. Both are approvals, so the face, the caret and
 *  the menu are each a protected surface (ADR 0309). The root is not: its
 *  stacking context would trap the menu under the controls beside it. */
export interface SplitButtonMenuItem {
  /** Stable key for the rendered <button>. */
  key: string;
  label: ComponentChildren;
  /** Full class incl. the `.action-btn` variant, e.g. `action-btn action-btn-danger`. */
  className: string;
  tooltip?: string;
  disabled?: boolean;
  /** Reads disabled but still takes a press, which says why. */
  heldBack?: boolean;
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
  /** The face reads disabled but still takes a press, which says why. */
  primaryHeldBack?: boolean;
  /** Full class for the caret button — usually identical to primaryClassName. */
  caretClassName: string;
  caretAriaLabel: string;
  /** Draw the face alone, as the menu's toggle: every tap opens the menu and
   *  none acts at once. The host lists the face's own action in `menuItems`.
   *  The face then takes `primaryAriaLabel` and ignores the primary props that
   *  act. The composer's round action button on a phone uses this. */
  faceOpensMenu?: boolean;
  /** Most important first. The menu opens upward, so it draws them from the
   *  button out: the first item sits next to the button, the last on top. */
  menuItems: SplitButtonMenuItem[];
  /** Attributes for the root box, from a host whose row is MEASURED.
   *
   *  The root always carries `data-row-item`, because the composer row measures
   *  its split buttons. The measurement looks only inside that row, so a split
   *  button elsewhere is unaffected. A host spreads these AFTER it, so a
   *  foldable one can overwrite the marker with its own fold key. */
  attrs?: Record<string, string>;
}

export function SplitButton(props: SplitButtonProps) {
  const open = useSignal(false);
  const caretRef = useRef<HTMLButtonElement>(null);
  const hasMenu = props.menuItems.length > 0;
  const close = () => { open.value = false; };
  const { attrs } = props;
  const toggle = () => { open.value = !open.value; };
  // The round face lives against the phone keyboard, where WebKit drops the
  // synthetic click. Opening a menu is harmless, so it takes the touch path.
  const wholeActivate = useTouchActivated(toggle, !!props.faceOpensMenu);
  const whole = (
    <button
      ref={caretRef}
      class={`${props.primaryClassName} split-button-whole protected-surface${open.value ? ' open' : ''}`}
      data-tooltip={props.primaryTooltip}
      aria-label={props.primaryAriaLabel}
      aria-disabled={props.primaryHeldBack || undefined}
      aria-haspopup="menu"
      aria-expanded={open.value}
      onTouchStart={wholeActivate.onTouchStart}
      onTouchMove={wholeActivate.onTouchMove}
      onTouchCancel={wholeActivate.onTouchCancel}
      onTouchEnd={wholeActivate.onTouchEnd}
      onClick={wholeActivate.onClick}
    >
      {props.primaryLabel}
    </button>
  );
  const faceAndCaret = (
    <>
      <button
        class={`${props.primaryClassName} split-button-primary protected-surface`}
        data-tooltip={props.primaryTooltip}
        aria-label={props.primaryAriaLabel}
        aria-disabled={props.primaryHeldBack || undefined}
        disabled={props.primaryDisabled}
        onClick={props.onPrimary}
      >
        {props.primaryLabel}
      </button>
      {hasMenu && (
        <button
          ref={caretRef}
          class={`${props.caretClassName} split-button-caret protected-surface${open.value ? ' open' : ''}`}
          aria-label={props.caretAriaLabel}
          aria-haspopup="menu"
          aria-expanded={open.value}
          onClick={toggle}
        >
          <ChevronUpIcon size="1rem" />
        </button>
      )}
    </>
  );
  return (
    <div data-row-item {...attrs} class={`split-button${open.value ? ' open' : ''}`}>
      {props.faceOpensMenu ? whole : faceAndCaret}
      <Overlay
        open={open.value && hasMenu}
        onClose={close}
        anchor={caretRef.current}
        backdrop={false}
        panelClass="split-button-menu protected-surface"
        panelRole="menu"
      >
        {[...props.menuItems].reverse().map((item) => (
          <button
            key={item.key}
            role="menuitem"
            class={item.className}
            data-tooltip={item.tooltip}
            disabled={item.disabled}
            aria-disabled={item.heldBack || undefined}
            onClick={() => { close(); item.onClick(); }}
          >
            {item.label}
          </button>
        ))}
      </Overlay>
    </div>
  );
}
