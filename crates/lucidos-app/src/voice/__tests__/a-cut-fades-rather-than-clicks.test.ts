/**
 * A reply the caller cuts off stops by fading, never by being chopped.
 *
 * `stop()` on its own truncates the waveform wherever it happens to be. A step
 * to zero is broadband, so the caller hears a click whatever was playing, and
 * they hear one on every barge-in. Somebody who talks over the talker a lot
 * therefore hears a call full of clicks.
 *
 * Driven through fakes, because there is no Web Audio here at all.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { CallPorts } from '../ports';

vi.mock('../captureWorklet', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  captureWorkletUrl: () => 'blob:capture-worklet',
}));

const NOTHING = (): void => undefined;
/** One chunk of talker audio: two samples, which is all the fakes read. */
const A_LITTLE_AUDIO = new ArrayBuffer(4);

/** What a gain node was told to do, in the order it was told. */
interface GainLog {
  ramps: { to: number; at: number }[];
  holds: { to: number; at: number }[];
  cancels: number;
}

/** When each source was told to stop, or undefined while it is still due. */
let stops: (number | undefined)[] = [];
/** Only the gains something automated, which is the speaker and nothing else. */
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
    // The capture graph builds one of these too, for the silent sink. Only a
    // gain something automates ever lands in the log below.
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
    // `primed` is module state, so each case needs its own copy of the module.
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

    // One gain for the whole call, so one ramp covers every queued chunk.
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

    // A bare `stop()` takes no argument, and that is exactly the chop. The
    // stop has to wait for the ramp, or the fade never gets to sound.
    const silent = gains[0].ramps[0].at;
    expect(stops).toEqual([silent, silent]);
  });

  it('puts the speaker back to full as the fade lands', async () => {
    // The stream carries on after a cut, so a speaker left at zero would play
    // the rest of the call silently.
    const device = await ports.openAudio(NOTHING, null);
    device.play(A_LITTLE_AUDIO);
    context.currentTime = 5;

    device.stopPlayback();

    const silent = gains[0].ramps[0].at;
    expect(gains[0].holds).toContainEqual({ to: 1, at: silent });
  });

  it('fades well inside the lead the next chunk will take', async () => {
    // Audio arriving after a cut starts a lead ahead of now. A fade longer
    // than that lead would still be sounding under it.
    const { PLAYBACK_LEAD_SECONDS } = await import('../schedule');
    const device = await ports.openAudio(NOTHING, null);
    device.play(A_LITTLE_AUDIO);
    context.currentTime = 5;

    device.stopPlayback();

    expect(gains[0].ramps[0].at - 5).toBeLessThan(PLAYBACK_LEAD_SECONDS);
  });

  it('forgets a cut chunk rather than waiting on an ended it may never get', async () => {
    // A chunk cut before it ever sounded is promised no `ended` event. Left in
    // the queue it would be re-stopped by every later cut, for the whole call.
    const device = await ports.openAudio(NOTHING, null);
    device.play(A_LITTLE_AUDIO);
    context.currentTime = 5;
    device.stopPlayback();
    stops = [];

    device.stopPlayback();

    expect(stops).toEqual([]);
  });
});
