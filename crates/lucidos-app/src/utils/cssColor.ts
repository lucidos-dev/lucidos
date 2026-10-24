/**
 * The hex colour a custom property paints on `<html>`, `var()` and
 * `color-mix()` resolved. For consumers outside CSS that take only a literal:
 * the Tauri window tint and `meta theme-color`.
 *
 * A theme may define a token as a `color-mix()` over other tokens. A computed
 * style serialises that as `color(srgb …)` or `oklab(…)`, depending on the
 * engine. Painting it into a one-pixel canvas and reading the pixel back is
 * the one conversion every engine agrees on.
 *
 * Returns null while nothing defines the property (no stylesheet yet), since
 * the probe would then read back the inherited text colour. Also null where
 * there is no canvas (a unit test). Either way the caller keeps its fallback.
 */
export function resolvedTokenHex(name: `--${string}`): string | null {
  if (typeof document === 'undefined' || !document.body) return null;
  if (getComputedStyle(document.documentElement).getPropertyValue(name).trim() === '') return null;
  const probe = document.createElement('span');
  probe.style.display = 'none';
  probe.style.color = `var(${name})`;
  document.body.appendChild(probe);
  const computed = getComputedStyle(probe).color;
  probe.remove();
  if (!computed) return null;

  const canvas = document.createElement('canvas');
  canvas.width = 1;
  canvas.height = 1;
  const ctx = canvas.getContext('2d', { willReadFrequently: true });
  if (!ctx) return null;
  ctx.fillStyle = computed;
  ctx.fillRect(0, 0, 1, 1);
  const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
  return `#${[r, g, b].map(c => c.toString(16).padStart(2, '0')).join('')}`;
}
