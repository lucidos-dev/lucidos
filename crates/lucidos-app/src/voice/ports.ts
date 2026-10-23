/**
 * The two devices a call needs, behind one interface each.
 *
 * Everything in this file talks to `AudioContext`, `getUserMedia` and
 * `WebSocket`, and nothing else in the voice client does. The runner takes
 * these as ports. So its rules are driven by fakes in a test, rather than by an
 * audio device that does not exist under Vitest.
 */
import { CAPTURE_WORKLET_NAME, captureWorkletUrl } from './captureWorklet';
import { microphoneConstraints, openWithFallback } from './microphone';
import { CHANNELS, SAMPLE_RATE_HZ, pcm16DurationSeconds, pcm16ToFloat } from './pcm';
import {
  CallSetupError,
  NO_MICROPHONE_API,
  NO_WEB_AUDIO,
  microphoneRefusal,
  wrongAudioRate,
} from './refusals';
import { PLAYBACK_START, type Playback, placeChunk, restartPlayback } from './schedule';
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

/** How often the speaker ran dry mid-reply, over one call. */
export interface PlaybackGaps {
  /** How many times the queue emptied with more of a reply still to come. */
  count: number;
  /** How long the speaker was silent across those holes, in seconds. */
  seconds: number;
}

/** The microphone and the speaker, as one open device. */
export interface AudioDevice {
  /** Why this is not the microphone that was asked for, or `null`.
   *
   *  Carried on the device rather than thrown, because the call went up: the
   *  chosen microphone was gone and the default answered instead. The runner
   *  reports it, so nobody spends a call believing they are on a device they
   *  are not. */
  note: string | null;
  play(pcm: ArrayBuffer): void;
  stopPlayback(): void;
  /** What the caller did NOT hear of the talker, measured as it happened.
   *
   *  Read once, as the call comes down. Nothing acts on it: the caller heard
   *  every one of these holes already, and the playback lead has grown to
   *  cover them. It exists so a report of a choppy call carries a number. */
  playbackGaps(): PlaybackGaps;
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
  /**
   * Open the microphone the workspace picked, or the system default.
   *
   * `deviceId` is what the reader chose on this device, and `null` is every
   * call placed before anybody chose. A chosen device that no longer resolves
   * does NOT fail the call: the default answers instead, and the returned
   * device carries the note saying so.
   */
  openAudio(
    onFrame: (samples: Float32Array) => void,
    deviceId: string | null,
  ): Promise<AudioDevice>;
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

/**
 * How long the speaker takes to fall silent when a reply is cut off.
 *
 * Short enough that a barge-in still feels immediate, and long enough that the
 * waveform reaches zero rather than being stepped there. A step of any size is
 * broadband, so the caller hears it as a click whatever they were listening to.
 *
 * Well under the playback lead a fresh start takes, so audio arriving after a
 * cut can never land on top of the fade.
 */
const CUT_FADE_SECONDS = 0.015;

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

/** True once a call CLAIMED the primed context, so `release` leaves it alone
 *  and the next call builds its own. Claimed at the top of `openAudio`, before
 *  anything is awaited, which is what makes the claim exclusive. */
let primedTaken = false;

function primeContext(): void {
  const Ctor = audioContextCtor();
  if (!Ctor) return;
  // A context another call already claimed is not this press's to hand on.
  // Waking a fresh one keeps the gesture, and leaves that call's audio alone.
  if (!primed || primedTaken || primed.state === 'closed') {
    primed = new Ctor({ sampleRate: SAMPLE_RATE_HZ });
    primedTaken = false;
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

async function openAudio(
  onFrame: (samples: Float32Array) => void,
  deviceId: string | null,
): Promise<AudioDevice> {
  const Ctor = audioContextCtor();
  if (!Ctor) throw new CallSetupError(NO_WEB_AUDIO);
  if (!navigator.mediaDevices?.getUserMedia) throw new CallSetupError(NO_MICROPHONE_API);

  // Claim the primed context, or build one this call alone owns. Claimed here,
  // before the first await, so a second call opening at the same time cannot
  // adopt it too. Two calls on one context is how the older one's `close`
  // silences the newer one, which then reports a microphone that was fine.
  let context: AudioContext;
  if (primedTaken) {
    context = new Ctor({ sampleRate: SAMPLE_RATE_HZ });
  } else {
    if (!primed || primed.state === 'closed') primed = new Ctor({ sampleRate: SAMPLE_RATE_HZ });
    context = primed;
    primedTaken = true;
  }

  /** Give up the context this call took, on a setup step that threw.
   *
   *  Ownership decides how. Still holding the primed slot, so hand the claim
   *  back and let the press's own `release` close what it woke. Otherwise the
   *  context is ours alone and nobody else will ever close it. */
  function abandonContext(): void {
    if (primed === context) {
      primedTaken = false;
      return;
    }
    void context.close().catch(() => undefined);
  }

  // Before the microphone, because a call that cannot work must not light a
  // recording indicator. `sampleRate` is fixed at construction, so this is the
  // browser's whole answer to what we asked for.
  if (context.sampleRate !== SAMPLE_RATE_HZ) {
    abandonContext();
    throw new CallSetupError(wrongAudioRate(context.sampleRate, SAMPLE_RATE_HZ));
  }

  let stream: MediaStream;
  let note: string | null;
  try {
    ({ stream, note } = await openWithFallback(deviceId, (id) =>
      navigator.mediaDevices.getUserMedia({ audio: microphoneConstraints(id) }),
    ));
  } catch (err) {
    abandonContext();
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
  let speaker: GainNode;
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
    // Every chunk of the talker plays through this one node, which is what a
    // cut fades. One gain for the call rather than one per chunk. A chunk
    // arrives every few tens of milliseconds, so a node each would be built
    // and dropped on the thread that draws the transcript.
    speaker = context.createGain();
    speaker.connect(context.destination);
  } catch (err) {
    stream.getTracks().forEach((track) => track.stop());
    abandonContext();
    throw new CallSetupError(microphoneRefusal(err));
  }

  let playback: Playback = PLAYBACK_START;
  /** Every chunk of the talker still due to play. */
  const queued = new Set<AudioBufferSourceNode>();

  function stopPlayback(): void {
    const at = context.currentTime;
    const silent = at + CUT_FADE_SECONDS;
    // Faded, never chopped. A bare `stop()` truncates the waveform wherever it
    // happens to be, and the caller hears that step to zero as a click on
    // every barge-in. Cancelling first keeps two cuts in a row from stacking
    // their ramps on one parameter.
    speaker.gain.cancelScheduledValues(at);
    speaker.gain.setValueAtTime(speaker.gain.value, at);
    speaker.gain.linearRampToValueAtTime(0, silent);
    // Back to full the moment the fade lands. Nothing can be scheduled before
    // then: a fresh start takes the playback lead, which is far longer.
    speaker.gain.setValueAtTime(1, silent);
    for (const node of queued) {
      // A source that already ended throws on `stop`. Nothing to do about it,
      // and nothing to report: the goal is silence and it is already silent.
      try {
        node.stop(silent);
      } catch {
        /* already finished */
      }
    }
    // Forgotten here rather than as each one ends. A chunk cut before it ever
    // sounded is promised no `ended` event, so a set waiting on one would
    // keep growing across a call.
    queued.clear();
    playback = restartPlayback(playback);
  }

  return {
    note,
    play(pcm: ArrayBuffer): void {
      const samples = pcm16ToFloat(pcm);
      if (samples.length === 0) return;
      const buffer = context.createBuffer(CHANNELS, samples.length, SAMPLE_RATE_HZ);
      buffer.copyToChannel(samples, 0);
      const node = context.createBufferSource();
      node.buffer = buffer;
      node.connect(speaker);
      const seconds = pcm16DurationSeconds(pcm.byteLength);
      const placed = placeChunk(playback, context.currentTime, seconds);
      playback = placed.playback;
      node.onended = () => {
        queued.delete(node);
        node.disconnect();
      };
      queued.add(node);
      node.start(placed.startAt);
    },
    stopPlayback,
    playbackGaps: () => ({ count: playback.gaps, seconds: playback.silentSeconds }),
    async close(): Promise<void> {
      stopPlayback();
      capture.port.onmessage = null;
      source.disconnect();
      capture.disconnect();
      silence.disconnect();
      speaker.disconnect();
      stream.getTracks().forEach((track) => track.stop());
      await context.close();
      // Only the device still holding the primed context may retire it. A call
      // that built its own must leave both alone, or it hands the NEXT call's
      // context to a press that will close it.
      if (primed === context) {
        primed = null;
        primedTaken = false;
      }
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
