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
