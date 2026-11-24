
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

export const ZOOM_STEP = 1.5;

export function steppedScale(
  scale: number,
  direction: 1 | -1,
  minScale: number,
  maxScale: number,
): number {
  const next = direction > 0 ? scale * ZOOM_STEP : scale / ZOOM_STEP;
  return Math.max(minScale, Math.min(maxScale, next));
}

export function fullSizeScale(
  fittedWidth: number,
  naturalWidth: number,
  pixelRatio: number,
): number {
  if (fittedWidth <= 0 || naturalWidth <= 0 || pixelRatio <= 0) return 0;
  return naturalWidth / (fittedWidth * pixelRatio);
}

export function zoomCeiling(cap: number, fullSize: number): number {
  return fullSize > cap ? fullSize : cap;
}

export function zoomFloor(fit: number, fullSize: number): number {
  return fullSize > 0 && fullSize < fit ? fullSize : fit;
}

export function fitToWindowScale(
  containerW: number,
  containerH: number,
  imgW: number,
  imgH: number,
): number {
  if (containerW <= 0 || containerH <= 0 || imgW <= 0 || imgH <= 0) return 1;
  const fit = Math.min(containerW / imgW, containerH / imgH);
  return sameScale(fit, 1) ? 1 : fit;
}

export function zoomPercent(scale: number, fullSize: number, fit: number): number {
  const oneToOne = fullSize > 0 ? fullSize : fit;
  if (oneToOne <= 0) return 100;
  return Math.round((scale / oneToOne) * 100);
}

export function sameScale(a: number, b: number): boolean {
  return Math.abs(a - b) <= Math.max(a, b) * 0.001;
}

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
