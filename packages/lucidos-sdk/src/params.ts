/**
 * Widget params: the JSON object a widget receives for one place it shows
 * (ADR 0415).
 *
 * The host writes them into the frame URL under the one query key
 * {@link WIDGET_PARAMS_QUERY}. Every other query name on an app URL belongs to
 * the host or the engine, so a param can never collide with one of them.
 */

/** The one query key that carries widget params. The host's `appUrl` imports
 *  it, so the writer and the reader cannot drift. */
export const WIDGET_PARAMS_QUERY = 'params';

/** A widget's params: a plain JSON object. */
export type WidgetParams = Record<string, unknown>;

/** Parse the params out of a query string. Anything that is not a JSON object
 *  reads as `{}`, so a widget never has to guard against a list or a number. */
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

/** True for a JSON object, the only shape params take. */
export function isWidgetParams(value: unknown): value is WidgetParams {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** This frame's widget params, `{}` when it was given none. */
export function params(): WidgetParams {
  return parseWidgetParams(window.location.search);
}
