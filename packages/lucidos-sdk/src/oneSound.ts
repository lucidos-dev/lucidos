/**
 * One sound at a time, across every app frame and the host.
 *
 * Each app frame has an opaque origin, so two frames cannot reach each other.
 * The host sees them all and relays: a frame that starts audible playback posts
 * {@link SOUND_STARTED_MESSAGE_TYPE}, and the host pushes {@link SOUND_PAUSE_CHANNEL}
 * to every other frame. The host half is
 * `crates/lucidos-app/src/store/actions/app-sound-bridge.ts`, which runs the
 * same two helpers below on its own document.
 *
 * Web Audio is out of scope, and so is a second element inside one frame: an
 * app may layer its own sounds (docs/plans/2026-10-10-one-sound-at-a-time.md).
 */

import { onHostPush } from './_bridge';

/** What a frame posts when one of its media elements starts making sound. */
export const SOUND_STARTED_MESSAGE_TYPE = 'lucidos:sound:started';

/** The push channel the host pauses a frame's media on. */
export const SOUND_PAUSE_CHANNEL = 'sound-pause';

/** What a frame posts on {@link SOUND_STARTED_MESSAGE_TYPE}. */
export interface SoundStarted {
  type: typeof SOUND_STARTED_MESSAGE_TYPE;
  startedAt: number;
}

/** What the host pushes on {@link SOUND_PAUSE_CHANNEL}: the latest start it
 *  has seen from that frame, or 0. A frame that has started a sound since
 *  ignores the pause, because the host will pause the others when its message
 *  lands. Without this, two frames starting at once pause each other. */
export interface SoundPause {
  seenStartedAt: number;
}

/** Epoch milliseconds, so a reloaded frame's starts still come after its old
 *  document's. The host keeps its last seen start per element. */
function soundClock(): number {
  return performance.timeOrigin + performance.now();
}

/** A muted or silent element is not a sound, so it neither pauses others nor
 *  is paused. A stream with no audio track, such as a camera preview, is
 *  silent however its element is set. */
export function isAudibleMedia(target: EventTarget | null): target is HTMLMediaElement {
  if (!(target instanceof HTMLMediaElement) || target.muted || target.volume === 0) return false;
  const stream = target.srcObject;
  return !(typeof MediaStream !== 'undefined' && stream instanceof MediaStream && stream.getAudioTracks().length === 0);
}

/** Pause every audible, playing media element in `doc`, except `keep`. */
export function pauseAudibleMedia(doc: Document, keep?: HTMLMediaElement): void {
  for (const media of doc.querySelectorAll<HTMLMediaElement>('audio, video')) {
    if (media !== keep && !media.paused && isAudibleMedia(media)) media.pause();
  }
}

/** Call `onStart` whenever a media element in `doc` starts making sound: it
 *  plays unmuted, or a playing element is unmuted. A volume step on a clip
 *  already sounding is no start. Both events skip bubbling, so the listener
 *  captures. */
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

/** Tell the host when this frame starts a sound, and pause on its push. */
export function installOneSound(): void {
  if (uninstall || window.parent === window) return;
  let lastStartedAt = 0;
  const unwatch = watchAudibleMediaStarts(document, () => {
    lastStartedAt = soundClock();
    const message: SoundStarted = { type: SOUND_STARTED_MESSAGE_TYPE, startedAt: lastStartedAt };
    window.parent.postMessage(message, '*');
  });
  const unlisten = onHostPush(SOUND_PAUSE_CHANNEL, (data) => {
    const seen = (data as Partial<SoundPause> | null)?.seenStartedAt;
    if (typeof seen === 'number' && seen < lastStartedAt) return;
    pauseAudibleMedia(document);
  });
  uninstall = () => { unwatch(); unlisten(); };
}

/** Test-only: undo the one-shot install, so a case can set up a fresh frame. */
export function _resetOneSoundForTesting(): void {
  uninstall?.();
  uninstall = null;
}
