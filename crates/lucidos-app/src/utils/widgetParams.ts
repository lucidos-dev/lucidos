import type { WidgetParams } from '@lucidos/sdk';

export type { WidgetParams };

/** JSON with every object's keys sorted, so one set of params always gives one
 *  string, whatever order the agent wrote it in (ADR 0415). It keys instances
 *  on this side only: the engine compares its own `canonical_widget_params`. */
export function canonicalWidgetParams(params: WidgetParams | undefined): string {
  return JSON.stringify(sortKeys(params ?? {}));
}

function sortKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (typeof value !== 'object' || value === null) return value;
  const record = value as Record<string, unknown>;
  return Object.fromEntries(Object.keys(record).sort().map((k) => [k, sortKeys(record[k])]));
}

/** True when there is at least one param to carry. */
export function hasWidgetParams(params: WidgetParams | undefined): params is WidgetParams {
  return params !== undefined && Object.keys(params).length > 0;
}

/** One *widget instance*'s key: the widget plus its canonical params. The
 *  label is not part of it (ADR 0415). */
export function widgetInstanceKey(appId: string, params: WidgetParams | undefined): string {
  return `${appId}?${canonicalWidgetParams(params)}`;
}
