
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
import { landingHash } from '../../utils/workspaceLanding';

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
  preview: Pick<FrontendPreviewStatus, 'path'> | null | undefined,
  origin: string,
  threadId?: string | null,
): string | null {
  const path = preview?.path;
  if (!path || !/^\/~preview\/[a-f0-9]{16}\/[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\/$/.test(path)) return null;
  const hash = threadId ? landingHash({ thread: threadId }) : '';
  return `${new URL(path, origin).href}${hash}`;
}

export function handleFrontendPreviewStarted(payload: {
  thread_id?: string;
  path?: string;
}): void {
  frontendPreview.value = {
    status: 'loaded',
    data: {
      running: true,
      thread_id: payload.thread_id,
      path: payload.path,
    },
  };
}

export function handleFrontendPreviewStopped(payload: { thread_id?: string }): void {
  const current = frontendPreview.value.status === 'loaded' ? frontendPreview.value.data : null;
  const namesAnother =
    !!payload.thread_id && !!current?.thread_id && current.thread_id !== payload.thread_id;
  if (current?.running && namesAnother) return;
  frontendPreview.value = { status: 'loaded', data: { running: false } };
}
