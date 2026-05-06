import { computed } from '@preact/signals';
import { focusedThreadId } from '../../store/store';
import { updateCompose } from '../../store/actions/compose';
import { getDraft } from '../../store/composeDrafts';
import { inferMimeFromBase64 } from '../../utils/inferMimeFromBase64';

export type { PastedImage } from '../../utils/inferMimeFromBase64';
import type { PastedImage } from '../../utils/inferMimeFromBase64';

const EMPTY: PastedImage[] = [];

export function getPastedImages(threadId: string | null): PastedImage[] {
  const wire = getDraft(threadId).images;
  if (wire.length === 0) return EMPTY;
  return wire.map((base64) => ({ base64, mimeType: inferMimeFromBase64(base64) }));
}

export const pastedImagesForCurrentThread = computed<PastedImage[]>(() =>
  getPastedImages(focusedThreadId.value),
);

export function addPastedImage(threadId: string, image: PastedImage): void {
  const next = [...getDraft(threadId).images, image.base64];
  updateCompose(threadId, { images: next });
}

export function removePastedImage(threadId: string, index: number): void {
  const next = getDraft(threadId).images.filter((_, i) => i !== index);
  updateCompose(threadId, { images: next });
}
