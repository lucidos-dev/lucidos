import { signal } from '@preact/signals';

export interface UploadSendIntent<TContext = unknown> {
  useCodingAgent: boolean;
  context: TContext | null;
  /** Ask the draft as a side question rather than send it. */
  asSideQuestion?: boolean;
}

/** Thread sends the user clicked while an attached image was still uploading.
 *  The draft stays intact and the actual send is retried once the pending
 *  upload entries settle into confirmed draft hashes. While queued, the same
 *  optimistic submitting signal as a normal send drives the Send→Cancel morph,
 *  and the composer shows the wait (`uploadSendNotice`). */
export const queuedUploadSends = signal<Map<string, UploadSendIntent>>(new Map());

