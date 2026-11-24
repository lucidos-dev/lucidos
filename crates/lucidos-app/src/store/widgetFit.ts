import { signal } from '@preact/signals';
import type { WidgetSize } from '../generated/thread-event-wire';

export const FIT_RENDERER_SESSION_KEY = 'lucidos-fit-renderer-session';

export function isSelectedFitRenderer(sessionId: string | undefined, deviceId: string | undefined, logicalDevice: string | null): boolean {
  return !!sessionId && !!deviceId && deviceId === logicalDevice
    && sessionStorage.getItem(FIT_RENDERER_SESSION_KEY) === sessionId;
}

export interface WidgetFitRequest {
  requestId: string;
  appId: string;
  sizes: WidgetSize[];
  scale: number;
  scaleDevice?: string;
}

export const widgetFitRequests = signal<WidgetFitRequest[]>([]);

export function requestWidgetFit(request: WidgetFitRequest): void {
  widgetFitRequests.value = [...widgetFitRequests.value, request];
}

export function finishWidgetFit(requestId: string): void {
  widgetFitRequests.value = widgetFitRequests.value.filter((r) => r.requestId !== requestId);
}
