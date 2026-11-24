
import { request as httpRequest, SdkError } from './_fetch';
import { appMayCall, normalizeSuffix } from './appReach';
import { assertString } from './_validate';

const REFUSED = 403;

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
      + 'See system-knowhow/js-sdk/lucidos-apiurl.md, lucidos.request.',
    ));
  }
  return httpRequest<T>(resolved.full, init);
}
