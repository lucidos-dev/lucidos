import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { CallPorts } from '../ports';

vi.mock('../captureWorklet', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  captureWorkletUrl: () => 'blob:capture-worklet',
}));

const NOTHING = (): void => undefined;
const A_LITTLE_AUDIO = new ArrayBuffer(4);

interface GainLog {
  ramps: { to: number; at: number }[];
  holds: { to: number; at: number }[];
  cancels: number;
}

let stops: (number | undefined)[] = [];
let gains: GainLog[] = [];
let context: FakeAudioContext;

function graphNode(): { connect: () => void; disconnect: () => void } {
  return { connect: NOTHING, disconnect: NOTHING };
}

class FakeAudioContext {
  readonly sampleRate = 24_000;
  state: 'suspended' | 'running' | 'closed' = 'suspended';
  currentTime = 0;
  destination = graphNode();
  audioWorklet = { addModule: (): Promise<void> => Promise.resolve() };

  constructor() {
    context = this;
  }

  resume(): Promise<void> {
    this.state = 'running';
    return Promise.resolve();
  }

  close(): Promise<void> {
    this.state = 'closed';
    return Promise.resolve();
  }

  createMediaStreamSource(): unknown {
    return graphNode();
  }

  createGain(): unknown {
    const log: GainLog = { ramps: [], holds: [], cancels: 0 };
    const seen = (): void => {
      if (!gains.includes(log)) gains.push(log);
    };
    return {
      gain: {
        value: 1,
        cancelScheduledValues: (): void => {
          log.cancels++;
          seen();
        },
        setValueAtTime: (to: number, at: number): void => {
          log.holds.push({ to, at });
          seen();
        },
        linearRampToValueAtTime: (to: number, at: number): void => {
          log.ramps.push({ to, at });
          seen();
        },
      },
      ...graphNode(),
    };
  }

  createBuffer(): unknown {
    return { copyToChannel: NOTHING };
  }

  createBufferSource(): unknown {
    const index = stops.length;
    stops.push(undefined);
    return {
      buffer: null,
      onended: null,
      start: NOTHING,
      stop: (at?: number): void => {
        stops[index] = at;
      },
      ...graphNode(),
    };
  }
}

class FakeWorkletNode {
  port: { onmessage: ((event: MessageEvent) => void) | null } = { onmessage: null };
  connect = NOTHING;
  disconnect = NOTHING;
}

function getUserMedia(): Promise<unknown> {
  return Promise.resolve({ getTracks: () => [{ stop: NOTHING }] });
}

describe('cutting a reply off', () => {
  let ports: CallPorts;

  beforeEach(async () => {
    stops = [];
    gains = [];
    vi.stubGlobal('AudioContext', FakeAudioContext);
    vi.stubGlobal('AudioWorkletNode', FakeWorkletNode);
    vi.stubGlobal('navigator', { mediaDevices: { getUserMedia } });
    vi.resetModules();
    ({ browserPorts: ports } = await import('../ports'));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('ramps the speaker down instead of chopping it', async () => {
    const device = await ports.openAudio(NOTHING, null);
    device.play(A_LITTLE_AUDIO);
    device.play(A_LITTLE_AUDIO);
    context.currentTime = 5;

    device.stopPlayback();

    expect(gains).toHaveLength(1);
    expect(gains[0].cancels).toBe(1);
    expect(gains[0].ramps).toEqual([{ to: 0, at: expect.any(Number) }]);
    expect(gains[0].ramps[0].at).toBeGreaterThan(5);
  });

  it('stops every queued chunk at the end of the fade, never before it', async () => {
    const device = await ports.openAudio(NOTHING, null);
    device.play(A_LITTLE_AUDIO);
    device.play(A_LITTLE_AUDIO);
    context.currentTime = 5;

    device.stopPlayback();

    const silent = gains[0].ramps[0].at;
    expect(stops).toEqual([silent, silent]);
  });

  it('puts the speaker back to full as the fade lands', async () => {
    const device = await ports.openAudio(NOTHING, null);
    device.play(A_LITTLE_AUDIO);
    context.currentTime = 5;

    device.stopPlayback();

    const silent = gains[0].ramps[0].at;
    expect(gains[0].holds).toContainEqual({ to: 1, at: silent });
  });

  it('fades well inside the lead the next chunk will take', async () => {
    const { PLAYBACK_LEAD_SECONDS } = await import('../schedule');
    const device = await ports.openAudio(NOTHING, null);
    device.play(A_LITTLE_AUDIO);
    context.currentTime = 5;

    device.stopPlayback();

    expect(gains[0].ramps[0].at - 5).toBeLessThan(PLAYBACK_LEAD_SECONDS);
  });

  it('forgets a cut chunk rather than waiting on an ended it may never get', async () => {
    const device = await ports.openAudio(NOTHING, null);
    device.play(A_LITTLE_AUDIO);
    context.currentTime = 5;
    device.stopPlayback();
    stops = [];

    device.stopPlayback();

    expect(stops).toEqual([]);
  });
});
