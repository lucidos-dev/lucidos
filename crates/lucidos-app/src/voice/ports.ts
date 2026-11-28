import { ApiError, isTransportError, mintSocketTicket as postSocketTicket } from '../api/client';
import { CAPTURE_WORKLET_NAME, captureWorkletUrl } from './captureWorklet';
import { microphoneConstraints, openWithFallback } from './microphone';
import { CHANNELS, SAMPLE_RATE_HZ, pcm16DurationSeconds, pcm16ToFloat } from './pcm';
import {
  CallSetupError,
  NO_MICROPHONE_API,
  NO_ROUTE_FOR_A_CALL,
  NO_WEB_AUDIO,
  microphoneRefusal,
  wrongAudioRate,
} from './refusals';
import { PLAYBACK_START, type Playback, placeChunk, restartPlayback } from './schedule';
import { voiceSocketUrl } from '../utils/socketUrl';
import { blockedRelayGateway, probeUpgrade, relayBlockedText } from '../utils/socketProbe';

export interface CallSocket {
  sendText(text: string): void;
  sendAudio(pcm: ArrayBuffer): void;
  close(): void;
}

export interface SocketHandlers {
  onOpen(): void;
  onText(text: string): void;
  onAudio(pcm: ArrayBuffer): void;
  onClose(): void;
}

export interface PlaybackGaps {
  count: number;
  seconds: number;
}

export interface AudioDevice {
  note: string | null;
  play(pcm: ArrayBuffer): void;
  stopPlayback(): void;
  isPlaying(): boolean;
  playbackGaps(): PlaybackGaps;
  close(): Promise<void>;
}

export interface CallPorts {
  prime(): void;
  release(): void;
  openAudio(
    onFrame: (samples: Float32Array) => void,
    deviceId: string | null,
  ): Promise<AudioDevice>;
  mintSocketTicket(threadId: string): Promise<string>;
  openSocket(threadId: string, ticket: string, handlers: SocketHandlers): CallSocket;
  probeUpgrade(): Promise<boolean>;
}

const CUT_FADE_SECONDS = 0.015;

type AudioContextCtor = new (options?: AudioContextOptions) => AudioContext;

function audioContextCtor(): AudioContextCtor | null {
  if (typeof window === 'undefined') return null;
  const legacy = (window as { webkitAudioContext?: AudioContextCtor }).webkitAudioContext;
  return (window.AudioContext as AudioContextCtor | undefined) ?? legacy ?? null;
}

let primed: AudioContext | null = null;

let primedTaken = false;

function primeContext(): void {
  const Ctor = audioContextCtor();
  if (!Ctor) return;
  if (!primed || primedTaken || primed.state === 'closed') {
    primed = new Ctor({ sampleRate: SAMPLE_RATE_HZ });
    primedTaken = false;
  }
  void primed.resume().catch(() => undefined);
}

function releaseContext(): void {
  if (primedTaken || !primed) return;
  const context = primed;
  primed = null;
  void context.close().catch(() => undefined);
}

async function openAudio(
  onFrame: (samples: Float32Array) => void,
  deviceId: string | null,
): Promise<AudioDevice> {
  const Ctor = audioContextCtor();
  if (!Ctor) throw new CallSetupError(NO_WEB_AUDIO);
  if (!navigator.mediaDevices?.getUserMedia) throw new CallSetupError(NO_MICROPHONE_API);

  let context: AudioContext;
  if (primedTaken) {
    context = new Ctor({ sampleRate: SAMPLE_RATE_HZ });
  } else {
    if (!primed || primed.state === 'closed') primed = new Ctor({ sampleRate: SAMPLE_RATE_HZ });
    context = primed;
    primedTaken = true;
  }

  function abandonContext(): void {
    if (primed === context) {
      primedTaken = false;
      return;
    }
    void context.close().catch(() => undefined);
  }

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
    silence = context.createGain();
    silence.gain.value = 0;
    source.connect(capture);
    capture.connect(silence);
    silence.connect(context.destination);
    speaker = context.createGain();
    speaker.connect(context.destination);
  } catch (err) {
    stream.getTracks().forEach((track) => track.stop());
    abandonContext();
    throw new CallSetupError(microphoneRefusal(err));
  }

  let playback: Playback = PLAYBACK_START;
  const queued = new Set<AudioBufferSourceNode>();

  function stopPlayback(): void {
    const at = context.currentTime;
    const silent = at + CUT_FADE_SECONDS;
    speaker.gain.cancelScheduledValues(at);
    speaker.gain.setValueAtTime(speaker.gain.value, at);
    speaker.gain.linearRampToValueAtTime(0, silent);
    speaker.gain.setValueAtTime(1, silent);
    for (const node of queued) {
      try {
        node.stop(silent);
      } catch {
      }
    }
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
    isPlaying: () => queued.size > 0,
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
      if (primed === context) {
        primed = null;
        primedTaken = false;
      }
    },
  };
}

async function mintSocketTicket(threadId: string): Promise<string> {
  const gateway = await blockedRelayGateway();
  if (gateway) throw new CallSetupError(relayBlockedText(gateway));
  try {
    return await postSocketTicket(threadId);
  } catch (err) {
    if (err instanceof ApiError) throw new CallSetupError(err.reason);
    if (isTransportError(err)) throw new CallSetupError(NO_ROUTE_FOR_A_CALL);
    throw err;
  }
}

function openSocket(threadId: string, ticket: string, handlers: SocketHandlers): CallSocket {
  const socket = new WebSocket(voiceSocketUrl(threadId, ticket));
  socket.binaryType = 'arraybuffer';
  socket.onopen = () => handlers.onOpen();
  socket.onmessage = (event: MessageEvent) => {
    if (typeof event.data === 'string') handlers.onText(event.data);
    else handlers.onAudio(event.data as ArrayBuffer);
  };
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

export const browserPorts: CallPorts = {
  prime: primeContext,
  release: releaseContext,
  openAudio,
  mintSocketTicket,
  openSocket,
  probeUpgrade,
};
