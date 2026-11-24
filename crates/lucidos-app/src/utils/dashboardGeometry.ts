import type { WidgetSize } from '../generated/thread-event-wire';


export const DASHBOARD_CELL_REM = 10;
export const DASHBOARD_GAP_REM = 0.75;
export const DASHBOARD_STEPS_PER_CELL = 2;
export const DASHBOARD_MIN_COLUMNS = 2;
export const DASHBOARD_PAD_X_REM = 1;

export const WIDGET_SIZE_CELLS: Record<WidgetSize, { columns: number; rows: number }> = {
  small: { columns: 1, rows: 1 },
  medium: { columns: 2, rows: 1 },
  large: { columns: 2, rows: 2 },
  xl: { columns: 4, rows: 2 },
};

export function dashboardColumns(contentPx: number, remPx: number): number {
  const cell = DASHBOARD_CELL_REM * remPx;
  const gap = DASHBOARD_GAP_REM * remPx;
  return Math.max(DASHBOARD_MIN_COLUMNS, Math.floor((contentPx + gap) / (cell + gap)));
}

export function dashboardContentWidth(clientWidthPx: number, remPx: number): number {
  return clientWidthPx - 2 * DASHBOARD_PAD_X_REM * remPx;
}

export function widgetCellCss(hostPx: number): string {
  const pad = 2 * DASHBOARD_PAD_X_REM;
  return `min(${DASHBOARD_CELL_REM}rem, (${hostPx}px - ${pad}rem - ${DASHBOARD_GAP_REM}rem) / 2)`;
}

export function widgetCellPx(hostPx: number, remPx: number): number {
  const pad = 2 * DASHBOARD_PAD_X_REM * remPx;
  return Math.min(DASHBOARD_CELL_REM * remPx, (hostPx - pad - DASHBOARD_GAP_REM * remPx) / 2);
}

export const WIDGET_GAP_CSS = `${DASHBOARD_GAP_REM}rem`;

export function widgetBoxCss(size: WidgetSize): { width: string; height: string } {
  const { columns, rows } = WIDGET_SIZE_CELLS[size];
  const extent = (cells: number) => `calc(${cells} * var(--widget-cell) + ${cells - 1} * var(--widget-gap))`;
  return { width: extent(columns), height: extent(rows) };
}
