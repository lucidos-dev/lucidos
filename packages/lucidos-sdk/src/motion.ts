export function isReducedMotion(): boolean {
  return document.documentElement.getAttribute('data-motion') === 'reduce';
}

export function scaledDurationMs(baseMs: number): number {
  const scale = Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--duration-scale'));
  return baseMs * (isReducedMotion() ? 0.001 : Number.isFinite(scale) && scale > 0 ? scale : 1);
}
