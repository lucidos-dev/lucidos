import {
  SOUND_PAUSE_CHANNEL,
  SOUND_STARTED_MESSAGE_TYPE,
  pauseAudibleMedia,
  type SoundPause,
  watchAudibleMediaStarts,
} from '@lucidos/one-sound';
import { appFrameFor, mountedAppFrames } from '../../utils/appFrame';
import { postToAppFrame } from './app-bridge';

const seenStartedAt = new WeakMap<HTMLIFrameElement, number>();

function pauseAppFrames(keep: HTMLIFrameElement | null): void {
  for (const frame of mountedAppFrames()) {
    if (frame === keep) continue;
    const pause: SoundPause = { seenStartedAt: seenStartedAt.get(frame) ?? 0 };
    postToAppFrame(frame, SOUND_PAUSE_CHANNEL, pause);
  }
}

export function handleAppSoundMessage(event: MessageEvent): void {
  const data = event.data as { type?: unknown; startedAt?: unknown } | null;
  if (!data || typeof data !== 'object' || data.type !== SOUND_STARTED_MESSAGE_TYPE) return;
  const source = appFrameFor(event.source);
  if (!source) return;
  if (typeof data.startedAt === 'number' && Number.isFinite(data.startedAt)) {
    seenStartedAt.set(source, data.startedAt);
  }
  pauseAppFrames(source);
  pauseAudibleMedia(document);
}

export function watchShellSound(): () => void {
  return watchAudibleMediaStarts(document, (media) => {
    pauseAppFrames(null);
    pauseAudibleMedia(document, media);
  });
}
