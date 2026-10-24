/** Where the desktop dashboard rail keeps its per-device state in `localStorage`.
 *  A module of its own, free of side effects, so an e2e spec can seed the same
 *  keys. The store itself reads `localStorage` on import. */

export const DASHBOARD_RAIL_SHOWN_KEY = 'lucidos-dashboard-rail-shown';
export const DASHBOARD_RAIL_WIDTH_KEY = 'lucidos-dashboard-rail-width';
