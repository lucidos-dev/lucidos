import { createContext, cloneElement, type ComponentChildren, type VNode } from 'preact';
import { useContext, useLayoutEffect, useRef, useState } from 'preact/hooks';

/** Rows rendered before the first fill measurement, and when the pane can't be
 *  measured (tests / detached DOM) — a credible content-sized list. */
const FILL_FALLBACK_ROWS = 8;

const SkeletonContext = createContext(false);

/** True when rendering inside a {@link SkeletonProvider} — i.e. the component is
 *  being drawn as a loading placeholder rather than with real data. Row / list /
 *  tree-node components read this to (a) let the Sk* leaf primitives render
 *  shimmer placeholders and (b) force "usually present" optional structure to
 *  appear in the skeleton (e.g. `(useSkeleton() || item.detail) && <SkText/>`). */
export function useSkeleton(): boolean {
  return useContext(SkeletonContext);
}

/** Marks its subtree as skeleton mode so the Sk* leaf primitives render shimmer
 *  placeholders. {@link ListSkeletonOf} wraps the fanned-out rows in this for you;
 *  use it directly only when self-skeletonizing a one-off subtree. */
export function SkeletonProvider({ children }: { children: ComponentChildren }) {
  return <SkeletonContext.Provider value={true}>{children}</SkeletonContext.Provider>;
}

/** Text → shimmer bar. In skeleton mode renders the SAME outer element (so the
 *  surface class still drives its box / margins / flex placement) wrapping a clean
 *  `.sk-bar` child of width `w`; otherwise renders `children` unchanged. `as`
 *  picks the outer tag for block-level text rows (default `span`). */
export function SkText({
  class: cls,
  w,
  as = 'span',
  children,
}: {
  class?: string;
  w?: string;
  as?: 'span' | 'div';
  children?: ComponentChildren;
}) {
  const Tag = as;
  if (useSkeleton()) {
    return (
      <Tag class={cls} aria-hidden="true">
        <span class="sk-bar" style={{ width: w ?? '60%' }} />
      </Tag>
    );
  }
  return <Tag class={cls}>{children}</Tag>;
}

/** Any element (button, chip, icon, status dot, avatar) → shimmer block of the
 *  given footprint. In skeleton mode renders a standalone sized `.sk-bar`
 *  (`round` for buttons/chips, `circle` for dots/avatars); otherwise renders
 *  `children` (the real element). Size the block to mirror what it stands in for. */
export function SkBlock({
  w,
  h,
  circle,
  round,
  children,
}: {
  w?: string;
  h?: string;
  circle?: boolean;
  round?: boolean;
  children?: ComponentChildren;
}) {
  if (useSkeleton()) {
    return (
      <span
        class={`sk-bar${circle ? ' sk-circle' : ''}${round ? ' sk-round' : ''}`}
        style={{ width: w, height: h }}
        aria-hidden="true"
      />
    );
  }
  return <>{children}</>;
}

/** Number of rows to render to fill `availablePx` given a measured per-row
 *  `stridePx` (row height incl. container gap). The `+1` overshoots the pinned
 *  bottom edge so the `.sk-fill` overflow clip trims the trailing partial row,
 *  leaving no void below. Extracted as a pure function so it's unit-testable
 *  without a DOM (the fill effect itself needs layout). */
export function computeFillCount(availablePx: number, stridePx: number): number {
  if (availablePx <= 0 || stridePx <= 0) return FILL_FALLBACK_ROWS;
  return Math.ceil(availablePx / stridePx) + 1;
}

/**
 * Renders `count` copies of a SELF-SKELETONIZING row inside a
 * {@link SkeletonProvider}, so the
 * placeholder is the same markup as the real row and mirrors its layout by
 * construction (no separate skeleton tree to drift).
 *
 * - `row` is a thunk returning the row VNode (it receives the row index, so a
 *   tree skeleton can vary node kind / indent per row); it reads
 *   {@link useSkeleton} internally and draws itself as a placeholder.
 * - `containerClass` should be the SAME class the real list uses to stack its
 *   rows, so the row spacing / gap mirrors the loaded list.
 * - `fill` sizes the run to the available `.content-pane-body` height, measuring
 *   the ACTUAL rendered row stride (not a hardcoded constant) so variable-height
 *   rows fill the pane exactly with no void below.
 *
 * Gate it behind `useDelayedLoading` (300ms) and wrap it + the content in
 * `<LoadingFade>` so it crossfades out. Decorative → `aria-hidden`.
 */
export function ListSkeletonOf({
  row,
  count = 5,
  fill = false,
  containerClass,
}: {
  row: (i: number) => VNode;
  count?: number;
  fill?: boolean;
  containerClass?: string;
}) {
  if (fill) return <FillSkeleton row={row} containerClass={containerClass} />;
  return (
    <SkeletonProvider>
      <div class={containerClass} aria-hidden="true">
        {Array.from({ length: count }, (_, i) => cloneElement(row(i), { key: i }))}
      </div>
    </SkeletonProvider>
  );
}

/** Full-pane variant: measures the distance from the run's top to the bottom of
 *  its scrolling `.content-pane-body` ancestor and pins the run to exactly that
 *  height, rendering enough rows to fill it (the trailing partial row is clipped
 *  by `.sk-fill`'s `overflow: hidden`). Row stride is derived from the actually
 *  rendered rows (`scrollHeight / rowCount`, which includes the container gap),
 *  so any row height fills correctly. Tracks pane resizes via a ResizeObserver;
 *  falls back to a content-sized run when the pane can't be measured. */
function FillSkeleton({ row, containerClass }: { row: (i: number) => VNode; containerClass?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [heightPx, setHeightPx] = useState<number | null>(null);
  const [count, setCount] = useState(FILL_FALLBACK_ROWS);
  useLayoutEffect(() => {
    const el = ref.current;
    const pane = el?.closest('.content-pane-body');
    if (!el || !pane) return; // unmeasurable (tests / detached) → content-sized fallback
    const measure = () => {
      const available = pane.getBoundingClientRect().bottom - el.getBoundingClientRect().top;
      if (available <= 0) return;
      const rendered = el.children.length;
      const stride = rendered > 0 ? el.scrollHeight / rendered : 0;
      setHeightPx(available);
      setCount(computeFillCount(available, stride));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(pane);
    return () => ro.disconnect();
  }, []);
  return (
    <SkeletonProvider>
      <div
        ref={ref}
        class={`${containerClass ?? ''} sk-fill`.trim()}
        style={heightPx == null ? undefined : { height: `${heightPx}px` }}
        aria-hidden="true"
      >
        {Array.from({ length: count }, (_, i) => cloneElement(row(i), { key: i }))}
      </div>
    </SkeletonProvider>
  );
}
