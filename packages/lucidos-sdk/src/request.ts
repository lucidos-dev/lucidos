/**
 * Call an engine endpoint no other SDK method covers.
 *
 * The bridged escape hatch of ADR 0231. It takes a path suffix under
 * `/api/v1`, travels the host bridge from an app frame, and goes direct from a
 * standalone app tab. Errors, deadline and response parsing are the SDK's
 * usual ones, so a caller handles it exactly like `lucidos.data.read`.
 *
 * Not every route is reachable. The engine classifies each one, and this
 * refuses the rest before anything is sent. See `appReach.ts`.
 */

import { request as httpRequest, SdkError } from './_fetch';
import { appMayCall, normalizeSuffix } from './appReach';
import { assertString } from './_validate';

/** HTTP status for a route an app may not reach. The refusal is local, and it
 *  carries the code the engine would answer with, so a caller has one shape to
 *  handle either way. */
const REFUSED = 403;

/**
 * Call `/api/v1<suffix>` and parse the JSON answer.
 *
 * ```js
 * const { env_vars } = await lucidos.request('/env-vars');
 * await lucidos.request('/events/emit', {
 *   method: 'POST',
 *   headers: { 'Content-Type': 'application/json' },
 *   body: JSON.stringify({ event_type: 'ReportOpened', payload: { summary: 'x' } }),
 * });
 * ```
 *
 * Throws `SdkError` on a non-2xx answer, and on a route an app may not reach.
 * A response with no body resolves to `null`.
 */
export function request<T = unknown>(suffix: string, init?: RequestInit): Promise<T> {
  assertString('suffix', suffix);
  const resolved = normalizeSuffix(suffix);
  if (!resolved) {
    return Promise.reject(new SdkError(
      REFUSED,
      `lucidos.request: "${suffix}" is not a path under /api/v1. Pass the part after it, `
      + 'such as "/env-vars".',
    ));
  }
  const method = init?.method ?? 'GET';
  if (!appMayCall(method, resolved.pathname)) {
    return Promise.reject(new SdkError(
      REFUSED,
      `lucidos.request: an app may not call ${method.toUpperCase()} ${resolved.pathname}. `
      + 'See system-knowhow/js-sdk.md, lucidos.request.',
    ));
  }
  // The resolved path, never the raw one. What was checked and what is sent
  // have to be the same string.
  return httpRequest<T>(resolved.full, init);
}
