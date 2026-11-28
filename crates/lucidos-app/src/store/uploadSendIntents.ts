import { signal } from '@preact/signals';

export interface UploadSendIntent<TContext = unknown> {
  useCodingAgent: boolean;
  context: TContext | null;
  asSideQuestion?: boolean;
}

export const queuedUploadSends = signal<Map<string, UploadSendIntent>>(new Map());

