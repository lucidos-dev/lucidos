
import { onHostPush, postToHost } from './_bridge';

export const SOUND_STARTED_MESSAGE_TYPE = 'lucidos:sound:started';

export const SOUND_PAUSE_CHANNEL = 'sound-pause';

export interface SoundStarted {
  type: typeof SOUND_STARTED_MESSAGE_TYPE;
  startedAt: number;
}

export interface SoundPause {
  seenStartedAt: number;
}

function soundClock(): number {
  return performance.timeOrigin + performance.now();
}

export function isAudibleMedia(target: EventTarget | null): target is HTMLMediaElement {
  if (!(target instanceof HTMLMediaElement) || target.muted || target.volume === 0) return false;
  const stream = target.srcObject;
  return !(typeof MediaStream !== 'undefined' && stream instanceof MediaStream && stream.getAudioTracks().length === 0);
}

export function pauseAudibleMedia(doc: Document, keep?: HTMLMediaElement): void {
  for (const media of doc.querySelectorAll<HTMLMediaElement>('audio, video')) {
    if (media !== keep && !media.paused && isAudibleMedia(media)) media.pause();
  }
}

export function watchAudibleMediaStarts(doc: Document, onStart: (media: HTMLMediaElement) => void): () => void {
  const sounding = new WeakSet<HTMLMediaElement>();
  const onPlay = (event: Event) => {
    if (!isAudibleMedia(event.target)) return;
    sounding.add(event.target);
    onStart(event.target);
  };
  const onVolume = (event: Event) => {
    const media = event.target;
    if (!(media instanceof HTMLMediaElement)) return;
    if (!isAudibleMedia(media)) {
      sounding.delete(media);
    } else if (!media.paused && !sounding.has(media)) {
      sounding.add(media);
      onStart(media);
    }
  };
  doc.addEventListener('play', onPlay, true);
  doc.addEventListener('volumechange', onVolume, true);
  return () => {
    doc.removeEventListener('play', onPlay, true);
    doc.removeEventListener('volumechange', onVolume, true);
  };
}

let uninstall: (() => void) | null = null;

export function installOneSound(): void {
  if (uninstall || window.parent === window) return;
  let lastStartedAt = 0;
  const unwatch = watchAudibleMediaStarts(document, () => {
    lastStartedAt = soundClock();
    const message: SoundStarted = { type: SOUND_STARTED_MESSAGE_TYPE, startedAt: lastStartedAt };
    postToHost(message);
  });
  const unlisten = onHostPush(SOUND_PAUSE_CHANNEL, (data) => {
    const seen = (data as Partial<SoundPause> | null)?.seenStartedAt;
    if (typeof seen === 'number' && seen < lastStartedAt) return;
    pauseAudibleMedia(document);
  });
  uninstall = () => { unwatch(); unlisten(); };
}

export function _resetOneSoundForTesting(): void {
  uninstall?.();
  uninstall = null;
}
