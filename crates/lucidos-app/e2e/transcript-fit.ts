/** `scrollHeight` is an integer and the rects beside it are fractional, so a
 *  gap of one pixel is rounding. Any more is content the pane cannot hold. */
export const PANE_ROUNDING_PX = 1;

/** Whether the transcript's content is taller than its pane. One definition
 *  for both branches of the transcript spec: a second, wider threshold left
 *  a band where a thread really overflowed yet was judged "short". */
export const overflowsPane = (scrollHeight: number, clientHeight: number): boolean =>
  scrollHeight - clientHeight > PANE_ROUNDING_PX;
