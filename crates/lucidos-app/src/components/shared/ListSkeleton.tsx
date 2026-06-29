import { useLayoutEffect, useRef, useState } from 'preact/hooks';

/** Pixel stride of one skeleton row: `.list-skeleton-row` height (2.75rem = 44px)
 *  + `.list-skeleton` gap (0.5rem = 8px). Used in fill mode to size the row run
 *  to the available pane height. */
const ROW_STRIDE_PX = 52;
/** Rows rendered before the first measurement, and when the pane can't be
 *  measured (tests / detached DOM) — a credible content-sized list. */
const FILL_FALLBACK_ROWS = 8;

/** Generic shimmer placeholder for a loading list of known-shape rows — the
 *  default loading visual for list content (apps, triggers, notifications,
 *  files, …) instead of a centered spinner. Renders `rows` placeholder bars with
 *  the shared `shimmer` keyframes (drawer.css). Gate it behind `useDelayedLoading`
 *  (300ms delay, so fast loads never flash it) and wrap it + the content in
 *  `<LoadingFade>` so it crossfades out instead of hard-swapping. Static and
 *  data-free; decorative → aria-hidden so it isn't announced. Reduced-motion
 *  disables the shimmer (see styles/components.css).
 *
 *  `fill` is for FULL-PANE list views (Apps, Triggers, Files, …): the skeleton
 *  sizes itself to the available content-pane height so it doesn't stop partway
 *  and leave a void below, and gets the same horizontal inset as real `.list-row`s
 *  so its bars don't run edge-to-edge. Leave it off for section-level lists
 *  (settings subsections), where the section already provides padding and a short
 *  content-sized skeleton is correct. */
export function ListSkeleton({ rows = 5, fill = false }: { rows?: number; fill?: boolean }) {
  if (fill) return <FillListSkeleton />;
  return (
    <div class="list-skeleton" aria-hidden="true">
      {Array.from({ length: rows }, (_, i) => (
        <div class="list-skeleton-row" key={i} />
      ))}
    </div>
  );
}

/** Full-pane variant: measures the distance from the skeleton's top to the
 *  bottom of its scrolling `.content-pane-body` ancestor (so any toolbar above
 *  it is excluded) and pins the skeleton to exactly that height, rendering enough
 *  rows to fill it (the last partial row is clipped by `overflow: hidden`). Tracks
 *  pane resizes (window resize, mobile keyboard) via a ResizeObserver. Falls back
 *  to a content-sized run when the pane can't be measured. */
function FillListSkeleton() {
  const ref = useRef<HTMLDivElement>(null);
  const [heightPx, setHeightPx] = useState<number | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    const pane = el?.closest('.content-pane-body');
    if (!el || !pane) return; // unmeasurable (tests / detached) → content-sized fallback
    const measure = () => {
      const available = pane.getBoundingClientRect().bottom - el.getBoundingClientRect().top;
      if (available > 0) setHeightPx(available);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(pane);
    return () => ro.disconnect();
  }, []);
  // +1 so the run always reaches past the pinned bottom edge; the overflow clip
  // trims the last partial row.
  const rows = heightPx == null ? FILL_FALLBACK_ROWS : Math.ceil(heightPx / ROW_STRIDE_PX) + 1;
  return (
    <div
      ref={ref}
      class="list-skeleton list-skeleton--fill"
      style={heightPx == null ? undefined : { height: `${heightPx}px` }}
      aria-hidden="true"
    >
      {Array.from({ length: rows }, (_, i) => (
        <div class="list-skeleton-row" key={i} />
      ))}
    </div>
  );
}
