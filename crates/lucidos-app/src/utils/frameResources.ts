/** Temporary diagnostics: docs/temporary-measures.md § Page health log. */
import type { WidgetResources } from '@lucidos/widget-frame';

const reports = new WeakMap<HTMLIFrameElement, WidgetResources>();
const fields = ['videoLoaded', 'videoPixels', 'images', 'imagePixels'] as const;

export function noteFrameResources(frame: HTMLIFrameElement, data: Record<string, unknown>): void {
  const report = {} as WidgetResources;
  for (const key of fields) {
    const value = data[key];
    if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) return;
    report[key] = Math.min(value, key.endsWith('Pixels') ? 1_000_000_000 : 10_000);
  }
  reports.set(frame, report);
}

export function frameResources(root: Document): Record<string, number> {
  const total = { frameVideoLoaded: 0, frameVideoPixels: 0, frameImagePixels: 0 };
  for (const frame of root.querySelectorAll('iframe')) {
    const report = reports.get(frame);
    if (!report) continue;
    total.frameVideoLoaded += report.videoLoaded;
    total.frameVideoPixels += report.videoPixels;
    total.frameImagePixels += report.imagePixels;
  }
  return total;
}
