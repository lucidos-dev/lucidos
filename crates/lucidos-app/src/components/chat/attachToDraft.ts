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
import { sniffImageBytes, imageRejectionMessage } from '../../utils/imageBytes';

const PLUGIN_EXT = '.lucidos-plugin';

/** Why an upload failed, for the toast and the pending-upload row.
 *  Deliberately not `utils/errorDetail`: that one reads `Error.message`, and an
 *  `ApiError` carries the engine's sentence in `reason` instead. */
function uploadFailureReason(err: unknown): string {
  if (err instanceof ApiError) return err.reason;
  if (err instanceof Error) return err.message;
  return String(err);
}

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
  let bytes: ArrayBuffer;
  try {
    bytes = await source.arrayBuffer();
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    showToast(`Image upload failed: could not read the image (${reason})`, 'error');
    return;
  }

  // Ask the server's own question here, on the bytes, before anything is
  // drawn. Every inbound path (paste, drop, file picker, camera) funnels
  // through this function, and each of them gated on the DECLARED type only.
  // A macOS clipboard flavour can declare `image/png` and hand over zero
  // bytes. That drew a chip with a broken thumbnail, then failed at the
  // upload seconds later in a different corner of the screen.
  const name = source.name || 'pasted-image';
  const verdict = sniffImageBytes(new Uint8Array(bytes));
  if (verdict.kind !== 'accepted') {
    showToast(imageRejectionMessage(verdict, name, source.type), 'error');
    return;
  }

  // The sniffed mime, not the declared one. A wrong declared type is what
  // makes the browser guess at the preview.
  const file = new File([bytes], name, { type: verdict.mime });

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
    // uploading. `removePendingUpload` dropped the entry and revoked the URL,
    // so `hasPendingUpload` reads false. Bail without committing the hash to
    // the draft.
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
    const reason = uploadFailureReason(err);
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
    showToast(`Plugin upload failed: ${uploadFailureReason(err)}`, 'error');
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
