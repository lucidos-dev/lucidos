/** Measured at a large size, so the em value carries no rounding of its own. */
export const MEASURE_PX = 100;

const shifts = new Map<string, number>();

/** The canvas font for an element's computed style, at the measuring size.
 *  The canvas shorthand takes no `oblique <angle>` and no stretch percentage,
 *  and it drops a whole value it cannot parse. */
export function inkFont(style: Pick<CSSStyleDeclaration, 'fontStyle' | 'fontWeight' | 'fontFamily'>): string {
  const italic = style.fontStyle === 'italic' ? 'italic ' : '';
  return `${italic}${style.fontWeight} ${MEASURE_PX}px ${style.fontFamily}`;
}

/** How far `text` must move, in em, for its INK to sit where its advance is
 *  centred, or null when it cannot be measured.
 *
 *  `text-align: center` centres a glyph's advance, and a font may draw the
 *  glyph off it. Fira Code's bold "3" sits 0.027em left of its slot. The ink
 *  is found by drawing the text and scanning its pixels, because WebKit's
 *  `TextMetrics` reports the advance box as the ink's bounds.
 *
 *  Self-contained, with no reference outside its body, so the browser test
 *  can run this very function in a page. */
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

/** `drawnInkShift` for `text` in an element's computed font, or zero where it
 *  cannot be measured, which leaves the advance centred.
 *
 *  Cached once the font has loaded, since a badge re-measures on every font
 *  load. Before that the canvas draws a fallback face. */
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
