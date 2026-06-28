/** Placeholder shown in the thread content area while a thread's events load —
 *  an immediate, consistent "opening" affordance instead of a blank gap or a
 *  spinner that flashes on slow loads. The real exchanges fade in over it once
 *  they arrive (see ThreadView). Static and data-free; the shimmer reuses the
 *  global `shimmer` keyframes and is disabled under prefers-reduced-motion (see
 *  styles/chat/response.css). Decorative → aria-hidden so it isn't announced. */
export function ThreadSkeleton() {
  return (
    <div class="thread-skeleton" aria-hidden="true">
      {[0, 1, 2].map((i) => (
        <div class="thread-skeleton-exchange" key={i}>
          <div class="thread-skeleton-block thread-skeleton-user" />
          <div class="thread-skeleton-lines">
            <div class="thread-skeleton-block thread-skeleton-line" />
            <div class="thread-skeleton-block thread-skeleton-line" />
            <div class="thread-skeleton-block thread-skeleton-line thread-skeleton-line-short" />
          </div>
        </div>
      ))}
    </div>
  );
}
