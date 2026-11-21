import type { WidgetSize } from '../generated/thread-event-wire';

/** The dashboard's grid, in rem so UI scale applies. The dashboard computes its
 *  columns from these, and the desktop rail's floor is two of its cells
 *  (`store/paneMinimums.ts`). A thread card's *widget box* takes the same cell
 *  (`widgetCellCss`), so a card and a tile of one size are one box. */

/** A cell's edge. It holds at every dashboard width, and only a dashboard too narrow
 *  for two cells draws them smaller (`widgetCellCss`). */
export const DASHBOARD_CELL_REM = 10;
/** The space between cells. */
export const DASHBOARD_GAP_REM = 0.75;
/** Grid steps per cell edge. A step is half a cell less half a gap, so a small
 *  tile spans two and an *app shortcut* (a quarter tile) spans one. */
export const DASHBOARD_STEPS_PER_CELL = 2;
/** A dashboard never has fewer columns, so a medium or large tile always fits. */
export const DASHBOARD_MIN_COLUMNS = 2;
/** The dashboard's own side padding: `.dashboard`'s `--space-lg`. */
export const DASHBOARD_PAD_X_REM = 1;

/** The cells a widget box spans, across and down, per widget size (ADR 0432). */
export const WIDGET_SIZE_CELLS: Record<WidgetSize, { columns: number; rows: number }> = {
  small: { columns: 1, rows: 1 },
  medium: { columns: 2, rows: 1 },
  large: { columns: 2, rows: 2 },
  xl: { columns: 4, rows: 2 },
};

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

/** The cell edge, as CSS, for a dashboard whose `clientWidth` is `hostPx`: the
 *  full cell, or half the content width less a gap when two cells do not fit.
 *  That equals the edge of the column count above at every width. A thread
 *  pane passes its own width, which is the phone dashboard's. The rem terms
 *  keep a UI scale change live between two measures. */
export function widgetCellCss(hostPx: number): string {
  const pad = 2 * DASHBOARD_PAD_X_REM;
  return `min(${DASHBOARD_CELL_REM}rem, (${hostPx}px - ${pad}rem - ${DASHBOARD_GAP_REM}rem) / 2)`;
}

/** `widgetCellCss` in px, for a root font size of `remPx`: the cell a fit check
 *  draws at a UI scale other than the window's own. */
export function widgetCellPx(hostPx: number, remPx: number): number {
  const pad = 2 * DASHBOARD_PAD_X_REM * remPx;
  return Math.min(DASHBOARD_CELL_REM * remPx, (hostPx - pad - DASHBOARD_GAP_REM * remPx) / 2);
}

/** The space between cells, as CSS. */
export const WIDGET_GAP_CSS = `${DASHBOARD_GAP_REM}rem`;

/** A widget box's width and height for `size`, as CSS over the cell edge and
 *  gap `useWidgetCell` sets on an ancestor. */
export function widgetBoxCss(size: WidgetSize): { width: string; height: string } {
  const { columns, rows } = WIDGET_SIZE_CELLS[size];
  const extent = (cells: number) => `calc(${cells} * var(--widget-cell) + ${cells - 1} * var(--widget-gap))`;
  return { width: extent(columns), height: extent(rows) };
}
