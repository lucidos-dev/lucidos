import type { JSX } from 'preact';
import { useLayoutEffect, useRef } from 'preact/hooks';
import { measureInkShift } from '../../utils/inkCentre';

function centreInk(badge: HTMLElement | null): void {
  if (!badge) return;
  const shift = measureInkShift(badge.textContent ?? '', getComputedStyle(badge));
  if (shift) badge.style.setProperty('--badge-ink-shift', `${shift.toFixed(4)}em`);
  else badge.style.removeProperty('--badge-ink-shift');
}

export function GlyphBadge({ children, ...props }: JSX.HTMLAttributes<HTMLSpanElement>) {
  const ref = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => centreInk(ref.current));
  useLayoutEffect(() => {
    const centre = () => centreInk(ref.current);
    document.fonts?.addEventListener('loadingdone', centre);
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
  const text = typeof children === 'string' || typeof children === 'number';
  return (
    <span {...props} ref={ref}>
      {text ? <span class="badge-ink">{children}</span> : children}
    </span>
  );
}
