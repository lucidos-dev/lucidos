/** Number of placeholder exchanges rendered. Deliberately generous: the
 *  skeleton lives in `.thread-skeleton-overlay`, which is `position:absolute;
 *  inset:0; overflow:hidden` over the flex:1 `.thread-content-wrap` — a
 *  guaranteed-definite height that CLIPS the excess. So this only needs to
 *  exceed the tallest realistic content pane; extra exchanges are clipped (free)
 *  and the skeleton always fills the full height instead of stopping partway and
 *  leaving a void below. */
export const THREAD_SKELETON_EXCHANGES = 10;

/** Placeholder shown in the thread content area while a thread's events load —
 *  an immediate, consistent "opening" affordance instead of a blank gap or a
 *  spinner that flashes on slow loads. The real exchanges fade in over it once
 *  they arrive (see ThreadView). Static and data-free; the shimmer reuses the
 *  global `shimmer` keyframes and is disabled under prefers-reduced-motion (see
 *  styles/chat/response.css). Decorative → aria-hidden so it isn't announced. */
export function ThreadSkeleton() {
  return (
    <div class="thread-skeleton" aria-hidden="true">
      {Array.from({ length: THREAD_SKELETON_EXCHANGES }, (_, i) => (
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
