
export function clampWithin(start: number, size: number, min: number, max: number, margin = 8): number {
  return Math.max(min + margin, Math.min(start, max - size - margin));
}

export function clampToViewportX(left: number, width: number, margin = 8): number {
  return clampWithin(left, width, 0, window.innerWidth, margin);
}
