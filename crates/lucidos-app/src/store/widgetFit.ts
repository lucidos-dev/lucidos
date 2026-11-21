import { signal } from '@preact/signals';
import type { WidgetSize } from '../generated/thread-event-wire';

/** Public selection handle only. A launcher sets this in its shell tab after
 * registration. No origin token or renderer credential enters this realm. */
export const FIT_RENDERER_SESSION_KEY = 'lucidos-fit-renderer-session';

export function isSelectedFitRenderer(sessionId: string | undefined, deviceId: string | undefined, logicalDevice: string | null): boolean {
  return !!sessionId && !!deviceId && deviceId === logicalDevice
    && sessionStorage.getItem(FIT_RENDERER_SESSION_KEY) === sessionId;
}

/** A *widget fit check* this window renders (`engine/widget_fit.rs`). */
export interface WidgetFitRequest {
  requestId: string;
  appId: string;
  /** Smallest first. */
  sizes: WidgetSize[];
  /** The UI scale percent the boxes render at. */
  scale: number;
  /** The device that uses that scale, for the frames' first paint. */
  scaleDevice?: string;
}

/** The checks waiting to render, oldest first. One renders at a time. */
export const widgetFitRequests = signal<WidgetFitRequest[]>([]);

export function requestWidgetFit(request: WidgetFitRequest): void {
  widgetFitRequests.value = [...widgetFitRequests.value, request];
}

/** Drop the check that just answered. */
export function finishWidgetFit(requestId: string): void {
  widgetFitRequests.value = widgetFitRequests.value.filter((r) => r.requestId !== requestId);
}
