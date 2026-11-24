import { APP_SWIPE_END_MESSAGE_TYPE, APP_SWIPE_MESSAGE_TYPE } from '@lucidos/pane-swipe';
import { isKnownAppFrame } from '../../utils/appFrame';

export type FrameSwipeMessage =
  | { kind: 'drag'; dx: number }
  | { kind: 'release'; paneDelta: -1 | 0 | 1 }
  | { kind: 'gone' };

export type FrameSource = MessageEventSource | null;

export type FrameSwipeListener = (source: FrameSource, message: FrameSwipeMessage) => void;

export function parseAppSwipeMessage(event: MessageEvent): FrameSwipeMessage | null {
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

const hostListeners = new Set<FrameSwipeListener>();

export function forwardPreviewSwipe(source: Window | null, message: FrameSwipeMessage): void {
  for (const listener of hostListeners) listener(source, message);
}

export function announceFrameGone(source: Window | null): void {
  if (!source) return;
  for (const listener of hostListeners) listener(source, { kind: 'gone' });
}

export function subscribeFrameSwipes(listener: FrameSwipeListener): () => void {
  const onMessage = (e: MessageEvent) => {
    const message = parseAppSwipeMessage(e);
    if (message) listener(e.source, message);
  };
  window.addEventListener('message', onMessage);
  hostListeners.add(listener);
  return () => {
    window.removeEventListener('message', onMessage);
    hostListeners.delete(listener);
  };
}
