import { PinIcon } from './icons';
import { ToggleIconButton } from './ToggleIconButton';

export function PinButton({ pinned, label, tooltip, onToggle, stopPropagation, extraClass, tabIndex }: {
  pinned: boolean;
  label: string;
  tooltip: string;
  onToggle: () => void;
  stopPropagation?: boolean;
  extraClass?: string;
  tabIndex?: number;
}) {
  return (
    <ToggleIconButton
      tabIndex={tabIndex}
      tone="glyph"
      pressed={pinned}
      class={`header-icon pin-btn${extraClass ? ` ${extraClass}` : ''}`}
      onClick={(e) => {
        if (stopPropagation) e.stopPropagation();
        onToggle();
      }}
      aria-label={label}
      data-tooltip={tooltip}
    >
      <PinIcon filled={pinned} />
    </ToggleIconButton>
  );
}
