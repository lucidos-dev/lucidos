import type { App, AppReveal, Loadable } from '../../store/types';

export const COVER_MAX_MS = 3000;

export const READY_MAX_MS = 15000;

export interface AppFrameSignals {
  loaded: boolean;
  ready: boolean;
  fused: boolean;
}

export function liveAppReveal(app: App, apps: Loadable<App[]>): AppReveal {
  const listed = apps.status === 'loaded' ? apps.data.find((a) => a.id === app.id) : undefined;
  return (listed ?? app).reveal;
}

export function revealFuseMs(reveal: AppReveal): number {
  return reveal === 'on-ready' ? READY_MAX_MS : COVER_MAX_MS;
}

export function appFrameRevealed(reveal: AppReveal, s: AppFrameSignals): boolean {
  if (s.fused) return true;
  return reveal === 'on-ready' ? s.ready : s.loaded;
}

export function appFrameLoading(reveal: AppReveal, s: AppFrameSignals): boolean {
  return reveal === 'on-ready' ? !(s.ready || s.fused) : !s.loaded;
}
