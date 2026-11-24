import { APP_PULL_MESSAGE_TYPE, APP_REFRESH_MESSAGE_TYPE } from '@lucidos/pull-to-refresh';
import { appFrameFor, isCanvasAppFrame } from '../../utils/appFrame';
import { runPanelRefresh, showPullTravel } from '../panelRefresh';

export function handleAppPullMessage(event: MessageEvent): void {
  const data = event.data as { type?: unknown; travel?: unknown } | null;
  if (!data || typeof data !== 'object') return;
  if (data.type !== APP_PULL_MESSAGE_TYPE && data.type !== APP_REFRESH_MESSAGE_TYPE) return;
  if (!isCanvasAppFrame(appFrameFor(event.source))) return;
  if (data.type === APP_REFRESH_MESSAGE_TYPE) {
    void runPanelRefresh();
    return;
  }
  const travel = typeof data.travel === 'number' && Number.isFinite(data.travel) ? data.travel : 0;
  showPullTravel(Math.max(0, travel));
}
