/**
 * The one impure file: it drives a call's devices and owns none of its rules.
 *
 * `callState.ts` decides what happens. This carries the decisions out. It holds
 * the socket, the open audio device, the speech gate and the one timer a call
 * runs, and hands every arriving frame back to the reducer.
 *
 * The devices arrive as ports, so a test drives a whole call with fakes and no
 * `AudioContext` anywhere.
 */
import {
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
import { CALL_REFUSED, NO_ROUTE_FOR_A_CALL, setupRefusal } from './refusals';
import { errorDetail } from '../utils/errorDetail';

/**
 * How many captured frames may wait for the socket before the oldest go.
 *
 * The microphone opens before the dial, so a caller who starts talking at once
 * is recorded with nowhere to send it. A captured frame is 40 ms, so 200 of
 * them is eight seconds, at about 384 kB while they are held.
 *
 * **Sized well above the window, because every frame counts, silence too.**
 * ADR 0184 measures one connect at 3.79 s, and the ring fills from the moment
 * the microphone opens rather than from the first word. A bound at the measured
 * window would therefore have dropped that call's opening words, with the
 * bubble already on screen saying nothing about the cut.
 *
 * Bounded at all because a dial that never completes would otherwise grow this
 * for the life of the page.
 */
export const PREROLL_FRAMES_MAX = 200;

/** How long one captured frame lasts, which is the only clock a call reads. */
export const CAPTURE_FRAME_MS = (CAPTURE_FRAME_SAMPLES / SAMPLE_RATE_HZ) * 1_000;

/**
 * How long this utterance may wait before its row is withdrawn, or `null`.
 *
 * `none` has nothing to withdraw and `live` is bounded by the caller, so
 * neither waits on a clock. The other two are both waits, on different things.
 */
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
  /** Wake the audio device. Call this synchronously inside the press. */
  prime(): void;
  /** Hand back an audio device a press woke but no call ever took. */
  release(): void;
  /** The one control was pressed: place a call, or ring off. */
  press(threadId: string): void;
  /** The call's thread is no longer the focused one. */
  leave(): void;
}

export interface CallRunnerOptions {
  ports: CallPorts;
  /** Every state change, for the signal the UI reads. */
  onState(state: CallState): void;
  /** A problem the reader must know about, outside the call's own reporting. */
  onProblem(message: string): void;
  /**
   * The microphone this workspace picked on this device, asked per call.
   *
   * A function rather than a value. The picker writes it between calls, and a
   * value captured at wiring time would be the one from app start. Absent, or
   * `null`, means the system default.
   */
  microphone?: () => string | null;
  speechGate?: SpeechGateSettings;
}

export function createCallRunner(options: CallRunnerOptions): CallRunner {
  let state: CallState = CALL_IDLE;
  let socket: CallSocket | null = null;
  let audio: AudioDevice | null = null;
  let gate: SpeechGateState = SPEECH_GATE_SHUT;
  /**
   * Captured frames since the caller last stopped making speech.
   *
   * The whole of how a barge-in is told from somebody finishing their sentence
   * (`callState.ts`, `BARGE_IN_QUIET_MS`). Counted in FRAMES rather than read
   * off a clock, so it measures the caller's own silence and nothing else.
   *
   * Reset at both edges, so it is the length of ONE quiet stretch. It reads
   * 80 ms long, for the frames the gate spends opening, and 320 ms short, for
   * the hangover it spends shutting. Both are small against a bound of a
   * second, and they pull opposite ways.
   */
  let quietFrames = 0;
  /**
   * The bound running on an utterance whose words have not landed, and which
   * utterance it belongs to.
   *
   * Armed once per state an utterance reaches, never per input. Every delta of
   * the talker's reply is an input. Re-arming on each would push the bound out
   * for as long as the talker kept speaking.
   */
  let held: { count: number; at: CallerUtterance } | null = null;
  let holdTimer: ReturnType<typeof setTimeout> | null = null;
  /**
   * Caller audio captured before the socket was up, oldest first.
   *
   * Emptied by the `flush-audio` effect on `session_started`, and dropped by
   * `teardown`. Capped at {@link PREROLL_FRAMES_MAX}, which is sized above the
   * measured connect window. Holding more only grows the leak on a dial that
   * never lands.
   */
  let preroll: ArrayBuffer[] = [];
  /** True once the handshake succeeded. A close before it is a refusal. */
  let handshook = false;
  /**
   * Which call the devices belong to.
   *
   * Opening a device is asynchronous, and a call can end while it is in
   * flight. Every callback checks its own number, so a microphone that arrives
   * after its call ended is closed instead of left listening.
   */
  let generation = 0;

  function input(next: CallInput): void {
    const step = stepCall(state, next);
    state = step.state;
    options.onState(state);
    holdTheUtterance();
    for (const effect of step.effects) perform(effect);
  }

  /**
   * Keep the bound on the current utterance in step with the reducer.
   *
   * The reducer says which wait an utterance is in, and each wait has its own
   * duration. This is the clock behind that, and nothing else here knows the
   * time.
   */
  function holdTheUtterance(): void {
    const at = state.utterance;
    // Both bounds wait on the PROVIDER, and during `connecting` there is no
    // provider to wait on: the caller's audio is held rather than sent. Running
    // the clock there withdraws the bubble of somebody who spoke into a slow
    // connect, which is the vanishing bubble ADR 0184 argues against.
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
        // The gate is deliberately left alone. It measures the CALLER's voice,
        // and the caller is mid-word: this effect fires on the barge-in they
        // just made. Resetting it here would shut the gate under them and read
        // the rest of the same sentence as a second utterance.
        audio?.stopPlayback();
        return;
      case 'forget-speech':
        // The gate alone. `quietFrames` measures how long the CALLER has held
        // their peace, and a floor flip is nothing they did. Reset here, the
        // first second of every reply would be barge-in proof.
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

  /**
   * Hand one audio device back, and say so when it will not go.
   *
   * Two places release one: a call that ended, and a microphone that arrived
   * after its call did. Both owe the same report, so both come through here. A
   * dropped rejection leaves the recording indicator lit and nobody told.
   */
  function releaseDevice(device: AudioDevice): void {
    void device.close().catch((err: unknown) => {
      options.onProblem(`The microphone could not be released: ${errorDetail(err)}`);
    });
  }

  async function open(threadId: string): Promise<void> {
    const mine = ++generation;
    let device: AudioDevice;
    try {
      device = await options.ports.openAudio(captured, options.microphone?.() ?? null);
    } catch (err) {
      if (mine === generation) {
        // Hand the woken audio context back. `teardown` cannot: it only knows
        // about a device that opened, and here none did. Without this a refused
        // microphone leaves a context running for the rest of the page's life.
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
    // The call is going up on a microphone the reader did not choose, because
    // the one they did choose was gone. Said now rather than never: a caller
    // who thinks they are on a headset will hold it to their mouth.
    if (device.note) options.onProblem(device.note);
    handshook = false;
    // The microphone is already open, so a throw from here MUST be caught.
    // `new WebSocket` throws outright on a URL the browser will not dial. This
    // runs detached from any caller, so an escaping error would leave the call
    // stuck connecting, with the microphone live and nothing said.
    try {
      socket = options.ports.openSocket(threadId, {
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
          // A close before the handshake is a refusal the browser will not
          // explain, so it is measured rather than read as an ordinary hangup.
          if (handshook) {
            input({ kind: 'socket-closed' });
            return;
          }
          input({ kind: 'refused' });
          // Read AFTER the teardown, which has already moved it on. A newer
          // call makes this answer stale, and a stale reason is worse than
          // none.
          void explainRefusal(generation);
        },
      });
    } catch (err) {
      input({ kind: 'failed', message: setupRefusal(err) });
    }
  }

  /**
   * Say why the handshake was refused, once the call is already down.
   *
   * The browser hides the response, so the two causes are told apart by asking
   * the engine's echo whether an upgrade survives the hops at all. Reported
   * through `onProblem`, because the call is already down by the time
   * the answer arrives.
   */
  async function explainRefusal(mine: number): Promise<void> {
    // A probe that cannot run is no evidence about the hops, so the engine-side
    // reason stands. Blaming the route on nothing is the worse wrong answer,
    // and this is the one call whose rejection nobody else would catch.
    const carried = await options.ports.probeUpgrade().catch(() => true);
    if (mine !== generation) return;
    options.onProblem(carried ? CALL_REFUSED : NO_ROUTE_FOR_A_CALL);
  }

  /** One captured frame: send it, and say whether the caller's voice moved.
   *
   *  Only an EDGE reaches the reducer. Twenty-five frames a second arrive, and
   *  what any reader wants from them is the moment speech starts and the moment
   *  it stops. */
  function captured(samples: Float32Array): void {
    if (!hearsTheCaller(state.phase)) return;
    const pcm = floatToPcm16(samples);
    // A frame with nowhere to go is held, never dropped. That is what makes
    // the bubble the gate raises a promise the words behind it can keep.
    if (socket && isLive(state.phase)) {
      socket.sendAudio(pcm);
    } else {
      if (preroll.length === PREROLL_FRAMES_MAX) preroll.shift();
      preroll.push(pcm);
    }
    const wasOpen = gate.open;
    gate = stepSpeechGate(gate, frameEnergy(samples), options.speechGate);
    if (gate.open === wasOpen) {
      if (!gate.open) quietFrames += 1;
      return;
    }
    const quietMs = quietFrames * CAPTURE_FRAME_MS;
    quietFrames = 0;
    input({ kind: 'speech', open: gate.open, quietMs });
  }

  /** Send what the connect window captured, oldest first, and forget it. */
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
    // A dial that never landed holds audio nothing will ever read.
    preroll = [];
    handshook = false;
    socket?.close();
    socket = null;
    const device = audio;
    audio = null;
    if (device) releaseDevice(device);
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
