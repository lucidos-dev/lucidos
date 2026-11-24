
import { signal } from '@preact/signals';
import {
  getFrontendPreview,
  startFrontendPreview,
  stopFrontendPreview,
  type FrontendPreviewStatus,
} from '../../api/client';
import { showToast } from '../store';
import { failedIfFresh, setLoadingIfFresh, type Loadable } from '../types';
import { errorDetail } from '../../utils/errorDetail';
import { DEVICE_ID_PARAM } from '../../utils/deviceIdSeed';
import { landingHash } from '../../utils/workspaceLanding';
import { isLoopbackHostname } from '../../utils/loopbackHostname';

export const frontendPreview = signal<Loadable<FrontendPreviewStatus>>({ status: 'not-loaded' });

export const frontendPreviewBusy = signal(false);

export async function loadFrontendPreview(): Promise<void> {
  setLoadingIfFresh(frontendPreview);
  try {
    frontendPreview.value = { status: 'loaded', data: await getFrontendPreview() };
  } catch (err) {
    frontendPreview.value = failedIfFresh(frontendPreview.value, err);
  }
}

export async function startPreviewForThread(threadId: string): Promise<void> {
  if (frontendPreviewBusy.value) return;
  frontendPreviewBusy.value = true;
  try {
    frontendPreview.value = { status: 'loaded', data: await startFrontendPreview(threadId) };
  } catch (err) {
    showToast(`Could not start the frontend preview: ${errorDetail(err)}`, 'error');
  } finally {
    frontendPreviewBusy.value = false;
  }
}

export async function stopPreview(): Promise<void> {
  if (frontendPreviewBusy.value) return;
  frontendPreviewBusy.value = true;
  try {
    frontendPreview.value = { status: 'loaded', data: await stopFrontendPreview() };
  } catch (err) {
    showToast(`Could not stop the frontend preview: ${errorDetail(err)}`, 'error');
  } finally {
    frontendPreviewBusy.value = false;
  }
}

export function previewHref(
  preview: Pick<FrontendPreviewStatus, 'port' | 'loopback_only'> | null | undefined,
  hostname: string,
  deviceId?: string | null,
  threadId?: string | null,
): string | null {
  const port = preview?.port;
  if (!port) return null;
  if (preview.loopback_only && !isLoopbackHostname(hostname)) return null;
  const host = preview.loopback_only
    ? 'localhost'
    : hostname.includes(':')
      ? `[${hostname}]`
      : hostname;
  const base = `https://${host}:${port}/`;
  const query = deviceId ? `?${DEVICE_ID_PARAM}=${encodeURIComponent(deviceId)}` : '';
  const hash = threadId ? landingHash({ thread: threadId }) : '';
  return `${base}${query}${hash}`;
}

export function handleFrontendPreviewStarted(payload: {
  thread_id?: string;
  port?: number;
  loopback_only?: boolean;
}): void {
  frontendPreview.value = {
    status: 'loaded',
    data: {
      running: true,
      thread_id: payload.thread_id,
      port: payload.port,
      loopback_only: payload.loopback_only,
    },
  };
}

export function previewIsElsewhere(
  preview: Pick<FrontendPreviewStatus, 'loopback_only'> | null | undefined,
  hostname: string,
): boolean {
  return preview?.loopback_only === true && !isLoopbackHostname(hostname);
}

export function handleFrontendPreviewStopped(payload: { thread_id?: string }): void {
  const current = frontendPreview.value.status === 'loaded' ? frontendPreview.value.data : null;
  const namesAnother =
    !!payload.thread_id && !!current?.thread_id && current.thread_id !== payload.thread_id;
  if (current?.running && namesAnother) return;
  frontendPreview.value = { status: 'loaded', data: { running: false } };
}
