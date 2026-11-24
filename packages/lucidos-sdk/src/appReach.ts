
import { APP_REACHABLE_ROUTES } from './generated/app-reach';

function segments(path: string): string[] {
  return path.split('/');
}

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

export function normalizeSuffix(path: string): { pathname: string; full: string } | null {
  if (!path.startsWith('/') || path.startsWith('//')) return null;
  let url: URL;
  try {
    url = new URL(path, 'https://app.invalid');
  } catch {
    return null;
  }
  return { pathname: url.pathname, full: `${url.pathname}${url.search}${url.hash}` };
}

export function appMayCall(method: string, pathname: string, appId?: string | null): boolean {
  return appReachableMethods(pathname, appId).includes(method.toUpperCase());
}

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

function segmentRank(segment: string): number {
  if (segment.startsWith('*')) return 0;
  if (segment.startsWith(':')) return 1;
  return 2;
}

function moreSpecific(a: string, b: string): boolean {
  const left = segments(a);
  const right = segments(b);
  for (let i = 0; i < Math.min(left.length, right.length); i++) {
    const diff = segmentRank(left[i]) - segmentRank(right[i]);
    if (diff !== 0) return diff > 0;
  }
  return left.length > right.length;
}
