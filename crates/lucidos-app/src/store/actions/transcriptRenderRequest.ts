import { signal } from '@preact/signals';

export const transcriptRenderRequest = signal<{ threadId: string; key: string } | null>(null);
