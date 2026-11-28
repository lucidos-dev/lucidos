import { useEffect } from 'preact/hooks';
import { effect } from '@preact/signals';
import { connectionStatus, databaseReachable, scaledDurationMs, threadsLoaded } from '../store/store';
import {
  bootSplashPlaysNoReveal,
  bootSplashPresent,
  dismissBootSplash,
  revealBootEscape,
  setBootStatus,
} from '../utils/bootSplash';
import { bootDeepLinkLanded } from '../utils/bootTiming';
import { WORKSPACE_ID } from '../utils/basePath';
import type { ConnectionStatus } from '../store/types';

export const BOOT_SPLASH_SAFETY_MS = 15_000;

export const BOOT_SPLASH_MIN_REVEAL_MS = 1_200;

export const STATUS_DELAY_MS = 3_000;

function msSinceLoad(): number {
  return typeof performance !== 'undefined' ? performance.now() : BOOT_SPLASH_MIN_REVEAL_MS;
}

export function remainingRevealMs(elapsedMs: number, revealSkipped: boolean): number {
  if (revealSkipped) return 0;
  return Math.max(0, scaledDurationMs(BOOT_SPLASH_MIN_REVEAL_MS) - elapsedMs);
}

export function isWorkspaceReady(connection: ConnectionStatus, threads: boolean): boolean {
  return connection === 'connected' && threads;
}

export function bootSplashShouldRelease(
  connection: ConnectionStatus,
  threads: boolean,
  databaseIsReachable: boolean,
  notificationLanded = false,
): boolean {
  if (isWorkspaceReady(connection, threads)) return true;
  return connection === 'connected' && (!databaseIsReachable || notificationLanded);
}

export function delayedBootStatus(connection: ConnectionStatus, isDirect: boolean): string {
  if (connection === 'connected') return 'Loading…';
  if (workspaceIsUnreachable(connection, isDirect)) return 'Workspace not started';
  return 'Connecting…';
}

export function workspaceIsUnreachable(connection: ConnectionStatus, isDirect: boolean): boolean {
  return connection === 'disconnected' && isDirect;
}

export function startDelayedBootStatus(isDirect: boolean): () => void {
  let stop: (() => void) | null = null;
  const timer = window.setTimeout(() => {
    stop = effect(() => {
      setBootStatus(delayedBootStatus(connectionStatus.value, isDirect));
      if (workspaceIsUnreachable(connectionStatus.value, isDirect)) revealBootEscape();
    });
  }, STATUS_DELAY_MS);
  return () => {
    clearTimeout(timer);
    stop?.();
    stop = null;
  };
}

export function useBootSplashReady(): void {
  useEffect(() => {
    if (!bootSplashPresent()) return;

    const stopStatus = startDelayedBootStatus(WORKSPACE_ID === null);
    let dismissArmed = false;
    let revealTimer: ReturnType<typeof setTimeout> | undefined;

    function finish() {
      clearTimeout(cap);
      stopStatus();
      dismissBootSplash();
    }

    const cap = window.setTimeout(finish, BOOT_SPLASH_SAFETY_MS);

    const stop = effect(() => {
      if (dismissArmed) return;
      if (!bootSplashShouldRelease(
        connectionStatus.value,
        threadsLoaded.value,
        databaseReachable.value,
        bootDeepLinkLanded.value,
      )) return;
      dismissArmed = true;
      const remaining = remainingRevealMs(msSinceLoad(), bootSplashPlaysNoReveal());
      if (remaining === 0) finish();
      else revealTimer = window.setTimeout(finish, remaining);
    });

    return () => {
      clearTimeout(cap);
      if (revealTimer !== undefined) clearTimeout(revealTimer);
      stopStatus();
      stop();
    };
  }, []);
}
