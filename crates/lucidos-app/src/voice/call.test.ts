import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SPEECH_GATE_DEFAULTS } from './speechGate';
import { observeSourceGeneration } from '../utils/restoreSourceGeneration';
import {
  CAPTURE_FRAME_MS,
  ONSET_RUNS_HELD,
  PREROLL_FRAMES_MAX,
  type CallRunner,
  createCallRunner,
} from './call';
import {
  BARGE_IN_QUIET_MS,
  LANDING_BOUND_MS,
  WORDS_BOUND_MS,
  type CallState,
} from './callState';
import { CAPTURE_FRAME_SAMPLES, floatToPcm16 } from './pcm';
import type { AudioDevice, CallPorts, PlaybackGaps, SocketHandlers } from './ports';
import {
  CALL_REFUSED,
  CallSetupError,
  UNEXPECTED_SETUP_FAILURE,
  NO_ROUTE_FOR_A_CALL,
} from './refusals';

const THREAD = 'thread-1';

const GAVE_UP_THE_FLOOR = Math.ceil(BARGE_IN_QUIET_MS / CAPTURE_FRAME_MS);

interface Harness {
  runner: CallRunner;
  ports: CallPorts;
  states: CallState[];
  problems: string[];
  last(): CallState;
  devices: FakeDevice[];
  sockets: FakeSocket[];
  socket(): FakeSocket;
  device(): FakeDevice;
  askedFor: (string | null)[];
  capture(rms: number, frames?: number): void;
  speak(frames?: number): void;
  hush(frames?: number): void;
  releases(): number;
  releaseAudio(): Promise<void>;
  armSlowAudio(): void;
  armAudioFailure(error: unknown): void;
  armCloseFailure(error: unknown): void;
  armSocketFailure(error: unknown): void;
  armMintFailure(error: unknown): void;
  armSlowMint(): void;
  releaseMint(): Promise<void>;
  minted: string[];
  armBlockedUpgrade(): void;
  armProbeFailure(error: unknown): void;
  settleProbe(): Promise<void>;
}

class FakeDevice implements AudioDevice {
  played: ArrayBuffer[] = [];
  stops = 0;
  closes = 0;
  closeError: unknown = null;
  note: string | null = null;

  playing = false;

  play(pcm: ArrayBuffer): void {
    this.played.push(pcm);
  }

  stopPlayback(): void {
    this.stops++;
  }

  isPlaying(): boolean {
    return this.playing;
  }

  playbackGaps(): PlaybackGaps {
    return { count: 0, seconds: 0 };
  }

  close(): Promise<void> {
    this.closes++;
    return this.closeError ? Promise.reject(this.closeError) : Promise.resolve();
  }
}

class FakeSocket {
  texts: string[] = [];
  audio: ArrayBuffer[] = [];
  closes = 0;
  forgotten = false;

  constructor(
    readonly threadId: string,
    readonly ticket: string,
    readonly handlers: SocketHandlers,
  ) {}

  sendText(text: string): void {
    this.texts.push(text);
  }

  sendAudio(pcm: ArrayBuffer): void {
    this.audio.push(pcm);
  }

  close(): void {
    this.closes++;
    this.forgotten = true;
  }

  say(frame: object): void {
    if (!this.forgotten) this.handlers.onText(JSON.stringify(frame));
  }

  controls(): string[] {
    return this.texts.map((t) => JSON.parse(t).type as string);
  }

  cuts(): string[] {
    return this.controls().filter((type) => type !== 'caller_started_speaking');
  }
}

function harness(opts: { microphone?: string; note?: string } = {}): Harness {
  const states: CallState[] = [];
  const problems: string[] = [];
  const devices: FakeDevice[] = [];
  const sockets: FakeSocket[] = [];
  const askedFor: (string | null)[] = [];
  const deviceNote = opts.note ?? null;
  let onFrame: ((samples: Float32Array) => void) | null = null;
  let pending: (() => void) | null = null;
  let slow = false;
  let failure: unknown = null;
  let socketFailure: unknown = null;
  let mintFailure: unknown = null;
  let slowMint = false;
  let pendingMint: (() => void) | null = null;
  const minted: string[] = [];
  let releases = 0;
  let deviceCloseError: unknown = null;
  let upgradeCarried = true;
  let probeFailure: unknown = null;

  const ports: CallPorts = {
    prime: () => undefined,
    release: () => {
      releases++;
    },
    openAudio: async (frameSink, deviceId) => {
      onFrame = frameSink;
      askedFor.push(deviceId);
      if (slow) await new Promise<void>((resolve) => (pending = resolve));
      if (failure !== null) throw failure;
      const device = new FakeDevice();
      device.closeError = deviceCloseError;
      device.note = deviceNote;
      devices.push(device);
      return device;
    },
    mintCallTicket: async (threadId) => {
      if (slowMint) await new Promise<void>((resolve) => (pendingMint = resolve));
      if (mintFailure !== null) throw mintFailure;
      minted.push(threadId);
      return `ticket-for-${threadId}`;
    },
    openSocket: (threadId, ticket, handlers) => {
      if (socketFailure !== null) throw socketFailure;
      const socket = new FakeSocket(threadId, ticket, handlers);
      sockets.push(socket);
      return socket;
    },
    probeUpgrade: () =>
      probeFailure !== null ? Promise.reject(probeFailure) : Promise.resolve(upgradeCarried),
  };

  const runner = createCallRunner({
    ports,
    microphone: () => opts.microphone ?? null,
    onState: (state) => states.push(state),
    onProblem: (message) => problems.push(message),
  });

  return {
    runner,
    ports,
    states,
    problems,
    devices,
    sockets,
    last: () => states[states.length - 1],
    socket: () => sockets[sockets.length - 1],
    device: () => devices[devices.length - 1],
    askedFor,
    capture(rms, frames = 1) {
      const samples = new Float32Array(CAPTURE_FRAME_SAMPLES).fill(rms);
      for (let i = 0; i < frames; i++) onFrame?.(samples);
    },
    speak(frames = SPEECH_GATE_DEFAULTS.framesToOpen) {
      this.capture(SPEECH_GATE_DEFAULTS.threshold * 2, frames);
    },
    hush(frames = SPEECH_GATE_DEFAULTS.framesToClose) {
      this.capture(SPEECH_GATE_DEFAULTS.threshold / 2, frames);
    },
    releases: () => releases,
    armSlowAudio() {
      slow = true;
    },
    armAudioFailure(error) {
      failure = error;
    },
    armCloseFailure(error) {
      deviceCloseError = error;
    },
    armSocketFailure(error) {
      socketFailure = error;
    },
    armMintFailure(error) {
      mintFailure = error;
    },
    armSlowMint() {
      slowMint = true;
    },
    async releaseMint() {
      pendingMint?.();
      pendingMint = null;
      await Promise.resolve();
      await Promise.resolve();
    },
    minted,
    armBlockedUpgrade() {
      upgradeCarried = false;
    },
    armProbeFailure(error) {
      probeFailure = error;
    },
    async settleProbe() {
      await Promise.resolve();
      await Promise.resolve();
    },
    async releaseAudio() {
      pending?.();
      pending = null;
      await Promise.resolve();
      await Promise.resolve();
    },
  };
}

const STARTED = {
  type: 'session_started',
  audio: { sample_rate_hz: 24_000, channels: 1, encoding: 'pcm_s16le' },
};

async function liveCall(): Promise<Harness> {
  const h = harness();
  h.runner.press(THREAD);
  await Promise.resolve();
  await Promise.resolve();
  h.socket().handlers.onOpen();
  h.socket().say(STARTED);
  return h;
}

describe('placing a call', () => {
  afterEach(() => observeSourceGeneration(null));

  it('does not mint a current ticket for a press held across restore by microphone permission', async () => {
    const h = harness();
    h.armSlowAudio();
    h.runner.press(THREAD);
    observeSourceGeneration('11111111-1111-4111-8111-111111111111');
    await h.releaseAudio();
    expect(h.minted).toEqual([]);
    expect(h.sockets).toEqual([]);
    expect(h.device().closes).toBe(1);
    expect(h.last().note).toContain('Workspace restored');
  });

  it('does not dial a ticket whose mint finished after restore', async () => {
    const h = harness();
    h.armSlowMint();
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    observeSourceGeneration('11111111-1111-4111-8111-111111111111');
    await h.releaseMint();
    expect(h.sockets).toEqual([]);
    expect(h.device().closes).toBe(1);
    expect(h.last().note).toContain('Workspace restored');
  });

  it('opens the microphone before the socket', async () => {
    const h = harness();
    h.runner.press(THREAD);
    expect(h.sockets).toHaveLength(0);
    await Promise.resolve();
    await Promise.resolve();
    expect(h.devices).toHaveLength(1);
    expect(h.socket().threadId).toBe(THREAD);
  });

  it('dials with a ticket minted for this thread, after the microphone opens', async () => {
    const h = harness();
    h.armSlowAudio();
    h.runner.press(THREAD);
    await Promise.resolve();
    expect(h.minted).toHaveLength(0);
    await h.releaseAudio();
    await Promise.resolve();
    expect(h.minted).toEqual([THREAD]);
    expect(h.socket().ticket).toBe(`ticket-for-${THREAD}`);
  });

  it('is listening once the engine says the call is up', async () => {
    const h = await liveCall();
    expect(h.last().phase).toBe('listening');
  });
});

describe('which microphone a call opens', () => {
  it('names none until the reader picks one', async () => {
    const h = harness();
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    expect(h.askedFor).toEqual([null]);
  });

  it('carries the picked device down to the port', async () => {
    const h = harness({ microphone: 'headset' });
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    expect(h.askedFor).toEqual(['headset']);
  });

  it('says out loud when the port had to settle for another device', async () => {
    const h = harness({ microphone: 'headset', note: 'the headset is gone' });
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    expect(h.problems).toEqual(['the headset is gone']);
    expect(h.last().phase).not.toBe('idle');
  });

  it('says nothing when the picked device opened', async () => {
    const h = harness({ microphone: 'headset' });
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    expect(h.problems).toEqual([]);
  });
});

describe('audio in both directions', () => {
  let h: Harness;
  beforeEach(async () => {
    h = await liveCall();
  });

  it('sends captured frames as the PCM the socket names', () => {
    h.capture(0.1);
    expect(h.socket().audio).toHaveLength(1);
    expect(h.socket().audio[0].byteLength).toBe(CAPTURE_FRAME_SAMPLES * 2);
  });

  it('sends nothing before the call is live', () => {
    const cold = harness();
    cold.runner.press(THREAD);
    cold.capture(0.1);
    expect(cold.sockets).toHaveLength(0);
  });

  it('plays what the talker says', () => {
    const pcm = floatToPcm16(new Float32Array(240));
    h.socket().handlers.onAudio(pcm);
    expect(h.device().played).toEqual([pcm]);
  });
});

describe('barge-in', () => {
  it('cuts the talker off after a run of loud frames', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'one moment' });
    h.hush(GAVE_UP_THE_FLOOR);
    h.speak();
    expect(h.socket().cuts()).toEqual(['barge_in']);
    expect(h.device().stops).toBeGreaterThan(0);
    expect(h.last().phase).toBe('listening');
  });

  it('stays quiet while the talker is not speaking', async () => {
    const h = await liveCall();
    h.speak(SPEECH_GATE_DEFAULTS.framesToOpen * 3);
    expect(h.socket().cuts()).toEqual([]);
  });

  it('stays quiet when the caller is quiet', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'one moment' });
    h.hush(SPEECH_GATE_DEFAULTS.framesToOpen * 3);
    expect(h.socket().cuts()).toEqual([]);
  });

  it('leaves the gate open across the cut it makes', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'one moment' });
    h.speak();
    h.speak(SPEECH_GATE_DEFAULTS.framesToOpen);
    expect(h.last().utteranceCount).toBe(1);
  });

  it('stays quiet for a caller who never stopped talking', async () => {
    const h = await liveCall();
    h.speak();
    expect(h.last().utterance).toBe('live');
    h.socket().say({ type: 'talker_transcript', text: 'here is the answer' });
    h.speak(SPEECH_GATE_DEFAULTS.framesToOpen);
    expect(h.socket().cuts()).toEqual([]);
    expect(h.device().stops).toBe(0);
    expect(h.last().phase).toBe('speaking');
  });

  it('stays quiet for a caller drawing breath mid-sentence', async () => {
    const h = await liveCall();
    h.speak();
    h.hush();
    h.socket().say({ type: 'talker_transcript', text: 'Still ' });
    h.speak();
    expect(h.socket().cuts()).toEqual([]);
    expect(h.device().stops).toBe(0);
  });

  it('measures afresh only on the first delta of a reply', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'here ' });
    h.hush(GAVE_UP_THE_FLOOR);
    h.speak(SPEECH_GATE_DEFAULTS.framesToOpen - 1);
    h.socket().say({ type: 'talker_transcript', text: 'is the answer' });
    h.speak(1);
    expect(h.socket().cuts()).toEqual(['barge_in']);
  });

  it('cuts once however many breaths the caller takes over it', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'one moment' });
    h.hush(GAVE_UP_THE_FLOOR);
    h.speak();
    h.hush();
    h.speak();
    expect(h.socket().cuts()).toEqual(['barge_in']);
    expect(h.device().stops).toBe(1);
  });
});

describe('a live utterance', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('opens after the run, and not before it', async () => {
    const h = await liveCall();
    h.speak(SPEECH_GATE_DEFAULTS.framesToOpen - 1);
    expect(h.last().utterance).toBe('none');
    h.speak(1);
    expect(h.last().utterance).toBe('live');
  });

  it('waits on the provider once the caller stops', async () => {
    const h = await liveCall();
    h.speak();
    h.hush();
    expect(h.last().utterance).toBe('landing');
  });

  it('is withdrawn when the words never come', async () => {
    const h = await liveCall();
    h.speak();
    h.hush();
    vi.advanceTimersByTime(LANDING_BOUND_MS);
    expect(h.last().utterance).toBe('none');
  });

  it('is withdrawn on time however much else the call is doing', async () => {
    const h = await liveCall();
    h.speak();
    h.hush();
    vi.advanceTimersByTime(LANDING_BOUND_MS / 2);
    h.socket().say({ type: 'talker_transcript', text: 'one ' });
    h.socket().say({ type: 'talker_transcript', text: 'moment' });
    vi.advanceTimersByTime(LANDING_BOUND_MS / 2);
    expect(h.last().utterance).toBe('none');
  });

  it('gets the longer bound once the engine has the words', async () => {
    const h = await liveCall();
    h.speak();
    h.hush();
    h.socket().say({ type: 'user_turn_ended', transcript: 'what is on today' });
    vi.advanceTimersByTime(LANDING_BOUND_MS);
    expect(h.last().utterance).toBe('transcribed');
    vi.advanceTimersByTime(WORDS_BOUND_MS);
    expect(h.last().utterance).toBe('none');
  });

  it('runs no clock while the caller is still talking', async () => {
    const h = await liveCall();
    h.speak();
    vi.advanceTimersByTime(LANDING_BOUND_MS + WORDS_BOUND_MS);
    expect(h.last().utterance).toBe('live');
  });

  it('leaves nothing running once the call is gone', async () => {
    const h = await liveCall();
    h.speak();
    h.hush();
    h.socket().handlers.onClose();
    expect(h.last().utterance).toBe('none');
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe('audio still leaving the speaker as the floor flips', () => {
  it('draws no utterance from echo the gate never hears', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'here is the answer' });
    h.hush(20);
    h.socket().say({ type: 'talker_turn_ended' });
    h.hush(20);
    expect(h.last().utterance).toBe('none');
    expect(h.socket().controls()).toEqual([]);
  });

  it('carries no part-built run across the flip', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'here is the answer' });
    h.speak(SPEECH_GATE_DEFAULTS.framesToOpen - 1);
    h.socket().say({ type: 'talker_turn_ended' });
    h.speak(1);
    expect(h.last().utterance).toBe('none');
    h.speak(SPEECH_GATE_DEFAULTS.framesToOpen - 1);
    expect(h.last().utterance).toBe('live');
  });

  it('keeps an open gate open, so a barge-in stays one utterance', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'here is the answer' });
    h.speak();
    h.socket().say({ type: 'talker_turn_ended' });
    h.speak(SPEECH_GATE_DEFAULTS.framesToOpen);
    expect(h.last().utteranceCount).toBe(1);
    expect(h.last().utterance).toBe('live');
  });
});

describe('audio sent over the talker', () => {
  const { threshold, framesToOpen } = SPEECH_GATE_DEFAULTS;
  const loud = threshold * 2;
  const quiet = (step: number) => threshold * step * 0.01;
  const firstSample = (pcm: ArrayBuffer) => new DataView(pcm).getInt16(0, true);
  const asSent = (rms: number) => firstSample(floatToPcm16(new Float32Array(1).fill(rms)));
  const onsetFramesMax = framesToOpen * ONSET_RUNS_HELD;

  async function talkerSpeaking(): Promise<Harness> {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'here is the answer' });
    expect(h.last().phase).toBe('speaking');
    return h;
  }

  it('sends no echo the gate calls quiet', async () => {
    const h = await talkerSpeaking();
    h.hush(GAVE_UP_THE_FLOOR);
    expect(h.socket().audio).toHaveLength(0);
  });

  it('sends the held onset first, oldest first, when the caller cuts in', async () => {
    const h = await talkerSpeaking();
    h.hush(GAVE_UP_THE_FLOOR);
    const steps = Array.from({ length: onsetFramesMax + 5 }, (_, i) => i + 1);
    for (const step of steps) h.capture(quiet(step));
    h.speak(framesToOpen + 1);
    expect(h.socket().cuts()).toEqual(['barge_in']);

    const quietKept = steps.slice(-(onsetFramesMax - (framesToOpen - 1)));
    expect(h.socket().audio.map(firstSample)).toEqual([
      ...quietKept.map((step) => asSent(quiet(step))),
      ...Array<number>(framesToOpen + 1).fill(asSent(loud)),
    ]);
  });

  it('drops the held echo when the talker finishes uncut', async () => {
    const h = await talkerSpeaking();
    h.hush(5);
    h.socket().say({ type: 'talker_turn_ended' });
    h.hush(1);
    expect(h.socket().audio).toHaveLength(1);
  });

  it('holds echo until the reply has finished playing', async () => {
    const h = await talkerSpeaking();
    h.device().playing = true;
    h.socket().say({ type: 'talker_turn_ended' });
    expect(h.last().phase).toBe('listening');
    h.hush(GAVE_UP_THE_FLOOR);
    expect(h.socket().audio).toHaveLength(0);

    h.device().playing = false;
    h.hush(1);
    expect(h.socket().audio).toHaveLength(1);
  });
});

describe('every way a call ends releases the microphone', () => {
  it('a second press, then the engine confirming', async () => {
    const h = await liveCall();
    h.runner.press(THREAD);
    expect(h.socket().controls()).toEqual(['hang_up']);
    h.socket().say({ type: 'session_ended', reason: 'hangup' });
    expect(h.last().phase).toBe('idle');
    expect(h.device().closes).toBe(1);
    expect(h.socket().closes).toBe(1);
  });

  it('leaving the thread', async () => {
    const h = await liveCall();
    h.runner.leave();
    expect(h.socket().controls()).toEqual(['hang_up']);
    h.socket().handlers.onClose();
    expect(h.device().closes).toBe(1);
  });

  it('the socket dying with no goodbye', async () => {
    const h = await liveCall();
    h.socket().handlers.onClose();
    expect(h.last().phase).toBe('idle');
    expect(h.device().closes).toBe(1);
  });

  it('the engine ending the session on its own', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'session_ended', reason: 'provider_failed' });
    expect(h.device().closes).toBe(1);
  });

  it('a refused microphone, with no socket ever opened', async () => {
    const h = harness();
    h.armAudioFailure(new CallSetupError('No microphone was found on this device.'));
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    expect(h.sockets).toHaveLength(0);
    expect(h.last().phase).toBe('idle');
    expect(h.last().note).toBe('No microphone was found on this device.');
    expect(h.releases()).toBe(1);
  });

  it('a refused handshake, which the browser will not explain', async () => {
    const h = harness();
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    h.socket().handlers.onClose();
    expect(h.device().closes).toBe(1);
    expect(h.last().phase).toBe('idle');
  });

  it('a socket the browser refuses to build at all', async () => {
    const h = harness();
    h.armSocketFailure(new SyntaxError('unsupported URL'));
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    expect(h.device().closes).toBe(1);
    expect(h.last().phase).toBe('idle');
    expect(h.last().note).toBe(UNEXPECTED_SETUP_FAILURE);
  });

  it('a refused mint shows the engine\'s reason and releases the microphone', async () => {
    const h = harness();
    const reason = 'Only one of your own devices can place a call.';
    h.armMintFailure(new CallSetupError(reason));
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(h.last().phase).toBe('idle');
    expect(h.last().note).toBe(reason);
    expect(h.last().note).not.toBe(CALL_REFUSED);
    expect(h.device().closes).toBe(1);
    expect(h.sockets).toHaveLength(0);
  });

  it('a call hung up while its ticket mints never dials', async () => {
    const h = harness();
    h.armSlowMint();
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    h.runner.press(THREAD);
    expect(h.device().closes).toBe(1);
    await h.releaseMint();
    expect(h.sockets).toHaveLength(0);
    expect(h.device().closes).toBe(1);
  });

  it('closes only once, however many endings arrive', async () => {
    const h = await liveCall();
    h.runner.press(THREAD);
    h.socket().say({ type: 'session_ended', reason: 'hangup' });
    h.socket().handlers.onClose();
    expect(h.device().closes).toBe(1);
  });
});

describe('why a handshake was refused', () => {
  async function refused(block: boolean): Promise<Harness> {
    const h = harness();
    if (block) h.armBlockedUpgrade();
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    h.socket().handlers.onClose();
    await h.settleProbe();
    return h;
  }

  it('blames the engine when an upgrade does reach it', async () => {
    const h = await refused(false);
    expect(h.problems).toEqual([CALL_REFUSED]);
  });

  it('blames the route when the echo cannot upgrade either', async () => {
    const h = await refused(true);
    expect(h.problems).toEqual([NO_ROUTE_FOR_A_CALL]);
  });

  it('says nothing at all until the probe has answered', async () => {
    const h = harness();
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    h.socket().handlers.onClose();
    expect(h.problems).toEqual([]);
  });

  it('keeps the engine-side reason when the probe itself cannot run', async () => {
    const h = harness();
    h.armProbeFailure(new Error('no socket here'));
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    h.socket().handlers.onClose();
    await h.settleProbe();
    expect(h.problems).toEqual([CALL_REFUSED]);
  });

  it('drops an answer that a newer call has overtaken', async () => {
    const h = harness();
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    h.socket().handlers.onClose();
    h.runner.press(THREAD);
    await h.settleProbe();
    expect(h.problems).toEqual([]);
  });
});

describe('a refusal that is not plain English', () => {
  it('is replaced rather than shown as jargon', async () => {
    const h = harness();
    h.armAudioFailure(new TypeError('undefined is not an object'));
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    expect(h.last().note).toBe(UNEXPECTED_SETUP_FAILURE);
  });
});

describe('a microphone that arrives after its call ended', () => {
  it('is closed rather than left listening', async () => {
    const h = harness();
    h.armSlowAudio();
    h.runner.press(THREAD);
    h.runner.press(THREAD);
    expect(h.last().phase).toBe('idle');
    await h.releaseAudio();
    expect(h.devices).toHaveLength(1);
    expect(h.device().closes).toBe(1);
    expect(h.sockets).toHaveLength(0);
    expect(h.releases()).toBe(0);
  });
});

describe('a microphone that will not close', () => {
  it('says so, because the recording indicator stays lit', async () => {
    const h = await liveCall();
    h.device().closeError = new Error('device busy');
    h.socket().handlers.onClose();
    await Promise.resolve();
    await Promise.resolve();
    expect(h.problems.join(' ')).toContain('microphone could not be released');
  });

  it('says so for a microphone that arrived after its call ended', async () => {
    const h = harness();
    h.armSlowAudio();
    h.armCloseFailure(new Error('device busy'));
    h.runner.press(THREAD);
    h.runner.press(THREAD);
    await h.releaseAudio();
    await Promise.resolve();
    await Promise.resolve();
    expect(h.device().closes).toBe(1);
    expect(h.problems.join(' ')).toContain('microphone could not be released');
  });
});

describe('the frames a call sends', () => {
  it('are only the three the engine will read', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'hm' });
    h.hush(GAVE_UP_THE_FLOOR);
    h.speak();
    h.runner.press(THREAD);
    expect(new Set(h.socket().controls())).toEqual(
      new Set(['barge_in', 'caller_started_speaking', 'hang_up']),
    );
  });

  it('tell the engine the caller opened their mouth, cut or no cut', async () => {
    const h = await liveCall();
    h.speak();
    expect(h.socket().controls()).toEqual(['caller_started_speaking']);
    expect(h.device().stops).toBe(0);
  });

  it('tell it about a caller who started before the session was up', async () => {
    const h = harness();
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    h.speak();
    expect(h.socket().controls()).toEqual([]);
    h.socket().say(STARTED);
    expect(h.socket().controls()).toEqual(['caller_started_speaking']);
  });
});

describe('audio captured before the handshake', () => {
  async function connecting(): Promise<Harness> {
    const h = harness();
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    return h;
  }

  it('raises the bubble without waiting for the session', async () => {
    const h = await connecting();
    h.speak();
    expect(h.last().phase).toBe('connecting');
    expect(h.last().utterance).toBe('live');
  });

  it('is held rather than sent, then flushed when the session opens', async () => {
    const h = await connecting();
    h.speak();
    expect(h.socket().audio).toHaveLength(0);

    h.socket().handlers.onOpen();
    h.socket().say(STARTED);
    expect(h.socket().audio).toHaveLength(SPEECH_GATE_DEFAULTS.framesToOpen);
  });

  it('goes up in the order it was spoken, ahead of what follows', async () => {
    const h = await connecting();
    const loud = SPEECH_GATE_DEFAULTS.threshold * 2;
    h.capture(loud, 1);
    h.capture(loud / 2, 1);
    h.socket().handlers.onOpen();
    h.socket().say(STARTED);
    h.capture(loud / 4, 1);

    const sent = h.socket().audio.map(pcm => new DataView(pcm).getInt16(0, true));
    expect(sent).toEqual([...sent].sort((a, b) => b - a));
    expect(sent).toHaveLength(3);
  });

  it('drops the oldest frames past its bound', async () => {
    const h = await connecting();
    h.capture(SPEECH_GATE_DEFAULTS.threshold / 2, PREROLL_FRAMES_MAX + 40);
    h.socket().handlers.onOpen();
    h.socket().say(STARTED);
    expect(h.socket().audio.length).toBe(PREROLL_FRAMES_MAX);
    expect(h.socket().audio.length).toBeGreaterThan(0);
  });

  it('runs no withdrawal clock while the call is still connecting', async () => {
    vi.useFakeTimers();
    try {
      const h = await connecting();
      h.speak();
      h.hush();
      expect(h.last().utterance).toBe('landing');

      vi.advanceTimersByTime(LANDING_BOUND_MS * 3);
      expect(h.last().utterance).toBe('landing');

      h.socket().handlers.onOpen();
      h.socket().say(STARTED);
      vi.advanceTimersByTime(LANDING_BOUND_MS);
      expect(h.last().utterance).toBe('none');
    } finally {
      vi.useRealTimers();
    }
  });

  it('is dropped outright when the call is torn down', async () => {
    const h = await connecting();
    h.capture(SPEECH_GATE_DEFAULTS.threshold / 2, 5);
    h.socket().handlers.onClose();
    await h.settleProbe();

    const second = await (async () => {
      h.runner.press(THREAD);
      await Promise.resolve();
      await Promise.resolve();
      return h.socket();
    })();
    second.handlers.onOpen();
    second.say(STARTED);
    expect(second.audio).toHaveLength(0);
  });
});
