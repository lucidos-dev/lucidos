import { batch } from '@preact/signals';

import { uploadThreadBlob, uploadPluginArchive, ApiError } from '../../api/client';
import { showToast } from '../../store/store';
import { awaitThreadStarted, ensureFocusedComposeThread } from '../../store/actions/compose';
import { sendMessage } from '../../store/actions/chat';
import {
  addPendingUpload,
  detachPendingUpload,
  hasPendingUpload,
  patchPendingUpload,
} from '../../store/pendingUploads';
import { addAttachedImageHash, rememberSessionBlobUrl } from './pastedImages';

const PLUGIN_EXT = '.lucidos-plugin';

export async function attachImageToActiveDraft(file: File): Promise<void> {
  const threadId = ensureFocusedComposeThread();

  // Show the preview immediately, then upload in the background. The blob
  // URL is handed off twice: first to the pending entry (preview while
  // uploading), then to `sessionBlobUrls` via `rememberSessionBlobUrl` so
  // the confirmed image keeps rendering from the same in-memory File. This
  // dodges the per-browser quirk where preloading the server URL doesn't
  // reliably warm the HTTP cache (notably iOS Safari PWA), which used to
  // surface as a brief black flash when the preview swapped to a fresh
  // `<img src="/api/v1/blobs/<hash>">` that re-fetched over the network.
  const previewUrl = URL.createObjectURL(file);
  const localId = crypto.randomUUID();
  addPendingUpload({
    localId,
    threadId,
    previewUrl,
    mime: file.type,
    status: 'uploading',
    file,
  });

  try {
    // ensureFocusedComposeThread fires POST /threads as fire-and-forget.
    // The blob endpoint guards on thread_summaries existing, so without
    // this await a fresh-draft paste 404s with "thread not found".
    await awaitThreadStarted(threadId);
    const { hash } = await uploadThreadBlob(threadId, file);
    // Mid-flight cancel: the user clicked X on the preview while we were
    // uploading. `detachPendingUpload` returns null in that case — meaning
    // `removePendingUpload` already revoked the URL — so we bail without
    // committing the hash to the draft.
    if (!hasPendingUpload(threadId, localId)) return;
    // Promote: hand the blob URL ownership to the session map FIRST so
    // `getAttachedImages` will return it the moment the hash lands in the
    // draft, then commit the hash and detach the pending entry without
    // revoking the URL. `batch` collapses the writes into one render so
    // the strip never momentarily renders with neither entry (an empty
    // strip would unmount the wrapper and pop).
    batch(() => {
      rememberSessionBlobUrl(hash, previewUrl);
      addAttachedImageHash(threadId, hash);
      detachPendingUpload(threadId, localId);
    });
  } catch (err) {
    const reason =
      err instanceof ApiError
        ? err.reason
        : err instanceof Error
        ? err.message
        : String(err);
    patchPendingUpload(threadId, localId, { status: 'failed', error: reason });
    showToast(`Image upload failed: ${reason}`, 'error');
  }
}

export interface DroppedFileSplit {
  images: File[];
  plugins: File[];
  skipped: string[];
}

export function splitDroppedFiles(files: FileList): DroppedFileSplit {
  const images: File[] = [];
  const plugins: File[] = [];
  const skipped: string[] = [];
  for (let i = 0; i < files.length; i++) {
    const file = files[i];
    if (file.type.startsWith('image/')) {
      images.push(file);
    } else if (file.name.toLowerCase().endsWith(PLUGIN_EXT)) {
      plugins.push(file);
    } else {
      skipped.push(file.name);
    }
  }
  return { images, plugins, skipped };
}

/** Bridge from a browser File blob to the LLM `install_plugin` tool, which
 *  only accepts a server-absolute archive path. */
export async function uploadAndInstallPluginArchive(file: File): Promise<void> {
  let path: string;
  try {
    ({ path } = await uploadPluginArchive(file));
  } catch (err) {
    const reason =
      err instanceof ApiError
        ? err.reason
        : err instanceof Error
        ? err.message
        : String(err);
    showToast(`Plugin upload failed: ${reason}`, 'error');
    return;
  }
  await sendMessage(`Install the plugin at ${path}`);
}

/** The image attacher and plugin installer are injected so tests can run
 *  without real network or thread state. */
export async function attachDroppedFilesToDraft(
  files: FileList,
  attachImage: (file: File) => Promise<void> = attachImageToActiveDraft,
  installPlugin: (file: File) => Promise<void> = uploadAndInstallPluginArchive,
): Promise<void> {
  const { images, plugins, skipped } = splitDroppedFiles(files);
  await Promise.all(images.map((img) => attachImage(img)));
  // Serial so the per-install chat messages arrive in drop order; the LLM
  // processes them one at a time anyway.
  for (const plugin of plugins) {
    await installPlugin(plugin);
  }
  if (skipped.length > 0) {
    const msg = skipped.length === 1
      ? `Cannot attach "${skipped[0]}" to a message — only images can be attached. Drop on the Files panel to import.`
      : `Cannot attach ${skipped.length} non-image files to a message — only images can be attached. Drop on the Files panel to import.`;
    showToast(msg, 'warning');
  }
}
