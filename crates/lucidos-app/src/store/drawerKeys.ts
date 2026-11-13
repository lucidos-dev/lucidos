/** Where the thread drawer keeps its per-device state in `localStorage`. A
 *  module of its own, free of side effects, so an e2e spec can seed the same
 *  keys. The store itself reads `localStorage` on import. */

export const DRAWER_GROUPING_KEY = 'lucidos-drawer-grouping';
export const SELECTED_ONGOING_GROUP_KEY = 'lucidos-drawer-selected-ongoing-group';
export const THREAD_DRAWER_WIDTH_KEY = 'lucidos-thread-drawer-width';
