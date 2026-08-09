/**
 * Client state for the frontend preview: the Vite dev server the engine
 * supervises inside a coding-agent worktree, so a TypeScript or CSS change is
 * visible in the real app before Apply
 * (`crates/lucidos-engine/src/engine/frontend_preview.rs`).
 *
 * One slot per workspace, so one signal. It is loaded once at startup and kept
 * live by the two transient SSE events; a page that missed them (a reload, a
 * backgrounded PWA) is corrected by the next explicit action's response, which
 * always carries the authoritative status.
 */

import { signal } from '@preact/signals';
import {
  getFrontendPreview,
  startFrontendPreview,
  stopFrontendPreview,
  type FrontendPreviewStatus,
} from '../../api/client';
import { showToast } from '../store';
import { errorDetail } from '../../utils/errorDetail';
import { DEVICE_ID_PARAM } from '../../utils/deviceIdSeed';

/** The one preview slot, or `null` before the first load has answered. */
export const frontendPreview = signal<FrontendPreviewStatus | null>(null);

/** True while a start/stop is in flight, so the buttons can say so. Starting a
 *  cold Vite takes about a second, and the request does not answer until the
 *  server is actually serving. */
export const frontendPreviewBusy = signal(false);

export async function loadFrontendPreview(): Promise<void> {
  try {
    frontendPreview.value = await getFrontendPreview();
  } catch {
    // Telemetry carve-out (.claude/rules/frontend.md): runs at startup with no
    // user intent, and the only cost of a miss is that the row shows "not
    // running" until the user taps it, which then reports the real error.
    console.warn('[FrontendPreview] could not read the preview status');
  }
}

/** Point the preview at `threadId`'s worktree, replacing whatever ran before. */
export async function startPreviewForThread(threadId: string): Promise<void> {
  if (frontendPreviewBusy.value) return;
  frontendPreviewBusy.value = true;
  try {
    frontendPreview.value = await startFrontendPreview(threadId);
  } catch (err) {
    // The engine's refusals name the worktree or the missing file, so the
    // detail is the whole value of the toast.
    showToast(`Could not start the frontend preview: ${errorDetail(err)}`, 'error');
  } finally {
    frontendPreviewBusy.value = false;
  }
}

export async function stopPreview(): Promise<void> {
  if (frontendPreviewBusy.value) return;
  frontendPreviewBusy.value = true;
  try {
    frontendPreview.value = await stopFrontendPreview();
  } catch (err) {
    showToast(`Could not stop the frontend preview: ${errorDetail(err)}`, 'error');
  } finally {
    frontendPreviewBusy.value = false;
  }
}

/**
 * The href to open, built from the page's OWN location.
 *
 * Pure, and deliberately not the engine's `url`: that one is computed from the
 * `Host` of whichever request last touched the endpoint, which may have been
 * this device or may have been the CLI on the host machine. The page knows
 * exactly which name the user reached the workspace under, and handing a phone
 * a `localhost` link is handing it nothing.
 */
export function previewHref(
  port: number | undefined,
  loc: { protocol: string; hostname: string },
  deviceId?: string | null,
  threadId?: string | null,
): string | null {
  if (!port) return null;
  // `location.hostname` is already bare (no port) and, for IPv6, already
  // bracket-free, so the brackets have to go back on for a valid authority.
  const host = loc.hostname.includes(':') ? `[${loc.hostname}]` : loc.hostname;
  const base = `${loc.protocol}//${host}:${port}/`;
  // The preview is a different origin, so it has its own localStorage and would
  // otherwise register as a NEW device and render with none of this one's
  // device-scoped preferences (UI scale among them). Handing the id over is
  // what makes the preview look like the app the user is looking at.
  // See `utils/deviceIdSeed.ts`, which adopts it and strips it from the URL.
  const query = deviceId ? `?${DEVICE_ID_PARAM}=${encodeURIComponent(deviceId)}` : '';
  // Land on the thread the preview was started from. Its own origin means its
  // own navigation state, so the preview otherwise opens on the compose view and
  // the user has to find the thread again to see the change they are looking at.
  // `#thread=` is the existing landing channel (`THREAD_HASH_RE`): it retries
  // while the engine answers and reports a miss, which is what a fresh page load
  // needs. `deviceIdSeed`'s parameter strip goes through `URL.toString()`, which
  // keeps the fragment, so the two ride together.
  const hash = threadId ? `#thread=${encodeURIComponent(threadId)}` : '';
  return `${base}${query}${hash}`;
}

/** SSE: the engine started a preview. Carries the port, never a URL. */
export function handleFrontendPreviewStarted(payload: {
  thread_id?: string;
  port?: number;
}): void {
  frontendPreview.value = {
    running: true,
    thread_id: payload.thread_id,
    port: payload.port,
  };
}

/**
 * SSE: a preview stopped.
 *
 * Keyed on the thread, because the single slot makes start-elsewhere emit a
 * stop for the OLD thread and a start for the new one, and SSE gives no
 * ordering guarantee between two frames. Ignoring a stop for a thread we no
 * longer believe is running is what stops a late stop from erasing the preview
 * the user just moved.
 *
 * A stop that names NO thread is still applied: it cannot be "for a different
 * thread", and a stop we cannot attribute is safer read as authoritative than
 * as noise, since the alternative is a Stop button for a process that is gone.
 */
export function handleFrontendPreviewStopped(payload: { thread_id?: string }): void {
  const current = frontendPreview.value;
  const namesAnother =
    !!payload.thread_id && !!current?.thread_id && current.thread_id !== payload.thread_id;
  if (current?.running && namesAnother) return;
  frontendPreview.value = { running: false };
}
