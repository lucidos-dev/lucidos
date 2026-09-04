/**
 * A whole call driven through fake devices.
 *
 * The point is the terminal paths: every way a call can end must release the
 * microphone exactly once. A held `MediaStream` keeps the recording indicator
 * lit long after the call is gone. No test with a real `AudioContext` could run
 * here at all.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SPEECH_GATE_DEFAULTS } from './speechGate';
import { type CallRunner, createCallRunner } from './call';
import { LANDING_BOUND_MS, WORDS_BOUND_MS, type CallState } from './callState';
import { CAPTURE_FRAME_SAMPLES, floatToPcm16 } from './pcm';
import type { AudioDevice, CallPorts, SocketHandlers } from './ports';
import {
  CALL_REFUSED,
  CallSetupError,
  UNEXPECTED_SETUP_FAILURE,
  NO_ROUTE_FOR_A_CALL,
} from './refusals';

const THREAD = 'thread-1';

interface Harness {
  runner: CallRunner;
  ports: CallPorts;
  states: CallState[];
  problems: string[];
  last(): CallState;
  /** Devices handed out, newest last. */
  devices: FakeDevice[];
  sockets: FakeSocket[];
  socket(): FakeSocket;
  device(): FakeDevice;
  /** The device id each `openAudio` was asked for, newest last. */
  askedFor: (string | null)[];
  /** Push captured microphone frames at the runner. */
  capture(rms: number, frames?: number): void;
  /** Speak loudly enough to open the gate, for a whole run by default. */
  speak(frames?: number): void;
  /** Go quiet for long enough to shut it, for a whole run by default. */
  hush(frames?: number): void;
  /** How many times a woken audio device was handed back untaken. */
  releases(): number;
  /** Resolve a deferred `openAudio`, when one was armed. */
  releaseAudio(): Promise<void>;
  armSlowAudio(): void;
  armAudioFailure(error: unknown): void;
  armSocketFailure(error: unknown): void;
  /** Make the echo report that no hop carries an upgrade. */
  armBlockedUpgrade(): void;
  /** Make the probe itself fail, so it answers nothing either way. */
  armProbeFailure(error: unknown): void;
  /** Let a pending refusal probe answer. */
  settleProbe(): Promise<void>;
}

class FakeDevice implements AudioDevice {
  played: ArrayBuffer[] = [];
  stops = 0;
  closes = 0;
  closeError: unknown = null;
  note: string | null = null;

  play(pcm: ArrayBuffer): void {
    this.played.push(pcm);
  }

  stopPlayback(): void {
    this.stops++;
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

  /** What the engine says, ignored once the runner has closed us. */
  say(frame: object): void {
    if (!this.forgotten) this.handlers.onText(JSON.stringify(frame));
  }

  controls(): string[] {
    return this.texts.map((t) => JSON.parse(t).type as string);
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
  let releases = 0;
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
      device.note = deviceNote;
      devices.push(device);
      return device;
    },
    openSocket: (threadId, handlers) => {
      if (socketFailure !== null) throw socketFailure;
      const socket = new FakeSocket(threadId, handlers);
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
    armSocketFailure(error) {
      socketFailure = error;
    },
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

/** Press, let the microphone arrive, shake hands, and take the call live. */
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
  it('opens the microphone before the socket', async () => {
    const h = harness();
    h.runner.press(THREAD);
    expect(h.sockets).toHaveLength(0);
    await Promise.resolve();
    await Promise.resolve();
    expect(h.devices).toHaveLength(1);
    expect(h.socket().threadId).toBe(THREAD);
  });

  it('is listening once the engine says the call is up', async () => {
    const h = await liveCall();
    expect(h.last().phase).toBe('listening');
  });
});

describe('which microphone a call opens', () => {
  /** A workspace that never picked one asks for nothing in particular, which
   *  is how every call worked before the picker existed. */
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

  /** The call still goes up, so the note is the only way anybody learns they
   *  are not on the microphone they chose. */
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
    h.speak();
    expect(h.socket().controls()).toEqual(['barge_in']);
    expect(h.device().stops).toBeGreaterThan(0);
    expect(h.last().phase).toBe('listening');
  });

  it('stays quiet while the talker is not speaking', async () => {
    const h = await liveCall();
    h.speak(SPEECH_GATE_DEFAULTS.framesToOpen * 3);
    expect(h.socket().controls()).toEqual([]);
  });

  it('stays quiet when the caller is quiet', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'one moment' });
    h.hush(SPEECH_GATE_DEFAULTS.framesToOpen * 3);
    expect(h.socket().controls()).toEqual([]);
  });

  /** Stopping playback must not shut the gate. The caller is mid-word, and the
   *  rest of that word is one utterance, not a second one. */
  it('leaves the gate open across the cut it makes', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'one moment' });
    h.speak();
    h.speak(SPEECH_GATE_DEFAULTS.framesToOpen);
    expect(h.last().utteranceCount).toBe(1);
  });

  /** The caller never stopped, so the gate was already open when the talker
   *  started. Only an EDGE reaches the reducer. Without measuring afresh at
   *  the flip there is no edge left to make. The caller could then not cut in
   *  without first shutting up for a third of a second. */
  it('cuts in for a caller who never stopped talking', async () => {
    const h = await liveCall();
    h.speak();
    expect(h.last().utterance).toBe('live');
    h.socket().say({ type: 'talker_transcript', text: 'here is the answer' });
    h.speak(SPEECH_GATE_DEFAULTS.framesToOpen);
    expect(h.socket().controls()).toEqual(['barge_in']);
    expect(h.last().phase).toBe('listening');
  });

  /** Every delta of the reply arrives as one of these. Measuring afresh on
   *  each would shut the gate under a caller who IS cutting in. */
  it('measures afresh only on the first delta of a reply', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'here ' });
    h.speak(SPEECH_GATE_DEFAULTS.framesToOpen - 1);
    h.socket().say({ type: 'talker_transcript', text: 'is the answer' });
    h.speak(1);
    expect(h.socket().controls()).toEqual(['barge_in']);
  });
});

/**
 * The caller's own voice, as the transcript reads it.
 *
 * The row it draws is the only sign a caller has that anything heard them
 * start. So what matters is that it appears on speech, and never outlives it.
 */
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

  /** Every delta of the talker's reply is an input. A bound re-armed by each
   *  would outlive the answer it is waiting behind. */
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

/**
 * The talker's tail.
 *
 * `talker_turn_ended` says the provider finished generating, and the speaker
 * can still be playing what it generated. So the floor flips to the caller with
 * audio in the air. Echo cancellation and the run-of-frames rule are what cover
 * it. That is the same pair keeping such audio from firing a barge-in while the
 * talker holds the floor.
 */
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

  /** The caller cut in, and the engine reports the turn over a moment later.
   *  They are still mid-sentence, so that is one utterance and one row. */
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
    // No device opened, so teardown has nothing to close. The audio context the
    // press woke goes back here or it runs for the rest of the page's life.
    expect(h.releases()).toBe(1);
  });

  it('a refused handshake, which the browser will not explain', async () => {
    const h = harness();
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    h.socket().handlers.onClose();
    // The microphone goes back at once, without waiting on the probe below.
    expect(h.device().closes).toBe(1);
    expect(h.last().phase).toBe('idle');
  });

  it('a socket the browser refuses to build at all', async () => {
    const h = harness();
    h.armSocketFailure(new SyntaxError('unsupported URL'));
    h.runner.press(THREAD);
    await Promise.resolve();
    await Promise.resolve();
    // The microphone was already open at the throw. The whole point is that it
    // is released, rather than left listening on a call that never began.
    expect(h.device().closes).toBe(1);
    expect(h.last().phase).toBe('idle');
    expect(h.last().note).toBe(UNEXPECTED_SETUP_FAILURE);
  });

  it('closes only once, however many endings arrive', async () => {
    const h = await liveCall();
    h.runner.press(THREAD);
    h.socket().say({ type: 'session_ended', reason: 'hangup' });
    h.socket().handlers.onClose();
    expect(h.device().closes).toBe(1);
  });
});

/**
 * The cause of a refused handshake is measured, never guessed.
 *
 * Guessing shipped once and misdiagnosed the first real call anybody placed:
 * the message named a busy thread and an unreachable engine, and the truth was
 * a gateway too old to carry the upgrade at all.
 */
describe('why a handshake was refused', () => {
  /** Press, let the microphone arrive, then have the socket close unopened. */
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

  /** No evidence about the hops means the engine-side reason stands. Blaming
   *  the route on a probe that never ran is the worse wrong answer. */
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

  /** A reason for a call the reader has already replaced is noise. */
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
    h.runner.press(THREAD); // rings off while still connecting
    expect(h.last().phase).toBe('idle');
    await h.releaseAudio();
    expect(h.devices).toHaveLength(1);
    expect(h.device().closes).toBe(1);
    expect(h.sockets).toHaveLength(0);
    // Closing the device it opened IS handing the context back, so nothing
    // hands it back twice.
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
});

describe('the frames a call sends', () => {
  it('are only the two the engine will read', async () => {
    const h = await liveCall();
    h.socket().say({ type: 'talker_transcript', text: 'hm' });
    h.speak();
    h.runner.press(THREAD);
    expect(new Set(h.socket().controls())).toEqual(new Set(['barge_in', 'hang_up']));
  });
});
