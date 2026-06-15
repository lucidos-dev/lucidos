import { setSplitRatio, toggleContentPaneRatio, toggleThreadPaneRatio } from './splitHelpers';

/** Double-clicking the thread-side header maximizes the thread pane (toggles
 *  the content pane collapsed); the content-side header does the reverse. */
export function resolveHeaderDblClick({ clickedThreadSide, ratio }: { clickedThreadSide: boolean; ratio: number }) {
  setSplitRatio(clickedThreadSide ? toggleContentPaneRatio(ratio) : toggleThreadPaneRatio(ratio));
}
