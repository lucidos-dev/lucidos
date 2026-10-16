/** The dashboard's grid, in rem so UI scale applies. The dashboard computes its
 *  columns from these, and the desktop rail's floor is two of its cells
 *  (`store/paneMinimums.ts`). */

/** A cell's edge. It holds at every dashboard width, and only a dashboard too narrow
 *  for two cells draws them smaller (`.dashboard-grid`). */
export const DASHBOARD_CELL_REM = 10;
/** The space between cells. */
export const DASHBOARD_GAP_REM = 0.75;
/** A dashboard never has fewer columns, so a medium or large tile always fits. */
export const DASHBOARD_MIN_COLUMNS = 2;
/** The dashboard's own side padding: `.dashboard`'s `--space-lg`. */
export const DASHBOARD_PAD_X_REM = 1;

/** The dashboard's column count for this content width: as many cells as fit,
 *  and never fewer than two. A narrower dashboard drops a column rather than
 *  growing its cells. */
export function dashboardColumns(contentPx: number, remPx: number): number {
  const cell = DASHBOARD_CELL_REM * remPx;
  const gap = DASHBOARD_GAP_REM * remPx;
  return Math.max(DASHBOARD_MIN_COLUMNS, Math.floor((contentPx + gap) / (cell + gap)));
}

/** The width inside the dashboard's side padding, from its `clientWidth`. */
export function dashboardContentWidth(clientWidthPx: number, remPx: number): number {
  return clientWidthPx - 2 * DASHBOARD_PAD_X_REM * remPx;
}
