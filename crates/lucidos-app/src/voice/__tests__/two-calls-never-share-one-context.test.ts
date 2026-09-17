/**
 * One audio context belongs to one call, and to no other.
 *
 * A press wakes a context before the microphone is asked for. So a call that
 * ends while its microphone is still arriving leaves a device nobody wants,
 * and the runner closes it. Let two calls share the context and that close
 * silences the LIVE one: the reader is told the microphone could not be
 * opened, for a microphone that was fine.
 *
 * Driven through fakes, because there is no Web Audio here at all.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SAMPLE_RATE_HZ } from '../pcm';
import type { AudioDevice, CallPorts } from '../ports';

// The worklet is fetched as a blob URL, which needs a `Blob` and a browser
// `URL`. Neither says anything about the rules under test.
vi.mock('../captureWorklet', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  captureWorkletUrl: () => 'blob:capture-worklet',
}));

const NOTHING = (): void => undefined;
const A_LITTLE_AUDIO = new ArrayBuffer(4);

function graphNode(): { connect: () => void; disconnect: () => void } {
  return { connect: NOTHING, disconnect: NOTHING };
}

/** Every context the fakes built, oldest first. */
let contexts: FakeAudioContext[] = [];

class FakeAudioContext {
  readonly sampleRate: number;
  state: 'suspended' | 'running' | 'closed' = 'suspended';
  currentTime = 0;
  destination = graphNode();
  audioWorklet = { addModule: (): Promise<void> => Promise.resolve() };

  constructor(options?: { sampleRate?: number }) {
    this.sampleRate = options?.sampleRate ?? 44_100;
    contexts.push(this);
  }

  /** What every real method does first: refuse once the context is closed. */
  private live(): void {
    if (this.state === 'closed') throw new Error('InvalidStateError: context is closed');
  }

  resume(): Promise<void> {
    this.live();
    this.state = 'running';
    return Promise.resolve();
  }

  close(): Promise<void> {
    this.live();
    this.state = 'closed';
    return Promise.resolve();
  }

  createMediaStreamSource(): unknown {
    this.live();
    return graphNode();
  }

  createGain(): unknown {
    this.live();
    return { gain: { value: 1 }, ...graphNode() };
  }

  createBuffer(): unknown {
    this.live();
    return { copyToChannel: NOTHING };
  }

  createBufferSource(): unknown {
    this.live();
    return { buffer: null, onended: null, start: NOTHING, stop: NOTHING, ...graphNode() };
  }
}

class FakeWorkletNode {
  port: { onmessage: ((event: MessageEvent) => void) | null } = { onmessage: null };
  connect = NOTHING;
  disconnect = NOTHING;
}

/** Microphones asked for and not yet answered, oldest first. */
let mics: { grant: () => void; refuse: (err: unknown) => void }[] = [];

function getUserMedia(): Promise<unknown> {
  return new Promise((resolve, reject) => {
    mics.push({
      grant: () => resolve({ getTracks: () => [{ stop: NOTHING }] }),
      refuse: reject,
    });
  });
}

/** Can this device still put audio out, or was its context closed under it? */
function speaks(device: AudioDevice): boolean {
  try {
    device.play(A_LITTLE_AUDIO);
    return true;
  } catch {
    return false;
  }
}

describe('one context per call', () => {
  let ports: CallPorts;

  beforeEach(async () => {
    contexts = [];
    mics = [];
    vi.stubGlobal('AudioContext', FakeAudioContext);
    vi.stubGlobal('AudioWorkletNode', FakeWorkletNode);
    vi.stubGlobal('navigator', { mediaDevices: { getUserMedia } });
    // `primed` and `primedTaken` are module state, so each case needs its own
    // copy of the module rather than whatever the last one left behind.
    vi.resetModules();
    ({ browserPorts: ports } = await import('../ports'));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('runs the call on the very context the press woke', () => {
    // The ordinary path, and the one iOS needs: the context the gesture opened
    // is the context the call runs on, so nothing extra is built.
    ports.prime();
    void ports.openAudio(NOTHING, null);
    expect(contexts).toHaveLength(1);
    expect(contexts[0].sampleRate).toBe(SAMPLE_RATE_HZ);
  });

  it('a stale call closing its device leaves a live call speaking', async () => {
    // Press, ring off while the microphone is still coming, press again. Both
    // opens are in flight, and the first device to arrive is the stale one.
    ports.prime();
    const stale = ports.openAudio(NOTHING, null);
    const live = ports.openAudio(NOTHING, null);
    expect(mics).toHaveLength(2);

    mics[1].grant();
    const liveDevice = await live;
    mics[0].grant();
    const staleDevice = await stale;

    await staleDevice.close();

    expect(speaks(liveDevice)).toBe(true);
  });

  it('a call on its own context never retires the primed one', async () => {
    // The second press wakes a fresh context, because the first call still
    // holds the one before it. Closing the first must not hand the second
    // call's context to `release`.
    ports.prime();
    const first = ports.openAudio(NOTHING, null);
    mics[0].grant();
    const firstDevice = await first;

    ports.prime();
    const second = ports.openAudio(NOTHING, null);
    mics[1].grant();
    const secondDevice = await second;

    await firstDevice.close();
    ports.release();

    expect(speaks(secondDevice)).toBe(true);
  });

  it('a refused microphone hands the woken context back', async () => {
    // Nothing was taken, so the press that failed can still release what it
    // woke. Claiming the context early must not cost that.
    ports.prime();
    const refused = ports.openAudio(NOTHING, null);
    mics[0].refuse(new Error('NotAllowedError'));
    await expect(refused).rejects.toThrow();

    ports.release();

    expect(contexts).toHaveLength(1);
    expect(contexts[0].state).toBe('closed');
  });
});
