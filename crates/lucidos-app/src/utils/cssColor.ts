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
