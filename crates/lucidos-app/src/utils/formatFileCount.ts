/** A change's file count, as the meta line renders it.
 *
 *  Zero is spelled out rather than shown as "0 files" because it is a state the
 *  user has to act on, not a quantity: the change's branch commits cancelled
 *  out, so its Diff is empty and Discard is the only meaningful resolution.
 *  Every surface that shows a change's file count uses this, so the Changes
 *  card, the Files-view change picker, and the in-thread change card can't
 *  drift from each other. */
export function formatFileCount(count: number): string {
  if (count === 0) return 'No file changes';
  return `${count} file${count === 1 ? '' : 's'}`;
}
