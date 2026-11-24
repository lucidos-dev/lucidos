
import { perfRecordingOn, recordPerfSample } from './perfQueue';

const MARK_STALE_MS = 2_000;

export type NavigationKind = 'pane' | 'content';

export type NavigationFiredBy = 'pane' | 'content-view';

export interface NavigationMark {
  kind: NavigationKind;
  to: string;
  start: number;
  expectKey?: string | null;
}

let pending: NavigationMark | null = null;

let generation = 0;

export function navigationClass(viewKey: string | null): string {
  if (!viewKey) return 'none';
  const head = viewKey.split(':', 1)[0];
  return head || 'none';
}

export function markNavigationStart(
  kind: NavigationKind,
  to: string,
  start: number,
  expectKey?: string | null,
): void {
  if (!perfRecordingOn()) return;
  generation += 1;
  pending = { kind, to, start, expectKey };
}

export function takeNavigationStart(): NavigationMark | undefined {
  const mark = pending;
  pending = null;
  return mark ?? undefined;
}

export function reportNavigation(firedBy: NavigationFiredBy, arrivingKey?: string | null): void {
  const mark = takeNavigationStart();
  if (!mark) return;
  if (mark.expectKey !== undefined && mark.expectKey !== (arrivingKey ?? null)) return;
  if (performance.now() - mark.start > MARK_STALE_MS) return;
  const to = arrivingKey !== undefined ? navigationClass(arrivingKey) : mark.to;
  const gen = generation;
  requestAnimationFrame(() => {
    if (gen !== generation) return;
    const ms = performance.now() - mark.start;
    if (ms > MARK_STALE_MS) return;
    recordPerfSample('navigation', { kind: mark.kind, to, firedBy, ms: Math.round(ms) });
  });
}

export function _resetNavigationMarksForTesting(): void {
  pending = null;
  generation = 0;
}
