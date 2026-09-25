import { effect, untracked, type ReadonlySignal } from '@preact/signals';
import type { ThreadState } from './thread-events';

export interface TranscriptLivenessDeps {
  /** The thread on screen. */
  focused: ReadonlySignal<string | null>;
  threads: ReadonlySignal<Map<string, ThreadState>>;
  /** Is this thread live streaming? The agent running (`isThreadStreaming`),
   *  or a voice call up on it. */
  isLive(thread: ThreadState): boolean;
  /** `setTranscriptLive` in `components/chat/scrollState`. */
  setLive(live: boolean): void;
}

/**
 * Tell the transcript's standing follow whether the thread on screen is live
 * streaming. A reader's scroll turns the follow off only then, and parks it
 * otherwise (`leaveTheRideByScroll`).
 *
 * The THREAD PROJECTION is the source for the agent, never a turn's rendered
 * status, which is a second copy that drifts. A call writes no turn, so it is
 * asked for separately.
 *
 * Wired in `store/effects.ts`: the stores are the producers and the transcript
 * the consumer, and a producer must not import a consumer. A factory, so a
 * test drives its own focus and threads. Returns the dispose.
 */
export function watchTranscriptLiveness(deps: TranscriptLivenessDeps): () => void {
  return effect(() => {
    const id = deps.focused.value;
    const thread = id === null ? undefined : deps.threads.value.get(id);
    const live = thread !== undefined && deps.isLive(thread);
    // Untracked: the wake reads and writes scroll state, which must not
    // become a dependency of this effect.
    untracked(() => deps.setLive(live));
  });
}
