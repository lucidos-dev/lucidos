import { uploadErrorMessage, uploadPluginArchive } from '../../api/client';
import { showToast, showConfirm } from '../../store/store';
import { uploadFiles } from '../../store/actions/artifacts';
import { ensureFocusedComposeThread } from '../../store/actions/compose';
import { attachPendingUpload } from '../../store/actions/imageUploads';
import { sendMessage } from '../../store/actions/chat';
import { sniffImageBytes, imageRejectionMessage } from '../../utils/imageBytes';

const PLUGIN_EXT = '.lucidos-plugin';

export async function attachImageToActiveDraft(source: File): Promise<void> {
  const threadId = ensureFocusedComposeThread();

  let bytes: ArrayBuffer;
  try {
    bytes = await source.arrayBuffer();
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    showToast(`Image upload failed: could not read the image (${reason})`, 'error');
    return;
  }

  const name = source.name || 'pasted-image';
  const verdict = sniffImageBytes(new Uint8Array(bytes));
  if (verdict.kind !== 'accepted') {
    showToast(imageRejectionMessage(verdict, name, source.type), 'error');
    return;
  }

  const file = new File([bytes], name, { type: verdict.mime });

  await attachPendingUpload({ threadId, file, bytes });
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

export async function uploadAndInstallPluginArchive(file: File): Promise<void> {
  let path: string;
  try {
    ({ path } = await uploadPluginArchive(file));
  } catch (err) {
    showToast(`Plugin upload failed: ${uploadErrorMessage(err)}`, 'error');
    return;
  }
  await sendMessage(`Install the plugin at ${path}`);
}

export async function attachDroppedFilesToDraft(
  files: FileList,
  attachImage: (file: File) => Promise<void> = attachImageToActiveDraft,
  installPlugin: (file: File) => Promise<void> = uploadAndInstallPluginArchive,
  importFiles: (files: File[]) => Promise<void> | void = uploadFiles,
  confirm: typeof showConfirm = showConfirm,
): Promise<void> {
  const { images, plugins, skipped } = splitDroppedFiles(files);
  await Promise.all(images.map((img) => attachImage(img)));
  for (const plugin of plugins) {
    await installPlugin(plugin);
  }
  if (skipped.length > 0) {
    const message = skipped.length === 1
      ? `Only images can be attached to a message. Import "${skipped[0].name}" to the Files panel instead?`
      : `Only images can be attached to a message. Import these ${skipped.length} files to the Files panel instead?`;
    if (await confirm(message, 'Import', { variant: 'default' })) {
      await importFiles(skipped);
    }
  }
}
