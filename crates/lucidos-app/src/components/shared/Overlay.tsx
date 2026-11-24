import { createContext } from 'preact';
import { useCallback, useContext, useRef, useLayoutEffect, useState } from 'preact/hooks';
import { createPortal } from 'preact/compat';
import type { ComponentChildren, RefObject, JSX } from 'preact';
import { useDismissOnOutside } from '../../hooks/useAnchoredPopover';
import { overlayModalAttrs, useOverlayFocus } from './overlayFocus';
import { leaveExitDrawing, type ExitLook } from './overlayExitDrawing';
import {
  overlayStack,
  overlayStackDepth,
  pushOverlay,
  removeOverlay,
  topPanelOverlay,
} from '../../store/overlayStack';

let overlayIdCounter = 0;

const PortalTarget = createContext<HTMLElement | null>(null);

let openOverlayCount = 0;
function acquireOverlayInert(): void {
  if (typeof document === 'undefined') return;
  openOverlayCount += 1;
  if (openOverlayCount === 1) document.documentElement.setAttribute('data-overlay-open', '');
}
function releaseOverlayInert(): void {
  if (typeof document === 'undefined') return;
  openOverlayCount = Math.max(0, openOverlayCount - 1);
  if (openOverlayCount === 0) document.documentElement.removeAttribute('data-overlay-open');
}

const anchorMarks = new WeakMap<HTMLElement, number>();

function markAnchorInteractive(anchor: HTMLElement | null): () => void {
  if (!anchor) return () => {};
  anchorMarks.set(anchor, (anchorMarks.get(anchor) ?? 0) + 1);
  anchor.setAttribute('data-overlay-anchor', '');
  return () => {
    const left = (anchorMarks.get(anchor) ?? 1) - 1;
    if (left > 0) {
      anchorMarks.set(anchor, left);
      return;
    }
    anchorMarks.delete(anchor);
    anchor.removeAttribute('data-overlay-anchor');
  };
}

export interface OverlayProps {
  open: boolean;
  onClose: () => void | boolean;
  anchor?: HTMLElement | null;
  backdrop?: boolean;
  overlayClass?: string;
  panelClass?: string;
  panelStyle?: string | JSX.CSSProperties;
  panelRole?: JSX.HTMLAttributes<HTMLDivElement>['role'];
  ariaModal?: boolean;
  dataRole?: string;
  panelProps?: JSX.HTMLAttributes<HTMLDivElement>;
  keepMounted?: boolean;
  hiddenClass?: string;
  panelRef?: RefObject<HTMLDivElement>;
  portal?: boolean;
  children: ComponentChildren;
}

export function Overlay({
  open,
  onClose,
  anchor = null,
  backdrop = true,
  overlayClass,
  panelClass,
  panelStyle,
  panelRole,
  ariaModal,
  dataRole,
  panelProps,
  keepMounted = false,
  hiddenClass,
  panelRef: externalPanelRef,
  portal = false,
  children,
}: OverlayProps) {
  const usePortal = portal && !backdrop && typeof document !== 'undefined';
  const portalTarget = useContext(PortalTarget);
  const internalRef = useRef<HTMLDivElement>(null);
  const panelRef = externalPanelRef ?? internalRef;
  const [modalPanel, setModalPanel] = useState<HTMLElement | null>(null);
  useLayoutEffect(() => {
    if (backdrop) setModalPanel(open ? panelRef.current : null);
  }, [open, backdrop]);

  const latest = useRef({ panelRef, exitLook: null as ExitLook | null });
  latest.current.panelRef = panelRef;
  const attachPanel = useCallback((el: HTMLDivElement | null) => {
    const { panelRef: ref, exitLook } = latest.current;
    if (!el && ref.current && exitLook) leaveExitDrawing(ref.current, exitLook);
    ref.current = el;
  }, []);

  const idRef = useRef<string>();
  if (idRef.current === undefined) idRef.current = `overlay-${++overlayIdCounter}`;

  useDismissOnOutside(
    open, panelRef, anchor, onClose, () => topPanelOverlay()?.id === idRef.current, !backdrop,
  );

  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useLayoutEffect(() => {
    if (!open) return;
    const id = idRef.current!;
    pushOverlay({ id, dismiss: () => onCloseRef.current(), hasPanel: true });
    acquireOverlayInert();
    return () => {
      removeOverlay(id);
      releaseOverlayInert();
    };
  }, [open]);

  useLayoutEffect(() => markAnchorInteractive(open ? anchor : null), [open, anchor]);

  const modal = backdrop || !!ariaModal;
  useOverlayFocus(open, panelRef, modal, anchor);

  if (!open) {
    if (!keepMounted) return null;
    const cls = backdrop
      ? ['modal-overlay', overlayClass, hiddenClass].filter(Boolean).join(' ')
      : [panelClass, hiddenClass].filter(Boolean).join(' ');
    const hidden = <div class={cls} aria-hidden="true" />;
    return usePortal ? createPortal(hidden, portalTarget ?? document.body) : hidden;
  }

  const panel = (
    <div
      {...panelProps}
      ref={attachPanel}
      class={panelClass}
      style={panelStyle}
      role={panelRole}
      aria-modal={ariaModal}
      data-role={dataRole}
      data-overlay-panel={idRef.current}
      {...overlayModalAttrs(modal)}
    >
      {backdrop ? <PortalTarget.Provider value={modalPanel}>{children}</PortalTarget.Provider> : children}
    </div>
  );

  if (!backdrop) {
    latest.current.exitLook = null;
    return usePortal ? createPortal(panel, portalTarget ?? document.body) : panel;
  }
  const cls = ['modal-overlay', overlayClass].filter(Boolean).join(' ');
  const z = `z-index: calc(var(--z-modal) + ${overlayStackDepth(overlayStack.value, idRef.current)})`;
  latest.current.exitLook = { cls, style: z };
  return <div class={cls} style={z}>{panel}</div>;
}
