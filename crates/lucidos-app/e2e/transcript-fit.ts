export const PANE_ROUNDING_PX = 1;

export const overflowsPane = (scrollHeight: number, clientHeight: number): boolean =>
  scrollHeight - clientHeight > PANE_ROUNDING_PX;
