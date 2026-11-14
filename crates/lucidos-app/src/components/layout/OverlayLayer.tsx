import { effect } from '@preact/signals';
import { createPortal } from 'preact/compat';
import { useLayoutEffect, useRef } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { appFullscreenHost } from '../../store/appFullscreenHost';
import { clientRefreshing } from '../../hooks/sw-update';

/** Where the overlay layer sits this frame: `'home'` beside the app shell (the
 *  default), or the mount inside a natively fullscreen app panel.
 *
 *  A client refresh forces `'home'`, and that is a correctness requirement, not
 *  a nicety. `UiBlockingOverlay` inerts the app root's children, and the
 *  layer's own children except the toast container, so the "Refreshing…" toast
 *  stays dismissable. Inside the fullscreen panel the layer would sit under
 *  `.app-shell`, which the blocker inerts wholesale. A descendant cannot undo
 *  `inert`, unlike `pointer-events`.
 *
 *  Under NATIVE fullscreen that trade is "invisible" rather than "dismissable":
 *  back at the app root the toast is outside the only subtree the browser
 *  paints, and so is the blocker's own scrim, until the page reloads.
 *
 *  Pure, and exported for the unit test. */
export function overlayLayerTarget(
  host: HTMLElement | null,
  refreshing: boolean,
): HTMLElement | 'home' {
  if (refreshing) return 'home';
  return host ?? 'home';
}

let layerNode: HTMLElement | null = null;

/** The one element the overlay group renders into, for the page's lifetime.
 *  Created on first use so importing this module off a DOM touches nothing. */
export function overlayLayerNode(): HTMLElement {
  if (!layerNode) {
    layerNode = document.createElement('div');
    layerNode.className = 'overlay-layer';
  }
  return layerNode;
}

type TextField = HTMLInputElement | HTMLTextAreaElement;
type MovableParent = HTMLElement & { moveBefore?: (node: Node, child: Node | null) => void };

/** Puts `node` last in `parent` and keeps the focused field inside it focused.
 *  `moveBefore` keeps focus, caret and iframe state where the browser has it.
 *  `appendChild` drops focus to `<body>`, so the fallback restores focus and
 *  selection, which is the only state a reader would notice. */
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

/** The host's overlay group, kept together and moved as one.
 *
 *  The group always portals into `overlayLayerNode`. Native fullscreen MOVES
 *  that node into the mount inside the fullscreen panel, the only subtree the
 *  browser then paints, and back home again after. The portal container never
 *  changes, so no overlay remounts (ADR 0437).
 *
 *  The move is a signal effect, so it runs synchronously on the write.
 *  `AppUiInline` clears the host in its unmount cleanup, which runs before
 *  Preact removes the panel. So the node is home before its panel goes.
 *
 *  One stacking context keeps the overlays' relative order. The order itself
 *  comes from the band (`store/overlayStack.ts`) and toast urgency
 *  (`components/shared/toastUrgency.ts`).
 *
 *  `UiBlockingOverlay` stays outside the layer, since it inerts the app root's
 *  children. So does `DropZone`, which renders null. */
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
