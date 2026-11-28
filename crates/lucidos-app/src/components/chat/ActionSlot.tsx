import type { ComponentChildren } from 'preact';
import { SplitButton, type SplitButtonMenuItem } from '../shared/SplitButton';
import { PROTECTED_SURFACE } from '../shared/protectedSurface';

export type SlotTone = 'confirm' | 'danger' | 'neutral';

export interface SlotAction {
  key: string;
  label: string;
  ariaLabel?: string;
  tooltip: string;
  tone: SlotTone;
  icon: () => ComponentChildren;
  invoke: (() => void) | null;
  heldBack?: boolean;
}

export interface SlotOccupant {
  face: SlotAction;
  menu: SlotAction[];
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
