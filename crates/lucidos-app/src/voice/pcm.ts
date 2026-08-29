/**
 * The one audio format a call speaks, and the two conversions it needs.
 *
 * 24 kHz mono `pcm_s16le`, which is what `voice::provider::AudioFormat` names
 * and what the engine repeats in the opening `session_started` frame. There is
 * no negotiation: the client asks the browser for exactly this rate, so nothing
 * resamples anywhere.
 *
 * Web Audio speaks Float32 in [-1, 1] and the socket speaks Int16, so every
 * frame crosses here in one direction or the other. Pure, and therefore tested.
 */

/** Samples per second, both directions. */
export const SAMPLE_RATE_HZ = 24_000;

/** Channels, both directions. */
export const CHANNELS = 1;

/**
 * Samples in one captured frame, which is 40 ms at {@link SAMPLE_RATE_HZ}.
 *
 * A trade between latency and frame rate. Web Audio hands a worklet 128 samples
 * at a time, so this is 7.5 quanta gathered into one socket frame, or 25 frames
 * a second. Sending each quantum instead would be 187 frames a second for no
 * gain a listener can hear.
 */
export const CAPTURE_FRAME_SAMPLES = 960;

/** Bytes in one sample. */
const BYTES_PER_SAMPLE = 2;

/** Little-endian, which is what the `pcm_s16le` in the frame's `encoding`
 *  means. Written out rather than assumed, so a big-endian host would still
 *  send what the engine reads. */
const LITTLE_ENDIAN = true;

/**
 * Float32 samples to the bytes the socket carries.
 *
 * Out-of-range input is clamped rather than allowed to wrap. A sample above 1
 * is a loud microphone, and wrapping turns that into a click at full scale.
 */
export function floatToPcm16(samples: Float32Array): ArrayBuffer {
  const buffer = new ArrayBuffer(samples.length * BYTES_PER_SAMPLE);
  const view = new DataView(buffer);
  for (let i = 0; i < samples.length; i++) {
    const clamped = Math.max(-1, Math.min(1, samples[i]));
    const scaled = clamped >= 0 ? clamped * 32767 : clamped * 32768;
    view.setInt16(i * BYTES_PER_SAMPLE, Math.round(scaled), LITTLE_ENDIAN);
  }
  return buffer;
}

/**
 * The bytes the socket carried back to Float32 samples.
 *
 * A trailing odd byte is dropped. It cannot be half a sample, and a call is not
 * worth killing over one.
 */
export function pcm16ToFloat(bytes: ArrayBuffer): Float32Array<ArrayBuffer> {
  const count = Math.floor(bytes.byteLength / BYTES_PER_SAMPLE);
  const view = new DataView(bytes);
  const samples = new Float32Array(count);
  for (let i = 0; i < count; i++) {
    const value = view.getInt16(i * BYTES_PER_SAMPLE, LITTLE_ENDIAN);
    samples[i] = value >= 0 ? value / 32767 : value / 32768;
  }
  return samples;
}

/** How long a chunk of talker audio plays for, in seconds. */
export function pcm16DurationSeconds(byteLength: number): number {
  return Math.floor(byteLength / BYTES_PER_SAMPLE) / SAMPLE_RATE_HZ;
}
