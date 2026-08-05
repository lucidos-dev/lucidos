import { createPortal } from 'preact/compat';
import type { ComponentChildren } from 'preact';
import { appFullscreenHost } from '../../store/appFullscreenHost';
import { clientRefreshing } from '../../hooks/sw-update';

/** Where the host's overlay group renders this frame: `'inline'` at the app
 *  root (the default), or the element to portal it into.
 *
 *  A client refresh forces `'inline'`, and that is a correctness requirement,
 *  not a nicety. `UiBlockingOverlay` inerts its own SIBLINGS and skips
 *  `.toast-container` by class so the "Refreshing…" toast stays dismissable.
 *  Portaled, the toast is no longer a sibling: it lives under `.app-shell`,
 *  which the blocker inerts wholesale, and `inert` (unlike `pointer-events`)
 *  cannot be overridden by a descendant. Standing the layer down for the
 *  duration puts the toast back beside the blocker, where that carve-out works.
 *  The two states are mutually exclusive by construction: the blocker only
 *  exists during a client refresh, which ends in a page reload.
 *
 *  Under NATIVE fullscreen that trade is "invisible" rather than "dismissable":
 *  back at the app root the toast is outside the only subtree the browser
 *  paints. That is the pre-existing behaviour and the blocker's own scrim is
 *  invisible there too, so the alternative on offer is not a working toast but
 *  a visible dead one, for the second or two before the page reloads.
 *
 *  Pure, and exported for the unit test: the frontend test environment has no
 *  DOM, so the decision is a value rather than a rendered tree. */
export function overlayLayerTarget(
  host: HTMLElement | null,
  refreshing: boolean,
): HTMLElement | 'inline' {
  if (refreshing) return 'inline';
  return host ?? 'inline';
}

/** The host's overlay group, kept together and moved as one.
 *
 *  With nothing fullscreen this renders a bare fragment, so the DOM is exactly
 *  what it was before this component existed: each overlay a direct child of the
 *  app root, ordered by the z-index scale (`--z-modal` under `--z-toast`, and
 *  both under the JS tooltip).
 *
 *  While an app is NATIVELY fullscreen the browser paints only that element's
 *  subtree, so the group is portaled into a mount inside it (see
 *  `appFullscreenHost`, which owns the mount and the `display: contents` +
 *  `data-overlay-layer` element it publishes). Moving the group as a UNIT is the
 *  point: the overlays keep their relative order because they keep sharing one
 *  stacking context, so a toast still covers a modal rather than the two
 *  swapping depending on which was raised first.
 *
 *  Two App-root overlays deliberately stay OUTSIDE this layer.
 *  `UiBlockingOverlay` inerts its own siblings (`overlay.parentElement`'s
 *  children), so inside the portal it would inert nine overlays and miss
 *  `.app-shell`, i.e. stop blocking the thing it exists to block. `DropZone`
 *  renders null and only installs document listeners, so moving it would cost a
 *  remount and buy nothing.
 *
 *  Changing the portal target remounts the subtree, which is why the target is
 *  the fullscreen mount and not, say, `document.body` on every frame: it flips
 *  only when fullscreen is entered or left. */
export function OverlayLayer({ children }: { children: ComponentChildren }) {
  const target = overlayLayerTarget(appFullscreenHost.value, clientRefreshing.value);
  if (target === 'inline') return <>{children}</>;
  return createPortal(<>{children}</>, target);
}
