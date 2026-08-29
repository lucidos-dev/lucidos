import { describe, it, expect } from 'vitest';
import {
  CAPTURE_FRAME_SAMPLES,
  CHANNELS,
  SAMPLE_RATE_HZ,
  floatToPcm16,
  pcm16DurationSeconds,
  pcm16ToFloat,
} from './pcm';

/** Read the bytes back as little-endian 16-bit samples. */
function samplesOf(buffer: ArrayBuffer): number[] {
  const view = new DataView(buffer);
  const out: number[] = [];
  for (let i = 0; i < buffer.byteLength; i += 2) out.push(view.getInt16(i, true));
  return out;
}

describe('the format both directions speak', () => {
  it('is the 24 kHz mono the engine names', () => {
    expect(SAMPLE_RATE_HZ).toBe(24_000);
    expect(CHANNELS).toBe(1);
  });

  it('captures 40 ms in one frame', () => {
    expect(CAPTURE_FRAME_SAMPLES / SAMPLE_RATE_HZ).toBeCloseTo(0.04, 10);
  });
});

describe('float to the bytes the socket carries', () => {
  it('writes little-endian, whatever the host is', () => {
    const buffer = floatToPcm16(new Float32Array([1]));
    const bytes = new Uint8Array(buffer);
    expect([...bytes]).toEqual([0xff, 0x7f]);
  });

  it('spans full scale in both directions', () => {
    expect(samplesOf(floatToPcm16(new Float32Array([1, -1, 0])))).toEqual([32767, -32768, 0]);
  });

  it('clamps a loud sample rather than letting it wrap', () => {
    expect(samplesOf(floatToPcm16(new Float32Array([4, -4])))).toEqual([32767, -32768]);
  });

  it('carries two bytes per sample and nothing else', () => {
    expect(floatToPcm16(new Float32Array(CAPTURE_FRAME_SAMPLES)).byteLength).toBe(1920);
  });
});

describe('the bytes back to float', () => {
  it('round-trips the ends of the range exactly', () => {
    const round = pcm16ToFloat(floatToPcm16(new Float32Array([1, -1, 0])));
    expect([...round]).toEqual([1, -1, 0]);
  });

  it('round-trips ordinary speech within a quantisation step', () => {
    const original = new Float32Array([0.5, -0.25, 0.125, -0.9]);
    const round = pcm16ToFloat(floatToPcm16(original));
    for (let i = 0; i < original.length; i++) {
      expect(round[i]).toBeCloseTo(original[i], 4);
    }
  });

  it('drops a trailing odd byte rather than reading half a sample', () => {
    expect(pcm16ToFloat(new ArrayBuffer(5)).length).toBe(2);
  });
});

describe('how long a chunk plays for', () => {
  it('is its samples over the sample rate', () => {
    expect(pcm16DurationSeconds(1920)).toBeCloseTo(0.04, 10);
    expect(pcm16DurationSeconds(0)).toBe(0);
  });
});
