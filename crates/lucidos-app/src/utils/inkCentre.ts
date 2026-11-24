export const MEASURE_PX = 100;

const shifts = new Map<string, number>();

export function inkFont(style: Pick<CSSStyleDeclaration, 'fontStyle' | 'fontWeight' | 'fontFamily'>): string {
  const italic = style.fontStyle === 'italic' ? 'italic ' : '';
  return `${italic}${style.fontWeight} ${MEASURE_PX}px ${style.fontFamily}`;
}

export function drawnInkShift({ text, font, px }: { text: string; font: string; px: number }): number | null {
  const sizing = new OffscreenCanvas(1, 1).getContext('2d');
  if (!sizing) return null;
  sizing.font = font;
  if (!sizing.font.includes(`${px}px`)) return null;
  const advance = sizing.measureText(text).width;
  const pad = px;
  const width = Math.ceil(advance) + 2 * pad;
  const height = 2 * px;
  const ctx = new OffscreenCanvas(width, height).getContext('2d', { willReadFrequently: true });
  if (!ctx) return null;
  ctx.font = font;
  ctx.fillText(text, pad, px * 1.4);
  const rgba = ctx.getImageData(0, 0, width, height).data;
  let first = width;
  let last = -1;
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      if (rgba[(y * width + x) * 4 + 3] > 127) {
        first = Math.min(first, x);
        last = Math.max(last, x);
      }
    }
  }
  if (last < 0) return null;
  const inkMiddle = (first + last + 1) / 2 - pad;
  return (advance / 2 - inkMiddle) / px;
}

export function measureInkShift(text: string, style: Pick<CSSStyleDeclaration,
  'fontStyle' | 'fontWeight' | 'fontFamily'>): number {
  if (!text.trim() || typeof OffscreenCanvas === 'undefined') return 0;
  const font = inkFont(style);
  const key = `${font}\n${text}`;
  const cached = shifts.get(key);
  if (cached !== undefined) return cached;
  const shift = drawnInkShift({ text, font, px: MEASURE_PX }) ?? 0;
  if (document.fonts?.check(font, text) ?? true) shifts.set(key, shift);
  return shift;
}
