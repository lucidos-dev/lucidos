/**
 * May an app frame call this engine route?
 *
 * The answer is the engine's, generated from `ROUTE_REACH` in
 * `crates/lucidos-engine/src/api/app_reach.rs` (ADR 0231). This file is the
 * matcher for it, and the only copy: the SDK checks here before sending, and
 * the host bridge checks here before making the call.
 *
 * Default deny. A route the generated table does not list is refused, so a new
 * engine route is unreachable from an app until somebody answers for it.
 */

import { APP_REACHABLE_ROUTES } from './generated/app-reach';

/** Split a path into segments, dropping nothing, so a trailing slash is kept.
 *  `/proxy/:name/` is a real route and differs from `/proxy/:name`. */
function segments(path: string): string[] {
  return path.split('/');
}

/**
 * Does a concrete path match one route pattern?
 *
 * `:param` takes exactly one non-empty segment. `*rest` takes the remainder,
 * and needs at least one segment, matching how the engine registers
 * `/data/*path`.
 */
export function pathMatchesPattern(pattern: string, path: string): boolean {
  const pat = segments(pattern);
  const got = segments(path);
  for (let i = 0; i < pat.length; i++) {
    const p = pat[i];
    if (p.startsWith('*')) return got.length > i && got.slice(i).some(s => s.length > 0);
    if (i >= got.length) return false;
    if (p.startsWith(':')) {
      if (got[i].length === 0) return false;
      continue;
    }
    if (p !== got[i]) return false;
  }
  return pat.length === got.length;
}

/**
 * The `/api/v1` path suffix this request addresses, or null if it is not a
 * suffix this may resolve at all.
 *
 * Normalised FIRST, and the caller sends what comes back, so what was checked
 * and what is sent are one string. Checking the raw path and sending it
 * separately is the whole bug class: the URL parser resolves a dot segment
 * afterwards, and it counts `%2e%2e` as one, so `/data/%2e%2e/credentials`
 * passes a literal `..` scan and lands on `/api/v1/credentials`.
 *
 * Two refusals come first. A path not starting with `/` would resolve against
 * the calling document. A leading `//` is protocol-relative, so
 * `//example.com/x` names another host entirely.
 */
export function normalizeSuffix(path: string): { pathname: string; full: string } | null {
  if (!path.startsWith('/') || path.startsWith('//')) return null;
  let url: URL;
  try {
    // A base that cannot resolve anywhere: only the parsing is wanted, and a
    // real origin here would make an absolute path look reachable.
    url = new URL(path, 'https://app.invalid');
  } catch {
    return null;
  }
  return { pathname: url.pathname, full: `${url.pathname}${url.search}${url.hash}` };
}

/**
 * May an app call `method` on this `/api/v1` suffix?
 *
 * `appId` is the calling app, which the host resolves from the frame. Nothing
 * reads it yet. It is a parameter so per-app grants can land without
 * re-plumbing every caller (ADR 0231 decision 4).
 */
export function appMayCall(method: string, pathname: string, appId?: string | null): boolean {
  return appReachableMethods(pathname, appId).includes(method.toUpperCase());
}

/**
 * Every method an app may use on this path, empty when the route is refused.
 *
 * ONE route answers, the most specific match, because that is the one the
 * engine routes to. Unioning every match gave `/data/edit` the wildcard's
 * `GET`, `PUT` and `DELETE` on top of its own `POST`. A `Host` route added
 * under an `App` wildcard later would have been admitted outright.
 */
export function appReachableMethods(pathname: string, appId?: string | null): string[] {
  void appId;
  const matches = APP_REACHABLE_ROUTES.filter(route => pathMatchesPattern(route.path, pathname));
  if (matches.length === 0) return [];
  let best = matches[0];
  for (const route of matches.slice(1)) {
    if (moreSpecific(route.path, best.path)) best = route;
  }
  return best.methods;
}

/** Rank one segment: a literal beats a `:param`, which beats a `*wildcard`.
 *  The same order `matchit` gives them inside the engine's router. */
function segmentRank(segment: string): number {
  if (segment.startsWith('*')) return 0;
  if (segment.startsWith(':')) return 1;
  return 2;
}

/** Is `a` the route the engine would pick over `b`?
 *
 *  Compared segment by segment, left to right, at the first one where the two
 *  disagree. Both already match the same concrete path, so a difference in
 *  length means one ran out at a wildcard, and the longer literal wins. */
function moreSpecific(a: string, b: string): boolean {
  const left = segments(a);
  const right = segments(b);
  for (let i = 0; i < Math.min(left.length, right.length); i++) {
    const diff = segmentRank(left[i]) - segmentRank(right[i]);
    if (diff !== 0) return diff > 0;
  }
  return left.length > right.length;
}
