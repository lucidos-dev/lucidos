import { APP_PULL_MESSAGE_TYPE, APP_REFRESH_MESSAGE_TYPE } from '@lucidos/pull-to-refresh';
import { isKnownAppFrame } from '../../utils/appFrame';
import { runPanelRefresh, showPullTravel } from '../panelRefresh';

/** A pull inside an app frame, which the SDK posts up because the frame
 *  captures every touch it covers (`packages/lucidos-sdk/src/pullToRefresh.ts`).
 *  The host draws the affordance and runs the app panel's refresh, the same one
 *  the header's Refresh runs. A message from any other frame is ignored, so a
 *  nested embed cannot reload the app. */
export function handleAppPullMessage(event: MessageEvent): void {
  const data = event.data as { type?: unknown; travel?: unknown } | null;
  if (!data || typeof data !== 'object') return;
  if (data.type !== APP_PULL_MESSAGE_TYPE && data.type !== APP_REFRESH_MESSAGE_TYPE) return;
  if (!isKnownAppFrame(event.source)) return;
  if (data.type === APP_REFRESH_MESSAGE_TYPE) {
    void runPanelRefresh();
    return;
  }
  const travel = typeof data.travel === 'number' && Number.isFinite(data.travel) ? data.travel : 0;
  showPullTravel(Math.max(0, travel));
}
