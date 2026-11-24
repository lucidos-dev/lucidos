import type { RefObject } from 'preact';
import { useEffect } from 'preact/hooks';
import { getRemPx } from '../../utils/dom';

export type BannerLayout = 'desktop' | 'mobile';

export function bannerBelongsToLayout(layout: BannerLayout, mobileViewport: boolean): boolean {
  return layout === (mobileViewport ? 'mobile' : 'desktop');
}

export function bannerHeightValue(px: number | null, remSize: number): string | null {
  if (px === null || px <= 0 || remSize <= 0) return null;
  return `${px / remSize}rem`;
}

export function useBannerHeightVar(
  ref: RefObject<HTMLDivElement>,
  opts: { layout: BannerLayout; cssVar: string; active: boolean },
): void {
  const { layout, cssVar, active } = opts;
  useEffect(() => {
    if (layout !== 'desktop' || !active) return;
    const el = ref.current;
    if (!el) return;
    const root = document.documentElement;
    const publish = (px: number | null) => {
      const value = bannerHeightValue(px, getRemPx());
      if (value === null) root.style.removeProperty(cssVar);
      else root.style.setProperty(cssVar, value);
    };
    publish(el.getBoundingClientRect().height);
    const observer = new ResizeObserver(() => publish(el.getBoundingClientRect().height));
    observer.observe(el, { box: 'border-box' });
    return () => {
      observer.disconnect();
      publish(null);
    };
  }, [ref, layout, cssVar, active]);
}
