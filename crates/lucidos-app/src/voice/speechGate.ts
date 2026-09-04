/**
 * Deciding that the caller is speaking, from measured energy alone.
 *
 * One question, two readers. Armed while the talker holds the floor, the gate
 * opening is a barge-in: the engine reads an interruption from the provider's
 * own finished response, never from the caller starting to speak, and says so
 * in `voice/realtime.rs::done_events`. Armed while the caller holds it, the
 * same opening is a live utterance, which is what draws their bubble before the
 * words land.
 *
 * Neither reader lives here. `callState.ts` decides what an edge means, so this
 * file knows nothing about who has the floor.
 *
 * `getUserMedia` is asked for echo cancellation, so the talker's own voice
 * coming back through the speaker is mostly gone before a frame reaches here.
 * What is left is handled by wanting a RUN of loud frames rather than one.
 */

/** How loud, and for how long, before the caller counts as speaking. */
export interface SpeechGateSettings {
  /** Root mean square above which a frame counts as speech. */
  threshold: number;
  /** Consecutive loud frames needed before the gate opens. */
  framesToOpen: number;
  /** Consecutive quiet frames needed before it shuts again. */
  framesToClose: number;
}

/**
 * The defaults, in frames of {@link CAPTURE_FRAME_SAMPLES}.
 *
 * Three frames is 120 ms of continuous speech. Short enough to feel immediate,
 * and long enough that a cough, a door or one keyboard tap does not cut the
 * talker off mid-word.
 *
 * Eight is 320 ms of quiet, which is longer than the gap between two words and
 * shorter than the pause that ends a sentence. Without that hangover the gate
 * would flap on every syllable, and each flap is a state change the whole app
 * reads.
 */
export const SPEECH_GATE_DEFAULTS: SpeechGateSettings = {
  threshold: 0.03,
  framesToOpen: 3,
  framesToClose: 8,
};

/** What the gate remembers between frames. */
export interface SpeechGateState {
  /** True while the caller is taken to be speaking. */
  open: boolean;
  /** Consecutive frames arguing for the other answer. */
  run: number;
}

export const SPEECH_GATE_SHUT: SpeechGateState = { open: false, run: 0 };

/** The loudness of one captured frame, as root mean square. */
export function frameEnergy(samples: Float32Array): number {
  if (samples.length === 0) return 0;
  let sum = 0;
  for (let i = 0; i < samples.length; i++) sum += samples[i] * samples[i];
  return Math.sqrt(sum / samples.length);
}

/**
 * Advance the gate by one captured frame.
 *
 * The same state object comes back when nothing moved. So a caller can test for
 * an edge by identity, and pay nothing on the frames between.
 */
export function stepSpeechGate(
  state: SpeechGateState,
  rms: number,
  settings: SpeechGateSettings = SPEECH_GATE_DEFAULTS,
): SpeechGateState {
  const loud = rms >= settings.threshold;
  if (loud === state.open) return state.run === 0 ? state : { open: state.open, run: 0 };
  const run = state.run + 1;
  const needed = state.open ? settings.framesToClose : settings.framesToOpen;
  if (run < needed) return { open: state.open, run };
  return { open: !state.open, run: 0 };
}
