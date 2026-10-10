/** Where a scroll position is saved, and how to drop one.
 *
 *  The data layer resets a view's saved scroll after a save, so these live
 *  apart from `hooks/useScrollMemory.ts`, which restores it. Kept together,
 *  the store pulled the whole restore machinery into the entry chunk
 *  (ADR 0288). */

/** localStorage key for a chat thread's saved scroll offset. */
export function threadScrollKey(threadId: string): string {
  return `lucidos-scroll-thread-${threadId}`;
}

/** localStorage key for ContentPane's per-view scroll offset. ContentPane
 *  reads/writes this directly; the prefix lives here so resetContentScroll
 *  stays in sync. */
export function contentScrollKey(viewKey: string): string {
  return `lucidos-scroll-content-${viewKey}`;
}

/** Drop the saved scroll under `key`, so the next attach restores nothing. */
export function forgetSavedScroll(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch { /* quota or disabled, ignore */ }
}

/** Drop a ContentPane view's saved scroll so the next mount lands at the top
 *  instead of restoring (e.g., after saving a form). */
export function resetContentScroll(viewKey: string): void {
  forgetSavedScroll(contentScrollKey(viewKey));
}
