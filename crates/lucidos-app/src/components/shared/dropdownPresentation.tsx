import type { JSX } from 'preact';
import type { AnchorPosition } from '../../hooks/useAnchoredPopover';

export function dropdownPanelStyle(
  anchorWidth: number | null,
  pos: Pick<AnchorPosition, 'top' | 'left' | 'maxHeight'> | null,
): JSX.CSSProperties {
  if (anchorWidth === null) return { visibility: 'hidden' };
  return {
    position: 'fixed',
    minWidth: `${anchorWidth}px`,
    ...(pos
      ? { top: `${pos.top}px`, left: `${pos.left}px`, '--anchor-room': `${pos.maxHeight}px` }
      : { top: '0px', left: '0px', visibility: 'hidden' }),
  };
}

export function DropdownChevron({ open, onClick }: { open: boolean; onClick?: () => void }) {
  return <span class={`dropdown-chevron${open ? ' open' : ''}`} onClick={onClick}>▾</span>;
}

