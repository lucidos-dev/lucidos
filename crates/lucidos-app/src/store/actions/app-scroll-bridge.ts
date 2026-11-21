import { APP_SCROLL_MESSAGE_TYPE, APP_TOUCH_MESSAGE_TYPE } from '@lucidos/app-scroll';
import { appFrameFor, isCanvasAppFrame } from '../../utils/appFrame';

/** The reader's scroll inside the Canvas pane's app, which the SDK posts up
 *  because the frame's own document scrolls (ADR 0469). `source` tells one
 *  frame from the next, so a newly opened app starts afresh. */
export interface AppScrollMessage {
  source: MessageEventSource;
  delta: number;
  top: number;
  max: number;
}

/** Whether a finger is on the Canvas pane's app. The bars wait for it to lift,
 *  so the frame never moves under it. */
export interface AppTouchMessage {
  source: MessageEventSource;
  touching: boolean;
}

function finite(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

/** The message's data when it is of `type` and came from the Canvas pane's
 *  app, and null otherwise. A widget in a thread, or an embed inside the app,
 *  cannot move the host's bars. */
function canvasAppMessage(event: MessageEvent, type: string): Record<string, unknown> | null {
  const data = event.data as Record<string, unknown> | null;
  if (!data || typeof data !== 'object' || data.type !== type) return null;
  if (!event.source || !isCanvasAppFrame(appFrameFor(event.source))) return null;
  return data;
}

/** The Canvas app's scroll report, or null for anything else. */
export function parseAppScrollMessage(event: MessageEvent): AppScrollMessage | null {
  const data = canvasAppMessage(event, APP_SCROLL_MESSAGE_TYPE);
  if (!data || !finite(data.delta) || !finite(data.top) || !finite(data.max)) return null;
  return { source: event.source!, delta: data.delta, top: data.top, max: data.max };
}

/** The Canvas app's finger, or null for anything else. */
export function parseAppTouchMessage(event: MessageEvent): AppTouchMessage | null {
  const data = canvasAppMessage(event, APP_TOUCH_MESSAGE_TYPE);
  if (!data || typeof data.touching !== 'boolean') return null;
  return { source: event.source!, touching: data.touching };
}
