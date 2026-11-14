import type { JSX } from 'preact';
import { useLayoutEffect, useRef } from 'preact/hooks';
import { measureInkShift } from '../../utils/inkCentre';

/** States the shift for `badge`'s text in its current computed font. */
function centreInk(badge: HTMLElement | null): void {
  if (!badge) return;
  const shift = measureInkShift(badge.textContent ?? '', getComputedStyle(badge));
  if (shift) badge.style.setProperty('--badge-ink-shift', `${shift.toFixed(4)}em`);
  else badge.style.removeProperty('--badge-ink-shift');
}

/** A badge carrying a number, a sign or a word, with that text's INK centred
 *  in the pill rather than its advance (see `drawnInkShift`).
 *
 *  It measures its text in its own computed font, and states the result as
 *  `--badge-ink-shift`. `styles/badges.css` moves the `.badge-ink` span around
 *  the text by it. It measures again on every render, on a font load,
 *  and on a font switch. Every glyph badge renders through this, which
 *  `styles/__tests__/badge-glyph-centring.test.ts` holds. */
export function GlyphBadge({ children, ...props }: JSX.HTMLAttributes<HTMLSpanElement>) {
  const ref = useRef<HTMLSpanElement>(null);
  // Every render: the text, a class or a local font may have changed, and a
  // measurement is a cache hit once its font has loaded.
  useLayoutEffect(() => centreInk(ref.current));
  useLayoutEffect(() => {
    const centre = () => centreInk(ref.current);
    document.fonts?.addEventListener('loadingdone', centre);
    // A font switch lands as `--font-ui` on <html>, whose style also changes on
    // every divider drag. Only the font moves the ink.
    const root = document.documentElement;
    let font = root.style.getPropertyValue('--font-ui');
    const restyles = new MutationObserver(() => {
      const next = root.style.getPropertyValue('--font-ui');
      if (next === font) return;
      font = next;
      centre();
    });
    restyles.observe(root, { attributes: true, attributeFilter: ['style'] });
    return () => {
      document.fonts?.removeEventListener('loadingdone', centre);
      restyles.disconnect();
    };
  }, []);
  // Only text has ink to move. The brand badge's spinner and hourglass keep
  // the flex centring badges.css gives an icon.
  const text = typeof children === 'string' || typeof children === 'number';
  return (
    <span {...props} ref={ref}>
      {text ? <span class="badge-ink">{children}</span> : children}
    </span>
  );
}
