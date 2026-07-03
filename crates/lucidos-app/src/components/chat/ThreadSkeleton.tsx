/** Number of placeholder exchanges rendered. Deliberately generous: the
 *  skeleton lives in `.thread-skeleton-overlay`, which is `position:absolute;
 *  inset:0; overflow:hidden` over the flex:1 `.thread-content-wrap` — a
 *  guaranteed-definite height that CLIPS the excess. So this only needs to
 *  exceed the tallest realistic content pane; extra exchanges are clipped (free)
 *  and the skeleton always fills the full height instead of stopping partway and
 *  leaving a void below. Each exchange is now tall (a long response block), so
 *  fewer exchanges already overfill any pane. */
export const THREAD_SKELETON_EXCHANGES = 8;

/** Shimmer lines drawn for each exchange's response, cycled across exchanges.
 *  Deliberately long and varied: an LLM answer runs many lines, so a credible
 *  thread placeholder is dominated by tall response blocks under short user
 *  bubbles — not the flat 3-line stub it used to draw. The last line of each
 *  block renders short for a ragged paragraph end. */
const RESPONSE_LINE_COUNTS = [6, 9, 5, 8, 7, 10];

/** Placeholder shown in the thread content area while a thread's events load —
 *  an immediate, consistent "opening" affordance instead of a blank gap or a
 *  spinner that flashes on slow loads. The real exchanges fade in over it once
 *  they arrive (see ThreadView). Static and data-free; the shimmer reuses the
 *  global `shimmer` keyframes and is disabled under prefers-reduced-motion (see
 *  styles/chat/response.css). Decorative → aria-hidden so it isn't announced. */
export function ThreadSkeleton() {
  return (
    <div class="thread-skeleton" aria-hidden="true">
      {Array.from({ length: THREAD_SKELETON_EXCHANGES }, (_, i) => {
        const lines = RESPONSE_LINE_COUNTS[i % RESPONSE_LINE_COUNTS.length];
        return (
          <div class="thread-skeleton-exchange" key={i}>
            <div class="thread-skeleton-block thread-skeleton-user" />
            <div class="thread-skeleton-lines">
              {Array.from({ length: lines }, (_, j) => (
                <div
                  class={`thread-skeleton-block thread-skeleton-line${j === lines - 1 ? ' thread-skeleton-line-short' : ''}`}
                  key={j}
                />
              ))}
            </div>
          </div>
        );
      })}
    </div>
  );
}
