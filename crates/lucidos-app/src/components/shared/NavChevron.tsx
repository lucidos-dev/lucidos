import type { ComponentChildren } from 'preact';
import { useRef, useState } from 'preact/hooks';
import { Overlay } from './Overlay';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { useLongPress } from '../../hooks/useLongPress';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { BackIcon, ForwardIcon } from './icons';

export interface NavHistoryItem {
  /** Stable key for the rendered row. */
  key: string;
  /** Human label for the destination (thread title / content view title). */
  label: string;
  /** Optional leading glyph identifying the destination's type — a thread-type
   *  mark (ThreadNav) or a content-pane category icon (ContentNav). */
  icon?: ComponentChildren;
  /** Jump to this destination. */
  onSelect: () => void;
}

interface NavChevronProps {
  direction: 'back' | 'forward';
  disabled: boolean;
  /** Single-step navigation — the plain click / tap action. */
  onStep: () => void;
  /** History for the long-press / right-click menu, ordered nearest-first (the
   *  entry one step away at the top). Called lazily, only while the menu is open. */
  getItems: () => NavHistoryItem[];
  ariaLabel: string;
  /** Pre-formatted tooltip text. Omit to render no tooltip (e.g. mobile). */
  tooltip?: string;
  /** Extra class on the button (e.g. `content-back-btn`, `thread-nav-btn`). */
  buttonClass?: string;
  /** Enable the long-press / right-click history menu. Default `true`. Turned
   *  off where the chevron drives a non-enumerable history (the Tauri embedded
   *  webview's own back/forward stack). */
  history?: boolean;
}

/** A back/forward chevron with a browser-style history popover: a long-press
 *  (touch or mouse hold) or right-click opens a list of the destinations that
 *  chevron walks toward, so the user can jump several steps at once. A plain
 *  click still does the single-step navigation. Used by both the thread-pane
 *  chevrons (`ThreadNav`) and the content-pane chevrons (`PanelNav`). */
export function NavChevron({
  direction, disabled, onStep, getItems, ariaLabel, tooltip, buttonClass, history = true,
}: NavChevronProps) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const pos = useAnchoredPosition(anchor, menuRef);
  const open = anchor !== null;

  // The popover drops over the content pane, where the Tauri internal browser's
  // native webview paints above all HTML — hide it while the menu is open so the
  // history list isn't rendered behind it (mirrors Dropdown / Drawer / dialogs).
  useHidePanelWebviewWhile(open);

  const close = () => setAnchor(null);
  // A tap while the menu is open toggles it shut (the anchor is exempt from the
  // Overlay's outside-dismiss, so without this it would step-navigate instead).
  const handleClick = () => { if (open) close(); else onStep(); };
  const longPress = useLongPress((el) => setAnchor(el), handleClick);

  const Icon = direction === 'back' ? BackIcon : ForwardIcon;
  const cls = `icon-btn header-icon${buttonClass ? ` ${buttonClass}` : ''}`;
  const items = open ? getItems() : [];

  const handlers = history
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
        panelClass="dropdown-menu nav-history-menu"
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
