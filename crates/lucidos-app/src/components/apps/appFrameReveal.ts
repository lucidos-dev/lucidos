import type { App, AppReveal, Loadable } from '../../store/types';

/** Reveal fuse for an on-load frame whose `load` never arrives (a hung
 *  request). A pane covered forever is worse than whatever the frame managed
 *  to paint. */
export const COVER_MAX_MS = 3000;

/** Reveal fuse for an on-ready app that never calls `lucidos.ui.ready()`.
 *  Longer, since the app is loading its own data, but still bounded. */
export const READY_MAX_MS = 15000;

/** What an app frame has told the host since it mounted. */
export interface AppFrameSignals {
  /** The iframe's `load` event fired. */
  loaded: boolean;
  /** The app called `lucidos.ui.ready()`. */
  ready: boolean;
  /** The reveal fuse fired. */
  fused: boolean;
}

/** The open app's `reveal`, read from the live apps list. The open app is a
 *  snapshot taken when it opened, so a manifest edited since would otherwise
 *  keep holding a refreshed frame for a `ready()` the app no longer sends. */
export function liveAppReveal(app: App, apps: Loadable<App[]>): AppReveal {
  const listed = apps.status === 'loaded' ? apps.data.find((a) => a.id === app.id) : undefined;
  return (listed ?? app).reveal;
}

export function revealFuseMs(reveal: AppReveal): number {
  return reveal === 'on-ready' ? READY_MAX_MS : COVER_MAX_MS;
}

/** Whether the cover is off the frame. An on-ready app is revealed by its
 *  `ready()`, even one sent before `load`, and never by `load` alone. */
export function appFrameRevealed(reveal: AppReveal, s: AppFrameSignals): boolean {
  if (s.fused) return true;
  return reveal === 'on-ready' ? s.ready : s.loaded;
}

/** Whether the load bar is owed, before its delay gate. An on-load frame's bar
 *  outlasts the fuse: a frame uncovered early may still be drawing, and the bar
 *  says so until `load`. */
export function appFrameLoading(reveal: AppReveal, s: AppFrameSignals): boolean {
  return reveal === 'on-ready' ? !(s.ready || s.fused) : !s.loaded;
}
