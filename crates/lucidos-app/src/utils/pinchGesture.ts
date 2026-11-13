// Pure math for two-finger pinch-zoom + pan. Each frame recomputes from
// the snapshot taken at gesture start (PinchInitial), not from the previous
// frame, so there is no drift and pure two-finger pan (no scale change) still
// updates tx/ty. Anchor invariant: the image-space point under the fingers'
// initial midpoint stays under the fingers' current midpoint.

export interface PinchInitial {
  scale: number;
  tx: number;
  ty: number;
  midX: number;
  midY: number;
  dist: number;
  natCenterX: number;
  natCenterY: number;
}

export interface PinchUpdate {
  scale: number;
  tx: number;
  ty: number;
}

export function fingerDistance(ax: number, ay: number, bx: number, by: number): number {
  return Math.hypot(ax - bx, ay - by);
}

export function fingerMidpoint(
  ax: number,
  ay: number,
  bx: number,
  by: number,
): { x: number; y: number } {
  return { x: (ax + bx) / 2, y: (ay + by) / 2 };
}

// Returns initial state unchanged when initial.dist is 0 (fingers were
// already touching at gesture start) — the scale ratio would divide by zero.
export function computePinchUpdate(
  initial: PinchInitial,
  curMidX: number,
  curMidY: number,
  curDist: number,
  minScale: number,
  maxScale: number,
): PinchUpdate {
  if (initial.dist <= 0) {
    return { scale: initial.scale, tx: initial.tx, ty: initial.ty };
  }
  const rawScale = initial.scale * (curDist / initial.dist);
  const newScale = Math.max(minScale, Math.min(maxScale, rawScale));
  const ratio = newScale / initial.scale;
  const newTx =
    ratio * initial.tx + (curMidX - initial.natCenterX) - ratio * (initial.midX - initial.natCenterX);
  const newTy =
    ratio * initial.ty + (curMidY - initial.natCenterY) - ratio * (initial.midY - initial.natCenterY);
  return { scale: newScale, tx: newTx, ty: newTy };
}

// Pure function — caller is responsible for caching layout dimensions to
// avoid forced sync layout in the gesture hot path.
export function clampPanTransform(
  scale: number,
  tx: number,
  ty: number,
  containerW: number,
  containerH: number,
  imgW: number,
  imgH: number,
): { tx: number; ty: number } {
  if (scale <= 1) return { tx: 0, ty: 0 };
  const overflowX = Math.max(0, (imgW * scale - containerW) / 2);
  const overflowY = Math.max(0, (imgH * scale - containerH) / 2);
  return {
    tx: Math.max(-overflowX, Math.min(overflowX, tx)),
    ty: Math.max(-overflowY, Math.min(overflowY, ty)),
  };
}

export interface ImageLayout {
  containerW: number;
  containerH: number;
  imgW: number;
  imgH: number;
  natCenterX: number;
  natCenterY: number;
}

// Recover the *natural* (scale-1, untranslated) image geometry the pinch math
// and clampPanTransform expect, from a bounding rect that already reflects the
// live transform. getBoundingClientRect() includes CSS transforms, so when a
// gesture starts while the image is already zoomed/panned, imgRect is scaled by
// `scale` and its center shifted by (tx, ty). With transform-origin: center the
// inversion is exact — divide the size by the scale and subtract the pan from
// the center. Without this, a second pinch (or a one-finger pan after zooming)
// would clamp against an inflated band (panning the image off its own edges)
// and anchor against a shifted center (drifting sideways on unzoom).
export function naturalImageLayout(
  container: { width: number; height: number },
  imgRect: { left: number; top: number; width: number; height: number },
  scale: number,
  tx: number,
  ty: number,
): ImageLayout {
  const s = scale || 1;
  return {
    containerW: container.width,
    containerH: container.height,
    imgW: imgRect.width / s,
    imgH: imgRect.height / s,
    natCenterX: imgRect.left + imgRect.width / 2 - tx,
    natCenterY: imgRect.top + imgRect.height / 2 - ty,
  };
}

// One press of a zoom button multiplies (or divides) the scale by this.
export const ZOOM_STEP = 1.5;

// Where one press of the zoom-in / zoom-out control lands, within the range.
export function steppedScale(
  scale: number,
  direction: 1 | -1,
  minScale: number,
  maxScale: number,
): number {
  const next = direction > 0 ? scale * ZOOM_STEP : scale / ZOOM_STEP;
  return Math.max(minScale, Math.min(maxScale, next));
}

// The scale at which one image pixel covers one PHYSICAL screen pixel: full
// size, what the readout calls 100%. `fittedWidth` is the CSS width the image
// occupies at scale 1. `pixelRatio` is the device pixels the screen draws per
// CSS pixel.
//
// Counting CSS pixels instead reads a phone screenshot as 33% on the phone it
// came from. The screen is drawing it as sharply as it can, and the "actual
// size" offered from there is a threefold blow-up of the same pixels.
//
// Returns 0 when there is nothing to measure against, which is an image with no
// intrinsic width (an SVG that declares none). The caller then has no full-size
// target to offer and needs a fallback.
export function fullSizeScale(
  fittedWidth: number,
  naturalWidth: number,
  pixelRatio: number,
): number {
  if (fittedWidth <= 0 || naturalWidth <= 0 || pixelRatio <= 0) return 0;
  return naturalWidth / (fittedWidth * pixelRatio);
}

// The zoom ceiling: the fixed cap, raised whenever full size sits above it. A
// tall screenshot fits the window at a small fraction of its own size. A
// constant cap would put 1:1 out of reach for the images that need it most.
export function zoomCeiling(cap: number, fullSize: number): number {
  return fullSize > cap ? fullSize : cap;
}

// The zoom floor, and the mirror of the ceiling above. An image with fewer
// pixels than the window fits by being blown up, so its own pixels sit BELOW
// the fitted view. A floor at the fit would hide 1:1 from exactly those images.
// A fixed floor of 1 hides it on any screen drawing several device pixels per
// CSS pixel.
export function zoomFloor(fit: number, fullSize: number): number {
  return fullSize > 0 && fullSize < fit ? fullSize : fit;
}

// The scale that makes the image touch the window edge: what a viewer calls
// fit. `imgW` and `imgH` are the size at scale 1. CSS already shrinks an
// oversized image to the container, so that one answers 1. A small one answers
// the factor that grows it out to the edge.
export function fitToWindowScale(
  containerW: number,
  containerH: number,
  imgW: number,
  imgH: number,
): number {
  if (containerW <= 0 || containerH <= 0 || imgW <= 0 || imgH <= 0) return 1;
  const fit = Math.min(containerW / imgW, containerH / imgH);
  // Sub-pixel rounding in the measured box makes an image that already fits ask
  // for a hair of upscale. Leave it alone: a transform of 1.0001 buys nothing
  // and costs the crispness of an unscaled layer.
  return sameScale(fit, 1) ? 1 : fit;
}

// What the level control reads out: the size on screen as a percentage of the
// image's own pixels, so 100% is one image pixel per physical screen pixel.
// `fullSize` is the scale where the two meet. It is 0 for an image that
// declares no intrinsic size, and the fitted view stands in for 1:1 there.
export function zoomPercent(scale: number, fullSize: number, fit: number): number {
  const oneToOne = fullSize > 0 ? fullSize : fit;
  if (oneToOne <= 0) return 100;
  return Math.round((scale / oneToOne) * 100);
}

// Two scales the same, within a relative tolerance. A fit computed from
// measured pixels never lands on exact equality, and an absolute epsilon means
// something different at 1x and at 45x.
export function sameScale(a: number, b: number): boolean {
  return Math.abs(a - b) <= Math.max(a, b) * 0.001;
}

// Single-anchor zoom (wheel, double-tap). The screen point at (anchorX,
// anchorY) stays fixed across the scale change.
export function computeZoomAt(
  prevScale: number,
  prevTx: number,
  prevTy: number,
  anchorX: number,
  anchorY: number,
  newScale: number,
  natCenterX: number,
  natCenterY: number,
  minScale: number,
  maxScale: number,
): PinchUpdate {
  const clamped = Math.max(minScale, Math.min(maxScale, newScale));
  if (prevScale <= 0) return { scale: clamped, tx: prevTx, ty: prevTy };
  const ratio = clamped / prevScale;
  const tx = prevTx + (1 - ratio) * (anchorX - natCenterX - prevTx);
  const ty = prevTy + (1 - ratio) * (anchorY - natCenterY - prevTy);
  return { scale: clamped, tx, ty };
}
