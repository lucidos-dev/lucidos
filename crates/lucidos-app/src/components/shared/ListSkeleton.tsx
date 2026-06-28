/** Generic shimmer placeholder for a loading list of known-shape rows — the
 *  default loading visual for list content (apps, triggers, notifications,
 *  files, …) instead of a centered spinner. Renders `rows` placeholder bars with
 *  the shared `shimmer` keyframes (drawer.css). Gate it behind `useDelayedLoading`
 *  (300ms delay, so fast loads never flash it) and wrap it + the content in
 *  `<LoadingFade>` so it crossfades out instead of hard-swapping. Static and
 *  data-free; decorative → aria-hidden so it isn't announced. Reduced-motion
 *  disables the shimmer (see styles/components.css). */
export function ListSkeleton({ rows = 5 }: { rows?: number }) {
  return (
    <div class="list-skeleton" aria-hidden="true">
      {Array.from({ length: rows }, (_, i) => (
        <div class="list-skeleton-row" key={i} />
      ))}
    </div>
  );
}
