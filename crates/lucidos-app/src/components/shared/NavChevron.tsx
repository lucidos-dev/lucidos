import type { ComponentChildren } from 'preact';
import { useRef, useState } from 'preact/hooks';
import { Overlay } from './Overlay';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { useLongPress } from '../../hooks/useLongPress';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { BackIcon, ForwardIcon } from './icons';

export interface NavHistoryItem {
  key: string;
  label: string;
  icon?: ComponentChildren;
  onSelect: () => void;
}

interface NavChevronProps {
  direction: 'back' | 'forward';
  disabled: boolean;
  onStep: () => void;
  getItems: () => NavHistoryItem[];
  ariaLabel: string;
  tooltip?: string;
  buttonClass?: string;
  history?: boolean;
}

export function NavChevron({
  direction, disabled, onStep, getItems, ariaLabel, tooltip, buttonClass, history = true,
}: NavChevronProps) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const pos = useAnchoredPosition(anchor, menuRef);
  const open = anchor !== null;

  useHidePanelWebviewWhile(open);

  const close = () => setAnchor(null);
  const handleClick = () => { if (open) close(); else onStep(); };
  const longPress = useLongPress((el) => setAnchor(el), handleClick);

  const Icon = direction === 'back' ? BackIcon : ForwardIcon;
  const cls = `icon-btn header-icon nav-chevron${buttonClass ? ` ${buttonClass}` : ''}`;
  const items = open ? getItems() : [];

  const handlers = disabled
    ? {}
    : history
    ? {
        onClick: longPress.onClick,
        onPointerDown: longPress.onPointerDown,
        onPointerMove: longPress.onPointerMove,
        onPointerUp: longPress.onPointerUp,
        onPointerLeave: longPress.onPointerLeave,
        onPointerCancel: longPress.onPointerCancel,
        onContextMenu: longPress.onContextMenu,
      }
    : { onClick: onStep };

  return (
    <>
      <button
        class={cls}
        disabled={disabled}
        aria-label={ariaLabel}
        {...(tooltip ? { 'data-tooltip': tooltip } : {})}
        {...handlers}
      >
        <Icon />
      </button>
      <Overlay
        open={open}
        onClose={close}
        anchor={anchor}
        backdrop={false}
        portal
        panelClass="surface-box dropdown-menu nav-history-menu"
        panelRef={menuRef}
        panelStyle={anchor && pos
          ? { position: 'fixed', top: `${pos.top}px`, left: `${pos.left}px` }
          : { visibility: 'hidden' }}
      >
        {items.length === 0 ? (
          <div class="dropdown-option dropdown-no-results">No history</div>
        ) : items.map((item) => (
          <div
            key={item.key}
            class="dropdown-option"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => { item.onSelect(); close(); }}
          >
            {item.icon != null && <span class="nav-history-icon" aria-hidden="true">{item.icon}</span>}
            <div class="dropdown-option-label">{item.label}</div>
          </div>
        ))}
      </Overlay>
    </>
  );
}
