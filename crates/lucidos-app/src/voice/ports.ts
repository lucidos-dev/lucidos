/**
 * The two devices a call needs, behind one interface each.
 *
 * Everything in this file talks to `AudioContext`, `getUserMedia` and
 * `WebSocket`, and nothing else in the voice client does. The runner takes
 * these as ports. So its rules are driven by fakes in a test, rather than by an
 * audio device that does not exist under Vitest.
 */
import { CAPTURE_WORKLET_NAME, captureWorkletUrl } from './captureWorklet';
import { CHANNELS, SAMPLE_RATE_HZ, pcm16DurationSeconds, pcm16ToFloat } from './pcm';
import { CallSetupError, NO_MICROPHONE_API, NO_WEB_AUDIO, microphoneRefusal } from './refusals';
import { scheduleChunk } from './schedule';
import { voiceSocketUrl, wsEchoUrl } from './socketUrl';

/** The caller's end of the socket, once it is open. */
export interface CallSocket {
  sendText(text: string): void;
  sendAudio(pcm: ArrayBuffer): void;
  close(): void;
}

/** What the runner wants to hear about the socket. */
export interface SocketHandlers {
  /** The handshake succeeded. Anything before this is a refusal. */
  onOpen(): void;
  onText(text: string): void;
  onAudio(pcm: ArrayBuffer): void;
  onClose(): void;
}

/** The microphone and the speaker, as one open device. */
export interface AudioDevice {
  play(pcm: ArrayBuffer): void;
  stopPlayback(): void;
  close(): Promise<void>;
}

export interface CallPorts {
  /**
   * Open or wake the audio context, synchronously.
   *
   * Called from inside the press, before anything is awaited. That is what
   * unlocks audio on iOS, where a context created after an await belongs to no
   * gesture and stays suspended.
   */
  prime(): void;
  /**
   * Close a primed context no call ever took.
   *
   * The other half of {@link prime}. A press that never reaches a call still
   * opened an audio context, and one left running holds the audio hardware
   * awake for nothing. A context a call IS using is left alone.
   */
  release(): void;
  openAudio(onFrame: (samples: Float32Array) => void): Promise<AudioDevice>;
  openSocket(threadId: string, handlers: SocketHandlers): CallSocket;
  /**
   * Did an upgrade survive every hop between here and the engine?
   *
   * Asked only after a call was refused. `true` is the load-bearing answer:
   * the engine is up and the route carries a call, so the refusal was the
   * engine's own. `false` covers refused, unreachable and no answer in time,
   * which the browser does not tell apart.
   */
  probeUpgrade(): Promise<boolean>;
}

type AudioContextCtor = new (options?: AudioContextOptions) => AudioContext;

function audioContextCtor(): AudioContextCtor | null {
  if (typeof window === 'undefined') return null;
  const legacy = (window as { webkitAudioContext?: AudioContextCtor }).webkitAudioContext;
  return (window.AudioContext as AudioContextCtor | undefined) ?? legacy ?? null;
}

/**
 * The context the press opened, waiting for the microphone to arrive.
 *
 * One at a time, and dropped when the call that used it closes it. A closed
 * context cannot be reopened, so the next press builds a new one.
 */
let primed: AudioContext | null = null;

/** True once a call adopted the primed context, so `release` leaves it alone. */
let primedTaken = false;

function primeContext(): void {
  const Ctor = audioContextCtor();
  if (!Ctor) return;
  if (!primed || primed.state === 'closed') {
    primed = new Ctor({ sampleRate: SAMPLE_RATE_HZ });
  }
  // Best effort, and deliberately not awaited: this runs inside the press, and
  // awaiting here is what would cost the gesture. A context that stays
  // suspended is caught in `openAudio`, which reports it to the reader.
  void primed.resume().catch(() => undefined);
}

function releaseContext(): void {
  if (primedTaken || !primed) return;
  const context = primed;
  primed = null;
  // Nothing to report. No call ever ran, so a closed context nobody was using
  // is the outcome asked for, whether or not the close itself resolves.
  void context.close().catch(() => undefined);
}

async function openAudio(onFrame: (samples: Float32Array) => void): Promise<AudioDevice> {
  const Ctor = audioContextCtor();
  if (!Ctor) throw new CallSetupError(NO_WEB_AUDIO);
  if (!navigator.mediaDevices?.getUserMedia) throw new CallSetupError(NO_MICROPHONE_API);

  if (!primed || primed.state === 'closed') primed = new Ctor({ sampleRate: SAMPLE_RATE_HZ });
  const context = primed;

  let stream: MediaStream;
  try {
    // No sample rate is asked of the device. The context runs at 24 kHz and
    // resamples the stream itself. Asking hardware for a rate it does not have
    // is an `OverconstrainedError`, and no call at all.
    stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
        channelCount: CHANNELS,
      },
    });
  } catch (err) {
    throw new CallSetupError(microphoneRefusal(err));
  }

  // One try covers every step up to a wired graph. The microphone is open by
  // now, and the stream is the only handle on it. So a throw that escapes
  // leaves the recording indicator lit with nothing left to stop it. Building a
  // node on a context another call closed underneath us is the way in:
  // `createMediaStreamSource` throws on a closed context.
  let source: MediaStreamAudioSourceNode;
  let capture: AudioWorkletNode;
  let silence: GainNode;
  try {
    await context.audioWorklet.addModule(captureWorkletUrl());
    await context.resume();
    source = context.createMediaStreamSource(stream);
    capture = new AudioWorkletNode(context, CAPTURE_WORKLET_NAME);
    capture.port.onmessage = (event: MessageEvent) => onFrame(event.data as Float32Array);
    // The worklet produces nothing, but Safari runs `process` only for a node
    // that reaches the destination. A silent gain is what keeps it running
    // without putting the caller's own voice in their ear.
    silence = context.createGain();
    silence.gain.value = 0;
    source.connect(capture);
    capture.connect(silence);
    silence.connect(context.destination);
  } catch (err) {
    stream.getTracks().forEach((track) => track.stop());
    throw new CallSetupError(microphoneRefusal(err));
  }

  // Taken here and nowhere earlier: a throw above leaves the context untaken,
  // so the press that failed can still hand it back.
  primedTaken = true;
  let cursor = 0;
  const queued = new Set<AudioBufferSourceNode>();

  function stopPlayback(): void {
    for (const node of queued) {
      // A source that already ended throws on `stop`. Nothing to do about it,
      // and nothing to report: the goal is silence and it is already silent.
      try {
        node.stop();
      } catch {
        /* already finished */
      }
      node.disconnect();
    }
    queued.clear();
    cursor = 0;
  }

  return {
    play(pcm: ArrayBuffer): void {
      const samples = pcm16ToFloat(pcm);
      if (samples.length === 0) return;
      const buffer = context.createBuffer(CHANNELS, samples.length, SAMPLE_RATE_HZ);
      buffer.copyToChannel(samples, 0);
      const node = context.createBufferSource();
      node.buffer = buffer;
      node.connect(context.destination);
      const seconds = pcm16DurationSeconds(pcm.byteLength);
      const placed = scheduleChunk(cursor, context.currentTime, seconds);
      cursor = placed.cursor;
      node.onended = () => queued.delete(node);
      queued.add(node);
      node.start(placed.startAt);
    },
    stopPlayback,
    async close(): Promise<void> {
      stopPlayback();
      capture.port.onmessage = null;
      source.disconnect();
      capture.disconnect();
      silence.disconnect();
      stream.getTracks().forEach((track) => track.stop());
      await context.close();
      if (primed === context) primed = null;
      primedTaken = false;
    },
  };
}

function openSocket(threadId: string, handlers: SocketHandlers): CallSocket {
  const socket = new WebSocket(voiceSocketUrl(threadId));
  socket.binaryType = 'arraybuffer';
  socket.onopen = () => handlers.onOpen();
  socket.onmessage = (event: MessageEvent) => {
    if (typeof event.data === 'string') handlers.onText(event.data);
    else handlers.onAudio(event.data as ArrayBuffer);
  };
  // No `onerror` arm. A socket error is always followed by a close, and the
  // event carries nothing a reader could use: the browser hides the response.
  socket.onclose = () => handlers.onClose();

  function forget(): void {
    socket.onopen = null;
    socket.onmessage = null;
    socket.onclose = null;
  }

  return {
    sendText(text: string): void {
      if (socket.readyState === WebSocket.OPEN) socket.send(text);
    },
    sendAudio(pcm: ArrayBuffer): void {
      if (socket.readyState === WebSocket.OPEN) socket.send(pcm);
    },
    close(): void {
      forget();
      socket.close();
    },
  };
}

/**
 * How long the echo gets to answer.
 *
 * Only ever paid on a call that already failed, and only when nothing answers
 * at all. A route that cannot carry an upgrade normally refuses at once.
 */
const UPGRADE_PROBE_MS = 3_000;

function probeUpgrade(): Promise<boolean> {
  return new Promise((resolve) => {
    let socket: WebSocket;
    try {
      socket = new WebSocket(wsEchoUrl());
    } catch {
      // A URL the browser will not dial says nothing about the hops, and the
      // call's own dial would have thrown the same way first.
      resolve(true);
      return;
    }
    let settled = false;
    const finish = (carried: boolean): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.onopen = null;
      socket.onclose = null;
      socket.onerror = null;
      // The echo has served its purpose at `onopen`. Closing a socket still
      // connecting is allowed, and aborts it.
      socket.close();
      resolve(carried);
    };
    const timer = setTimeout(() => finish(false), UPGRADE_PROBE_MS);
    socket.onopen = () => finish(true);
    socket.onclose = () => finish(false);
    socket.onerror = () => finish(false);
  });
}

/** The real devices, for everything but a test. */
export const browserPorts: CallPorts = {
  prime: primeContext,
  release: releaseContext,
  openAudio,
  openSocket,
  probeUpgrade,
};
