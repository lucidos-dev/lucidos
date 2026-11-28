
export const WIDGET_PARAMS_QUERY = 'params';

export type WidgetParams = Record<string, unknown>;

export function parseWidgetParams(search: string): WidgetParams {
  const raw = new URLSearchParams(search).get(WIDGET_PARAMS_QUERY);
  if (raw === null) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    return isWidgetParams(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

export function isWidgetParams(value: unknown): value is WidgetParams {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function params(): WidgetParams {
  return parseWidgetParams(window.location.search);
}
