/**
 * Deciding that the caller spoke over the talker, from measured energy alone.
 *
 * The engine reads an interruption from the provider's own finished response,
 * never from the caller starting to speak, and says so in
 * `voice/realtime.rs::done_events`. Detecting a barge-in is therefore client
 * work, and this is it.
 *
 * `getUserMedia` is asked for echo cancellation, so the talker's own voice
 * coming back through the speaker is mostly gone before a frame reaches here.
 * What is left is handled by wanting a RUN of loud frames rather than one.
 */

/** How loud, and for how long, before the caller has interrupted. */
export interface BargeInSettings {
  /** Root mean square above which a frame counts as speech. */
  threshold: number;
  /** Consecutive loud frames needed before the gate fires. */
  framesToFire: number;
}

/**
 * The defaults, in frames of {@link CAPTURE_FRAME_SAMPLES}.
 *
 * Three frames is 120 ms of continuous speech. Short enough to feel immediate,
 * and long enough that a cough, a door or one keyboard tap does not cut the
 * talker off mid-word.
 */
export const BARGE_IN_DEFAULTS: BargeInSettings = {
  threshold: 0.03,
  framesToFire: 3,
};

/** What the gate remembers between frames. */
export interface BargeInState {
  loudFrames: number;
  /** True once this talker turn has been interrupted, so it happens once. */
  fired: boolean;
}

export const BARGE_IN_IDLE: BargeInState = { loudFrames: 0, fired: false };

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
 * The gate is armed only while the talker is speaking, and disarms the moment
 * it stops. So the first word of a call never reads as an interruption, which
 * is the same trap the engine avoids on its side.
 */
export function stepBargeIn(
  state: BargeInState,
  input: { rms: number; talkerSpeaking: boolean },
  settings: BargeInSettings = BARGE_IN_DEFAULTS,
): { state: BargeInState; fire: boolean } {
  if (!input.talkerSpeaking) return { state: BARGE_IN_IDLE, fire: false };
  if (state.fired) return { state, fire: false };
  if (input.rms < settings.threshold) {
    return { state: { loudFrames: 0, fired: false }, fire: false };
  }
  const loudFrames = state.loudFrames + 1;
  if (loudFrames < settings.framesToFire) {
    return { state: { loudFrames, fired: false }, fire: false };
  }
  return { state: { loudFrames, fired: true }, fire: true };
}
