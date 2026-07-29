import { batch } from '@preact/signals';

import { uploadThreadBlob, uploadPluginArchive, ApiError } from '../../api/client';
import { showToast, showConfirm } from '../../store/store';
import { uploadFiles } from '../../store/actions/artifacts';
import { awaitThreadStarted, ensureFocusedComposeThread } from '../../store/actions/compose';
import { sendMessage } from '../../store/actions/chat';
import {
  addPendingUpload,
  detachPendingUpload,
  hasPendingUpload,
  patchPendingUpload,
} from '../../store/pendingUploads';
import { addAttachedImageHash, rememberSessionBlobUrl } from './pastedImages';
import { generateUuid } from '../../utils/uuid';

const PLUGIN_EXT = '.lucidos-plugin';

export async function attachImageToActiveDraft(source: File): Promise<void> {
  const threadId = ensureFocusedComposeThread();

  // Snapshot the bytes into an in-memory File before any await. A File handed
  // to us by a paste or drop is often backed by the system clipboard/pasteboard
  // rather than memory, and that backing is only reliably readable during the
  // synchronous event turn. macOS Universal Clipboard (an image copied on an
  // iPhone, pasted on the Mac) is the sharp case: the File's bytes live behind
  // a promised pasteboard resource that the browser releases once the paste
  // event returns — the `await awaitThreadStarted` gap below is enough for that
  // to happen, after which BOTH the `<img>` preview and the upload `fetch` fail
  // with a cryptic "Failed to fetch" (a broken thumbnail plus the error toast
  // the user sees). `arrayBuffer()` is invoked here, still inside the event
  // turn, so everything downstream works off a stable copy. A regular in-memory
  // File (photo picker, camera capture) round-trips through this as a cheap
  // no-op copy.
  let file: File;
  try {
    const bytes = await source.arrayBuffer();
    file = new File([bytes], source.name || 'pasted-image', { type: source.type });
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    showToast(`Image upload failed: could not read the image (${reason})`, 'error');
    return;
  }

  // Show the preview immediately, then upload in the background. The blob
  // URL is handed off twice: first to the pending entry (preview while
  // uploading), then to `sessionBlobUrls` via `rememberSessionBlobUrl` so
  // the confirmed image keeps rendering from the same in-memory File. This
  // dodges the per-browser quirk where preloading the server URL doesn't
  // reliably warm the HTTP cache (notably iOS Safari PWA), which used to
  // surface as a brief black flash when the preview swapped to a fresh
  // `<img src="/api/v1/blobs/<hash>">` that re-fetched over the network.
  const previewUrl = URL.createObjectURL(file);
  const localId = generateUuid();
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
  skipped: File[];
}

export function splitDroppedFiles(files: FileList): DroppedFileSplit {
  const images: File[] = [];
  const plugins: File[] = [];
  const skipped: File[] = [];
  for (let i = 0; i < files.length; i++) {
    const file = files[i];
    if (file.type.startsWith('image/')) {
      images.push(file);
    } else if (file.name.toLowerCase().endsWith(PLUGIN_EXT)) {
      plugins.push(file);
    } else {
      skipped.push(file);
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

/** The image attacher, plugin installer, file importer, and confirm prompt are
 *  injected so tests can run without real network, thread state, or a rendered
 *  dialog. */
export async function attachDroppedFilesToDraft(
  files: FileList,
  attachImage: (file: File) => Promise<void> = attachImageToActiveDraft,
  installPlugin: (file: File) => Promise<void> = uploadAndInstallPluginArchive,
  importFiles: (files: File[]) => Promise<void> | void = uploadFiles,
  confirm: typeof showConfirm = showConfirm,
): Promise<void> {
  const { images, plugins, skipped } = splitDroppedFiles(files);
  await Promise.all(images.map((img) => attachImage(img)));
  // Serial so the per-install chat messages arrive in drop order; the LLM
  // processes them one at a time anyway.
  for (const plugin of plugins) {
    await installPlugin(plugin);
  }
  if (skipped.length > 0) {
    // Non-image, non-plugin files can't ride along on a message — but the user
    // clearly wanted to do *something* with them. Instead of dead-ending with a
    // warning, offer the one action that works: import them into the Files panel
    // (the same thing a drop on that panel would do).
    const message = skipped.length === 1
      ? `Only images can be attached to a message. Import "${skipped[0].name}" to the Files panel instead?`
      : `Only images can be attached to a message. Import these ${skipped.length} files to the Files panel instead?`;
    if (await confirm(message, 'Import', { variant: 'default' })) {
      await importFiles(skipped);
    }
  }
}
