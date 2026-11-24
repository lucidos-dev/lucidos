import { effect } from '@preact/signals';
import { createPortal } from 'preact/compat';
import { useLayoutEffect, useRef } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { appFullscreenHost } from '../../store/appFullscreenHost';
import { clientRefreshing } from '../../hooks/sw-update';

export function overlayLayerTarget(
  host: HTMLElement | null,
  refreshing: boolean,
): HTMLElement | 'home' {
  if (refreshing) return 'home';
  return host ?? 'home';
}

let layerNode: HTMLElement | null = null;

export function overlayLayerNode(): HTMLElement {
  if (!layerNode) {
    layerNode = document.createElement('div');
    layerNode.className = 'overlay-layer';
  }
  return layerNode;
}

type TextField = HTMLInputElement | HTMLTextAreaElement;
type MovableParent = HTMLElement & { moveBefore?: (node: Node, child: Node | null) => void };

export function moveOverlayLayer(node: HTMLElement, parent: HTMLElement): void {
  if (node.parentNode === parent && parent.lastChild === node) return;
  const target = parent as MovableParent;
  if (typeof target.moveBefore === 'function' && node.isConnected && parent.isConnected) {
    target.moveBefore(node, null);
    return;
  }
  const active = document.activeElement as HTMLElement | null;
  const focused = active && node.contains(active) ? active : null;
  const field = focused && 'selectionStart' in focused ? (focused as TextField) : null;
  const selection = field
    ? [field.selectionStart, field.selectionEnd, field.selectionDirection] as const
    : null;
  parent.appendChild(node);
  if (!focused || document.activeElement === focused) return;
  focused.focus({ preventScroll: true });
  if (field && selection && selection[0] !== null && selection[1] !== null) {
    field.setSelectionRange(selection[0], selection[1], selection[2] ?? undefined);
  }
}

export function OverlayLayer({ children }: { children: ComponentChildren }) {
  const homeRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => effect(() => {
    const target = overlayLayerTarget(appFullscreenHost.value, clientRefreshing.value);
    const parent = target === 'home' ? homeRef.current : target;
    if (parent) moveOverlayLayer(overlayLayerNode(), parent);
  }), []);
  return (
    <>
      <div class="overlay-layer-home" data-overlay-layer-home="" ref={homeRef} />
      {createPortal(<>{children}</>, overlayLayerNode())}
    </>
  );
}
