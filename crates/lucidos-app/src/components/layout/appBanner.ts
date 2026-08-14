/**
 * What every app-shell banner shares: which of its two instances renders, and
 * the height it reserves for everything anchored below the header.
 *
 * A banner is a persistent bar between the header and the panes, for a condition
 * that stays true until something changes (backup is off; this workspace is
 * unreachable). Two of them exist, and both can be up at once, which is what
 * makes this a module rather than a pattern copied twice: the layout gate is the
 * same sentence in both, and the height reservation is a ResizeObserver whose
 * one subtlety (re-reading the root font size at measure time) is not worth
 * getting right in two places.
 *
 * The hook lives here beside the pure halves rather than in `src/hooks/`,
 * because it is not a general capability: it publishes the specific contract
 * `--app-header-bottom` consumes, and the property names it may be handed are
 * the ones declared in `styles/global/base.css` next to that anchor.
 */
import type { RefObject } from 'preact';
import { useEffect } from 'preact/hooks';
import { getRemPx } from '../../utils/dom';

/** Which layout this instance belongs to. Both are mounted (the mobile one
 *  inside the fixed `.app-header`, the desktop one in the shell's flow), and each
 *  renders only under its own viewport, per the dual-render rule in
 *  `.claude/rules/frontend-css.md`. Rendering both would put two bars on screen
 *  and, worse, race two ResizeObservers to publish one CSS var. */
export type BannerLayout = 'desktop' | 'mobile';

/** Half of every banner's render gate: is this the mounted layout's instance?
 *  The other half is the banner's own condition, ANDed by the caller, so each
 *  banner's reason to exist stays in its own file. */
export function bannerBelongsToLayout(layout: BannerLayout, mobileViewport: boolean): boolean {
  return layout === (mobileViewport ? 'mobile' : 'desktop');
}

/** The value for a banner's height property, or null to clear it. Published in
 *  rem (mirroring `updateTitleBarHeightVar` in `useHideOnScroll.ts`) so the
 *  reservation survives a UI-scale change. */
export function bannerHeightValue(px: number | null, remSize: number): string | null {
  if (px === null || px <= 0 || remSize <= 0) return null;
  return `${px / remSize}rem`;
}

/** Keep `cssVar` in step with the rendered bar, DESKTOP ONLY.
 *
 *  Mobile publishes nothing: its banner lives inside the header element that
 *  `useHideOnScroll` already observes, so `--mobile-header-height` (and through
 *  it the mobile `--app-header-bottom` and every pane's `::before` spacer) grows
 *  on its own.
 *
 *  Each banner owns a DIFFERENT property, and `--app-header-bottom` sums them.
 *  Sharing one would mean two observers writing one value: whichever bar
 *  measured last would win, and dismissing either would clear the reservation
 *  for both.
 *
 *  Clearing on teardown is what stops a stale reservation from surviving a
 *  dismiss, a reconnect, or a switch to mobile. */
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
    // getRemPx() is read at MEASURE time, not captured at mount. Changing the UI
    // scale rewrites --user-ui-scale, which IS the root font size (base.css
    // `html { font-size: var(--user-ui-scale, 100%) }`), so a captured value
    // goes stale exactly when the bar's pixel height changes: the observer would
    // then divide the new px by the old rem and reserve the wrong space,
    // misplacing the toast stack and drawer until the banner remounted. Same
    // reason refreshHeight() in useHideOnScroll.ts re-reads it every time.
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
