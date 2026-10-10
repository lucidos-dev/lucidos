/**
 * Viewport clamping maths, shared by the host shell and by app iframes.
 *
 * It lives here because `tooltip.ts` needs it and runs in both hosts. The host
 * reaches it through `@lucidos/geometry`, re-exported from `utils/dom.ts` so
 * its own callers are unchanged.
 *
 * Pure apart from `window.innerWidth`, and deliberately NOT re-exported from
 * `index.ts`: nothing here belongs on `window.lucidos`.
 */

/** Clamp a `start` coordinate so a `size`-long element fits inside the
 *  `[min, max]` range, leaving `margin` px of breathing room at either end.
 *
 *  Axis-neutral on purpose: the arithmetic is identical for `left`/`width` and
 *  `top`/`height`, and an anchored popover needs both.
 *
 *  When the element is LONGER than the range it cannot fit, and the `Math.max`
 *  wins. It pins to the leading edge, so the start stays on screen and the
 *  element overflows the far end. The alternative pushes the start off screen,
 *  where the content is unreachable. */
export function clampWithin(start: number, size: number, min: number, max: number, margin = 8): number {
  return Math.max(min + margin, Math.min(start, max - size - margin));
}

/** Clamp a horizontal `left` coordinate so a `width`-wide element stays inside the viewport,
 *  leaving `margin` px of breathing room on either edge. */
export function clampToViewportX(left: number, width: number, margin = 8): number {
  return clampWithin(left, width, 0, window.innerWidth, margin);
}
