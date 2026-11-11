import { signal } from '@preact/signals';
import type { WidgetSize } from '../generated/thread-event-wire';

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
