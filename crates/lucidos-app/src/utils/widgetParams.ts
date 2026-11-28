import type { WidgetParams } from '@lucidos/sdk';

export type { WidgetParams };

export function canonicalWidgetParams(params: WidgetParams | undefined): string {
  return JSON.stringify(sortKeys(params ?? {}));
}

function sortKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (typeof value !== 'object' || value === null) return value;
  const record = value as Record<string, unknown>;
  return Object.fromEntries(Object.keys(record).sort().map((k) => [k, sortKeys(record[k])]));
}

export function hasWidgetParams(params: WidgetParams | undefined): params is WidgetParams {
  return params !== undefined && Object.keys(params).length > 0;
}

export function widgetInstanceKey(appId: string, params: WidgetParams | undefined): string {
  return `${appId}?${canonicalWidgetParams(params)}`;
}
