import {
  SOUND_PAUSE_CHANNEL,
  SOUND_STARTED_MESSAGE_TYPE,
  pauseAudibleMedia,
  type SoundPause,
  watchAudibleMediaStarts,
} from '@lucidos/one-sound';
import { appFrameFor, mountedAppFrames } from '../../utils/appFrame';
import { postToAppFrame } from './app-bridge';

/** The latest start each frame has reported. Keyed to the element, so a
 *  frame that goes away takes its entry with it. */
const seenStartedAt = new WeakMap<HTMLIFrameElement, number>();

/** Pause the media in every mounted app frame but `keep`. */
function pauseAppFrames(keep: HTMLIFrameElement | null): void {
  for (const frame of mountedAppFrames()) {
    if (frame === keep) continue;
    const pause: SoundPause = { seenStartedAt: seenStartedAt.get(frame) ?? 0 };
    postToAppFrame(frame, SOUND_PAUSE_CHANNEL, pause);
  }
}

/** An app frame started a sound: every other frame and the shell pause. A
 *  message from any frame that is not a mounted app frame, such as a nested
 *  embed, is ignored. */
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

/** The shell's own player started a sound: every app frame and the shell's
 *  other players pause. Returns the teardown. */
export function watchShellSound(): () => void {
  return watchAudibleMediaStarts(document, (media) => {
    pauseAppFrames(null);
    pauseAudibleMedia(document, media);
  });
}
