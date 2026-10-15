/** The Home board's grid, in rem so UI scale applies. The board computes its
 *  columns from these, and the desktop rail's floor is two of its cells
 *  (`store/paneMinimums.ts`). */

/** A cell's edge. It holds at every board width, and only a board too narrow
 *  for two cells draws them smaller (`.home-board-grid`). */
export const BOARD_CELL_REM = 10;
/** The space between cells. */
export const BOARD_GAP_REM = 0.75;
/** A board never has fewer columns, so a medium or large tile always fits. */
export const BOARD_MIN_COLUMNS = 2;
/** The board's own side padding: `.home-board`'s `--space-lg`. */
export const BOARD_PAD_X_REM = 1;

/** The board's column count for this content width: as many cells as fit,
 *  and never fewer than two. A narrower board drops a column rather than
 *  growing its cells. */
export function boardColumns(contentPx: number, remPx: number): number {
  const cell = BOARD_CELL_REM * remPx;
  const gap = BOARD_GAP_REM * remPx;
  return Math.max(BOARD_MIN_COLUMNS, Math.floor((contentPx + gap) / (cell + gap)));
}

/** The width inside the board's side padding, from its `clientWidth`. */
export function boardContentWidth(clientWidthPx: number, remPx: number): number {
  return clientWidthPx - 2 * BOARD_PAD_X_REM * remPx;
}
