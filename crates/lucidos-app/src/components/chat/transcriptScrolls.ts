export const SCROLLABLE_SLACK_PX = 10;

export function transcriptScrolls(view: { scrollHeight: number; clientHeight: number }): boolean {
  return view.scrollHeight > view.clientHeight + SCROLLABLE_SLACK_PX;
}
