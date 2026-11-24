
export const SAMPLE_RATE_HZ = 24_000;

export const CHANNELS = 1;

export const CAPTURE_FRAME_SAMPLES = 960;

const BYTES_PER_SAMPLE = 2;

const LITTLE_ENDIAN = true;

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

export function pcm16DurationSeconds(byteLength: number): number {
  return Math.floor(byteLength / BYTES_PER_SAMPLE) / SAMPLE_RATE_HZ;
}
