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
