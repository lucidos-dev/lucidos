/** How much overflow (px) a container must have before anybody can scroll it. A
 *  hair from a border or a rounded line height does not count.
 *
 *  ONE definition, read by `transcriptScrolls` below. `threadWindow.fillAction`
 *  grows the window until it is true. `scrollState.isScrollable` asks the same
 *  question for the up chevron. So the two must mean the same thing by the
 *  number, and a matching literal in each is not that. */
export const SCROLLABLE_SLACK_PX = 10;

/** Can the reader scroll this transcript at all?
 *
 *  Its own module, importing nothing, so `scrollState` reads it without a cycle
 *  and without bringing `threadWindow` into the entry chunk. */
export function transcriptScrolls(view: { scrollHeight: number; clientHeight: number }): boolean {
  return view.scrollHeight > view.clientHeight + SCROLLABLE_SLACK_PX;
}
