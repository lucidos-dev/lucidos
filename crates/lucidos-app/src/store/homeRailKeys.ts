/** Where the desktop Home rail keeps its per-device state in `localStorage`.
 *  A module of its own, free of side effects, so an e2e spec can seed the same
 *  keys. The store itself reads `localStorage` on import. */

export const HOME_RAIL_SHOWN_KEY = 'lucidos-home-rail-shown';
export const HOME_RAIL_WIDTH_KEY = 'lucidos-home-rail-width';
