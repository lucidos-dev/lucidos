import type { ComponentChildren } from 'preact';
import { useRef } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Overlay } from './Overlay';
import { ChevronUpIcon } from './icons';
import { useTouchActivated } from '../../hooks/useTouchActivated';

export interface SplitButtonMenuItem {
  key: string;
  label: ComponentChildren;
  className: string;
  tooltip?: string;
  disabled?: boolean;
  heldBack?: boolean;
  onClick: () => void;
}

export interface SplitButtonProps {
  primaryLabel: ComponentChildren;
  primaryClassName: string;
  onPrimary: () => void;
  primaryTooltip?: string;
  primaryAriaLabel?: string;
  primaryDisabled?: boolean;
  primaryHeldBack?: boolean;
  caretClassName: string;
  caretAriaLabel: string;
  faceOpensMenu?: boolean;
  menuItems: SplitButtonMenuItem[];
  attrs?: Record<string, string>;
}

export function SplitButton(props: SplitButtonProps) {
  const open = useSignal(false);
  const caretRef = useRef<HTMLButtonElement>(null);
  const hasMenu = props.menuItems.length > 0;
  const close = () => { open.value = false; };
  const { attrs } = props;
  const toggle = () => { open.value = !open.value; };
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
