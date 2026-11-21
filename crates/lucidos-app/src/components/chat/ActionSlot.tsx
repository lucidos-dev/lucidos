import type { ComponentChildren } from 'preact';
import { SplitButton, type SplitButtonMenuItem } from '../shared/SplitButton';
import { PROTECTED_SURFACE } from '../shared/protectedSurface';

/** A slot action's colour. */
export type SlotTone = 'confirm' | 'danger' | 'neutral';

/** One action the action slot can carry, as its face or as a row of its menu. */
export interface SlotAction {
  key: string;
  /** The face's word, short enough for the fixed pill. */
  label: string;
  /** The full name, when the word alone says less than it should. */
  ariaLabel?: string;
  tooltip: string;
  tone: SlotTone;
  icon: () => ComponentChildren;
  /** What a press does. A busy action has none: it shows a spinner, disabled. */
  invoke: (() => void) | null;
  /** Tappable but held back: it reads disabled, and a press says why. */
  heldBack?: boolean;
}

/** The one control in the action slot, where Send stands: a face, and every
 *  other action in its menu. */
export interface SlotOccupant {
  face: SlotAction;
  menu: SlotAction[];
  /** The accessible name of the part that opens the menu. */
  menuLabel: string;
}

const TONE_CLASS: Record<SlotTone, string> = {
  confirm: 'action-btn action-btn-confirm',
  danger: 'action-btn action-btn-danger',
  neutral: 'action-btn',
};

function classOf(action: SlotAction): string {
  return `${TONE_CLASS[action.tone]}${action.heldBack ? ' action-btn-blocked' : ''}`;
}

/** The face's icon and word. The icon holds one spot at the left and the word
 *  centres on the whole button (`.action-slot` in host-components.css). The
 *  compact faces show the icon alone, and the word stays the accessible name. */
export function SlotFace({ icon, label, busy = false }: { icon: () => ComponentChildren; label: string; busy?: boolean }) {
  return (
    <>
      <span class="action-slot-icon" aria-hidden="true">
        {busy ? <span class="mini-spinner" /> : icon()}
      </span>
      <span class="action-slot-label">{label}</span>
    </>
  );
}

function SlotButton({ action }: { action: SlotAction }) {
  return (
    <button
      type="button"
      data-row-item
      data-thread-action=""
      class={`${classOf(action)} action-slot ${PROTECTED_SURFACE}`}
      disabled={action.invoke === null}
      aria-disabled={action.heldBack || undefined}
      aria-label={action.ariaLabel}
      data-tooltip={action.tooltip}
      onClick={() => action.invoke?.()}
    >
      <SlotFace icon={action.icon} label={action.label} busy={action.invoke === null} />
    </button>
  );
}

function menuItem(action: SlotAction): SplitButtonMenuItem {
  return {
    key: action.key,
    label: action.label,
    className: classOf(action),
    tooltip: action.tooltip,
    heldBack: action.heldBack,
    onClick: () => action.invoke?.(),
  };
}

/** The action slot's occupant. Alone, it is one button. With a menu, it is a
 *  split button on the desktop face. On the compact face it is one round button
 *  that opens the menu, face included, on every tap, so no tap acts at once. */
export function ActionSlot({ occupant, compact }: { occupant: SlotOccupant; compact: boolean }) {
  const { face, menu } = occupant;
  if (menu.length === 0) return <SlotButton action={face} />;
  return (
    <SplitButton
      primaryLabel={<SlotFace icon={face.icon} label={face.label} />}
      primaryClassName={`${classOf(face)} action-slot`}
      primaryTooltip={face.tooltip}
      primaryAriaLabel={compact ? `${face.ariaLabel ?? face.label}. ${occupant.menuLabel}` : face.ariaLabel}
      primaryDisabled={face.invoke === null}
      primaryHeldBack={face.heldBack}
      onPrimary={() => face.invoke?.()}
      caretClassName={classOf(face)}
      caretAriaLabel={occupant.menuLabel}
      faceOpensMenu={compact}
      menuItems={(compact ? [face, ...menu] : menu).map(menuItem)}
      attrs={{ 'data-thread-action': '' }}
    />
  );
}
