import { signal } from '@preact/signals';

/** A turn the transcript is asked to draw, by `exchangeKey`. ThreadView
 *  answers it by moving its render window down to that turn. A module of its
 *  own, so ThreadView reaches it without loading the find targets. */
export const transcriptRenderRequest = signal<{ threadId: string; key: string } | null>(null);
