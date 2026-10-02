import { APP_SWIPE_END_MESSAGE_TYPE, APP_SWIPE_MESSAGE_TYPE } from '@lucidos/pane-swipe';
import { isKnownAppFrame } from '../../utils/appFrame';

export type AppSwipeMessage =
  | { kind: 'drag'; dx: number }
  | { kind: 'release'; paneDelta: -1 | 0 | 1 };

/** A sideways drag inside an app frame, which the SDK posts up because the
 *  frame captures every touch it covers (`packages/lucidos-sdk/src/paneSwipe.ts`).
 *  `MobileSwipeContainer` moves the panes on it as it does for its own touches.
 *
 *  Null for any other message, and for one from any other frame, so a nested
 *  embed cannot swipe the panes. A release whose delta no gesture could produce
 *  still ends the drag, as a snap back, so the track never sticks mid-drag. */
export function parseAppSwipeMessage(event: MessageEvent): AppSwipeMessage | null {
  const data = event.data as { type?: unknown; dx?: unknown; paneDelta?: unknown } | null;
  if (!data || typeof data !== 'object') return null;
  if (data.type !== APP_SWIPE_MESSAGE_TYPE && data.type !== APP_SWIPE_END_MESSAGE_TYPE) return null;
  if (!isKnownAppFrame(event.source)) return null;
  if (data.type === APP_SWIPE_MESSAGE_TYPE) {
    return typeof data.dx === 'number' && Number.isFinite(data.dx) ? { kind: 'drag', dx: data.dx } : null;
  }
  const paneDelta = data.paneDelta === 1 || data.paneDelta === -1 ? data.paneDelta : 0;
  return { kind: 'release', paneDelta };
}
