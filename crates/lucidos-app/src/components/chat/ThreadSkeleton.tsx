export const THREAD_SKELETON_EXCHANGES = 8;

const RESPONSE_LINE_COUNTS = [6, 9, 5, 8, 7, 10];

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
