import type { JSX } from 'preact';
import type { AnchorPosition } from '../../hooks/useAnchoredPopover';

/** Inline style for the menu panel, given the trigger's width and the computed
 *  position (`null` until `useAnchoredPosition` has measured the panel).
 *
 *  Set `minWidth` before measuring: a body portal otherwise resolves the
 *  stylesheet's `min-width: 100%` against the viewport and misplaces the panel.
 *  Fixed positioning and zero offsets keep the hidden measurement box inside
 *  the viewport, at its final geometry.
 *
 *  `--anchor-room` supplies the height cap on the opening side. Leave it absent
 *  while measuring so the first pass sees the menu's natural height. */
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

/** The trigger's chevron. It draws one glyph in both states, and `.open` turns
 *  it over. Never swap in an up glyph: it can differ in width and resize the
 *  trigger. */
export function DropdownChevron({ open, onClick }: { open: boolean; onClick?: () => void }) {
  return <span class={`dropdown-chevron${open ? ' open' : ''}`} onClick={onClick}>▾</span>;
}

