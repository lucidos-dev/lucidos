import {
  SPEECH_GATE_DEFAULTS,
  SPEECH_GATE_SHUT,
  type SpeechGateSettings,
  type SpeechGateState,
  frameEnergy,
  stepSpeechGate,
} from './speechGate';
import {
  CALL_IDLE,
  type CallEffect,
  type CallInput,
  type CallState,
  type CallerUtterance,
  LANDING_BOUND_MS,
  WORDS_BOUND_MS,
  hearsTheCaller,
  isLive,
  stepCall,
} from './callState';
import { parseServerFrame } from './frames';
import { CAPTURE_FRAME_SAMPLES, SAMPLE_RATE_HZ, floatToPcm16 } from './pcm';
import type { AudioDevice, CallPorts, CallSocket } from './ports';
import { CALL_REFUSED, CallSetupError, NO_ROUTE_FOR_A_CALL, setupRefusal } from './refusals';
import { errorDetail } from '../utils/errorDetail';
import { sourceGeneration } from '../utils/restoreSourceGeneration';

export const PREROLL_FRAMES_MAX = 200;

export const ONSET_RUNS_HELD = 3;

export const CAPTURE_FRAME_MS = (CAPTURE_FRAME_SAMPLES / SAMPLE_RATE_HZ) * 1_000;

function utteranceBound(at: CallerUtterance): number | null {
  switch (at) {
    case 'landing':
      return LANDING_BOUND_MS;
    case 'transcribed':
      return WORDS_BOUND_MS;
    case 'none':
    case 'live':
      return null;
  }
}

export interface CallRunner {
  prime(): void;
  release(): void;
  press(threadId: string): void;
  leave(): void;
}

export interface CallRunnerOptions {
  ports: CallPorts;
  onState(state: CallState): void;
  onProblem(message: string): void;
  microphone?: () => string | null;
  speechGate?: SpeechGateSettings;
}

export function createCallRunner(options: CallRunnerOptions): CallRunner {
  let state: CallState = CALL_IDLE;
  let socket: CallSocket | null = null;
  let audio: AudioDevice | null = null;
  let gate: SpeechGateState = SPEECH_GATE_SHUT;
  let quietFrames = 0;
  let held: { count: number; at: CallerUtterance } | null = null;
  let holdTimer: ReturnType<typeof setTimeout> | null = null;
  let preroll: ArrayBuffer[] = [];
  let onset: ArrayBuffer[] = [];
  const gateSettings = options.speechGate ?? SPEECH_GATE_DEFAULTS;
  const onsetFramesMax = gateSettings.framesToOpen * ONSET_RUNS_HELD;
  let handshook = false;
  let generation = 0;

  function input(next: CallInput): void {
    const step = stepCall(state, next);
    state = step.state;
    options.onState(state);
    holdTheUtterance();
    for (const effect of step.effects) perform(effect);
  }

  function holdTheUtterance(): void {
    const at = state.utterance;
    const bound = isLive(state.phase) ? utteranceBound(at) : null;
    if (bound === null) {
      dropTheHold();
      return;
    }
    if (held && held.count === state.utteranceCount && held.at === at) return;
    dropTheHold();
    held = { count: state.utteranceCount, at };
    const mine = generation;
    holdTimer = setTimeout(() => {
      holdTimer = null;
      held = null;
      if (mine === generation) input({ kind: 'utterance-timeout' });
    }, bound);
  }

  function dropTheHold(): void {
    if (holdTimer !== null) clearTimeout(holdTimer);
    holdTimer = null;
    held = null;
  }

  function perform(effect: CallEffect): void {
    switch (effect.kind) {
      case 'open':
        void open(effect.threadId);
        return;
      case 'send':
        socket?.sendText(JSON.stringify(effect.control));
        return;
      case 'stop-playback':
        audio?.stopPlayback();
        return;
      case 'forget-speech':
        gate = SPEECH_GATE_SHUT;
        return;
      case 'flush-audio':
        flushPreroll();
        return;
      case 'teardown':
        teardown();
        return;
    }
  }

  function releaseDevice(device: AudioDevice): void {
    void device.close().catch((err: unknown) => {
      options.onProblem(`The microphone could not be released: ${errorDetail(err)}`);
    });
  }

  function reportPlaybackGaps(device: AudioDevice): void {
    const gaps = device.playbackGaps();
    if (gaps.count === 0) return;
    console.warn(
      `[Voice] The speaker ran dry ${gaps.count} times, silent for ${gaps.seconds.toFixed(2)}s mid-reply.`,
    );
  }

  async function open(threadId: string): Promise<void> {
    const mine = ++generation;
    const pressedGeneration = sourceGeneration();
    const checkRestore = () => {
      if (sourceGeneration() !== pressedGeneration) {
        throw new CallSetupError('Workspace restored; start a new call after reviewing retained work.');
      }
    };
    let device: AudioDevice;
    try {
      device = await options.ports.openAudio(captured, options.microphone?.() ?? null);
    } catch (err) {
      if (mine === generation) {
        options.ports.release();
        input({ kind: 'failed', message: setupRefusal(err) });
      }
      return;
    }
    if (mine !== generation) {
      releaseDevice(device);
      return;
    }
    audio = device;
    if (device.note) options.onProblem(device.note);
    let ticket: string;
    try {
      checkRestore();
      ticket = await options.ports.mintCallTicket(threadId);
      checkRestore();
    } catch (err) {
      if (mine === generation) input({ kind: 'failed', message: setupRefusal(err) });
      return;
    }
    if (mine !== generation) return;
    handshook = false;
    try {
      socket = options.ports.openSocket(threadId, ticket, {
        onOpen: () => {
          if (mine === generation) handshook = true;
        },
        onText: (text) => {
          if (mine !== generation) return;
          const frame = parseServerFrame(text);
          if (frame) input({ kind: 'frame', frame });
        },
        onAudio: (pcm) => {
          if (mine === generation) audio?.play(pcm);
        },
        onClose: () => {
          if (mine !== generation) return;
          if (handshook) {
            input({ kind: 'socket-closed' });
            return;
          }
          input({ kind: 'refused' });
          void explainRefusal(generation);
        },
      });
    } catch (err) {
      input({ kind: 'failed', message: setupRefusal(err) });
    }
  }

  async function explainRefusal(mine: number): Promise<void> {
    const carried = await options.ports.probeUpgrade().catch(() => true);
    if (mine !== generation) return;
    options.onProblem(carried ? CALL_REFUSED : NO_ROUTE_FOR_A_CALL);
  }

  function captured(samples: Float32Array): void {
    if (!hearsTheCaller(state.phase)) return;
    const pcm = floatToPcm16(samples);
    const wasOpen = gate.open;
    gate = stepSpeechGate(gate, frameEnergy(samples), gateSettings);
    route(pcm);
    if (gate.open === wasOpen) {
      if (!gate.open) quietFrames += 1;
      return;
    }
    const quietMs = quietFrames * CAPTURE_FRAME_MS;
    quietFrames = 0;
    input({ kind: 'speech', open: gate.open, quietMs });
  }

  function route(pcm: ArrayBuffer): void {
    if (!socket || !isLive(state.phase)) {
      if (preroll.length === PREROLL_FRAMES_MAX) preroll.shift();
      preroll.push(pcm);
      return;
    }
    const talkerHeard = state.phase === 'speaking' || audio?.isPlaying() === true;
    if (talkerHeard && !gate.open) {
      if (onset.length === onsetFramesMax) onset.shift();
      onset.push(pcm);
      return;
    }
    if (gate.open) for (const held of onset) socket.sendAudio(held);
    onset = [];
    socket.sendAudio(pcm);
  }

  function flushPreroll(): void {
    const waiting = preroll;
    preroll = [];
    for (const pcm of waiting) socket?.sendAudio(pcm);
  }

  function teardown(): void {
    generation++;
    dropTheHold();
    gate = SPEECH_GATE_SHUT;
    quietFrames = 0;
    preroll = [];
    onset = [];
    handshook = false;
    socket?.close();
    socket = null;
    const device = audio;
    audio = null;
    if (device) {
      reportPlaybackGaps(device);
      releaseDevice(device);
    }
  }

  return {
    prime(): void {
      options.ports.prime();
    },
    release(): void {
      options.ports.release();
    },
    press(threadId: string): void {
      if (state.phase === 'idle') options.ports.prime();
      input({ kind: 'toggle', threadId });
    },
    leave(): void {
      input({ kind: 'leave' });
    },
  };
}
