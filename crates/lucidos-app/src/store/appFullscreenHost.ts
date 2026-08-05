import { signal } from '@preact/signals';

/** The app panel, and the empty element inside it the host's overlay layer
 *  renders into. Both are written by `AppUiInline`; everything here finds them
 *  by these markers rather than by holding a reference to them. */
export const APP_PANEL_SELECTOR = '[data-role="app-ui-panel"]';
export const OVERLAY_MOUNT_SELECTOR = '[data-overlay-layer]';

/** Where the host's overlay layer renders while an app is NATIVELY fullscreen:
 *  an empty mount element inside the fullscreen app panel, or null.
 *
 *  A natively fullscreen element is painted alone: the browser renders that
 *  element's subtree and nothing else, whatever its z-index. So the host's own
 *  overlays (the `previewFile` modal, confirm, prompt, toast, the search
 *  palette) cannot be seen over a fullscreen app from where they are mounted, at
 *  the app root. `OverlayLayer` portals the whole group into this element, which
 *  is the only place the browser will paint them.
 *
 *  The mount is a dedicated empty element rather than the panel itself because
 *  the panel's children are Preact's (the iframe, the pseudo-fullscreen chrome),
 *  and a portal filling a container another component also fills asks two diffs
 *  to agree about DOM they each think they own.
 *
 *  Pseudo-fullscreen (`.app-ui-fullscreen`, the CSS fallback) deliberately does
 *  NOT set this. That panel is painted in the normal layer, so it is a pure
 *  stacking question, and `--z-app-fullscreen` settles it: the panel sits below
 *  the modal layer and the overlays paint over it where they are. */
export const appFullscreenHost = signal<HTMLElement | null>(null);

/** The natively fullscreen element, across the standard and webkit-prefixed
 *  spellings. `null` off a DOM (unit tests, SSR). */
export function nativeFullscreenElement(): Element | null {
  if (typeof document === 'undefined') return null;
  const doc = document as unknown as { fullscreenElement?: Element | null; webkitFullscreenElement?: Element | null };
  return doc.fullscreenElement ?? doc.webkitFullscreenElement ?? null;
}

/** The one derivation: the overlay mount inside whatever is fullscreen right
 *  now, or null when there is nowhere for the host to paint.
 *
 *  Derived from the DOM on every call, deliberately, rather than from an element
 *  a component captured in a ref. A component's published element goes stale the
 *  moment its panel is replaced, and a stale host is indistinguishable from "no
 *  host": it refuses a preview whose modal would have been perfectly visible.
 *  That shipped as a mobile e2e flake where the DOM was in order (one panel, one
 *  mount, the panel fullscreen and containing the mount) and only the signal
 *  disagreed.
 *
 *  Null for the two cases that genuinely have nowhere to paint: nothing is
 *  fullscreen (the overlays render at the app root, as always), and something is
 *  fullscreen that is not an app panel of ours (an app that called
 *  `requestFullscreen` on its own content makes the IFRAME the fullscreen
 *  element, and an iframe renders no DOM children).
 *
 *  A DETACHED fullscreen element also resolves to null but does NOT block: see
 *  `fullscreenBlocksHostOverlays`. */
export function resolveOverlayMount(
  fullscreenEl: Element | null = nativeFullscreenElement(),
): HTMLElement | null {
  if (fullscreenEl === null || !fullscreenEl.isConnected) return null;
  if (!fullscreenEl.matches?.(APP_PANEL_SELECTOR)) return null;
  return fullscreenEl.querySelector<HTMLElement>(OVERLAY_MOUNT_SELECTOR);
}

/** Re-derive `appFullscreenHost` from the DOM. Called by `AppUiInline` on every
 *  render and on every fullscreen change, and by the app-message bridge right
 *  before it decides whether it can show a modal, so the decision and the portal
 *  that acts on it are reading the same instant. */
export function syncAppFullscreenHost(): void {
  const mount = resolveOverlayMount();
  if (appFullscreenHost.peek() !== mount) appFullscreenHost.value = mount;
}

/** What the host saw when it looked for somewhere to paint, for the refusal
 *  message an app receives. A refusal an author cannot act on is barely better
 *  than the silent success this whole change exists to remove, and the state
 *  that produced it is exactly what they need: which element holds fullscreen,
 *  and whether the host has a mount inside it. Never throws. */
export function describeOverlayTarget(
  fullscreenEl: Element | null = nativeFullscreenElement(),
): string {
  if (fullscreenEl === null) return 'nothing is fullscreen';
  const tag = fullscreenEl.tagName?.toLowerCase() ?? '?';
  const cls = typeof fullscreenEl.className === 'string' && fullscreenEl.className
    ? `.${fullscreenEl.className.trim().split(/\s+/).join('.')}`
    : '';
  const detached = fullscreenEl.isConnected ? '' : ', detached';
  const panels = typeof document !== 'undefined'
    ? document.querySelectorAll(APP_PANEL_SELECTOR).length
    : 0;
  const mounts = typeof document !== 'undefined'
    ? document.querySelectorAll(OVERLAY_MOUNT_SELECTOR).length
    : 0;
  return `fullscreen: ${tag}${cls}${detached}; app panels: ${panels}; overlay mounts: ${mounts}`;
}

/** Whether the host has nowhere to paint an overlay.
 *
 *  The `isConnected` arm is not defensive noise, it is a real window. A
 *  fullscreen element that has been detached (the app panel remounted under it)
 *  is still `document.fullscreenElement` until the browser gets around to
 *  exiting, and during those few frames the shell is on its way back to the
 *  normal layout anyway. Refusing there would reject a preview that is about to
 *  be perfectly visible.
 *
 *  Pure in its arguments (both default to a live read) so it is testable without
 *  a DOM. */
export function fullscreenBlocksHostOverlays(
  fullscreenEl: Element | null = nativeFullscreenElement(),
  mount: HTMLElement | null = resolveOverlayMount(fullscreenEl),
): boolean {
  if (fullscreenEl === null || !fullscreenEl.isConnected) return false;
  return mount === null;
}
