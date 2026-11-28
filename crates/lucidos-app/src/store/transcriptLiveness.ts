import { effect, untracked, type ReadonlySignal } from '@preact/signals';
import type { ThreadState } from './thread-events';

export interface TranscriptLivenessDeps {
  focused: ReadonlySignal<string | null>;
  threads: ReadonlySignal<Map<string, ThreadState>>;
  isLive(thread: ThreadState): boolean;
  setLive(live: boolean): void;
}

export function watchTranscriptLiveness(deps: TranscriptLivenessDeps): () => void {
  return effect(() => {
    const id = deps.focused.value;
    const thread = id === null ? undefined : deps.threads.value.get(id);
    const live = thread !== undefined && deps.isLive(thread);
    untracked(() => deps.setLive(live));
  });
}
