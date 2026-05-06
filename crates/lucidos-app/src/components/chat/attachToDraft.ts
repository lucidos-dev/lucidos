import { showToast } from '../../store/store';
import { addPastedImage, type PastedImage } from './pastedImages';
import { ensureFocusedComposeThread } from '../../store/actions/compose';

function readImageAsBase64(file: File): Promise<PastedImage> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const dataUrl = reader.result as string;
      const commaIdx = dataUrl.indexOf(',');
      const base64 = dataUrl.substring(commaIdx + 1);
      const mimeType = dataUrl.substring(5, dataUrl.indexOf(';'));
      resolve({ base64, mimeType });
    };
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

export async function attachImageToActiveDraft(file: File): Promise<void> {
  const img = await readImageAsBase64(file);
  const id = ensureFocusedComposeThread();
  addPastedImage(id, img);
}

export interface DroppedFileSplit {
  images: File[];
  skipped: string[];
}

export function splitDroppedFiles(files: FileList): DroppedFileSplit {
  const images: File[] = [];
  const skipped: string[] = [];
  for (let i = 0; i < files.length; i++) {
    const file = files[i];
    if (file.type.startsWith('image/')) {
      images.push(file);
    } else {
      skipped.push(file.name);
    }
  }
  return { images, skipped };
}

/** The image attacher is injected so tests can run without FileReader. */
export async function attachDroppedFilesToDraft(
  files: FileList,
  attachImage: (file: File) => Promise<void> = attachImageToActiveDraft,
): Promise<void> {
  const { images, skipped } = splitDroppedFiles(files);
  await Promise.all(images.map((img) => attachImage(img)));
  if (skipped.length > 0) {
    const msg = skipped.length === 1
      ? `Cannot attach "${skipped[0]}" to a message — only images can be attached. Drop on the Files panel to import.`
      : `Cannot attach ${skipped.length} non-image files to a message — only images can be attached. Drop on the Files panel to import.`;
    showToast(msg, 'warning');
  }
}
