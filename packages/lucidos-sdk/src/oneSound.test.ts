// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const pushHandlers = new Map<string, (data: unknown) => void>();
vi.mock('./_bridge', () => ({
  onHostPush: (channel: string, handler: (data: unknown) => void) => {
    pushHandlers.set(channel, handler);
    return () => pushHandlers.delete(channel);
  },
}));

import {
  SOUND_PAUSE_CHANNEL,
  SOUND_STARTED_MESSAGE_TYPE,
  _resetOneSoundForTesting,
  installOneSound,
  pauseAudibleMedia,
} from './oneSound';

/** jsdom plays nothing, so `paused` is set by hand and `pause` is a spy. */
function media(tag: 'audio' | 'video', { playing = false, muted = false } = {}) {
  const el = document.createElement(tag);
  el.muted = muted;
  Object.defineProperty(el, 'paused', { configurable: true, value: !playing });
  const pause = vi.spyOn(el, 'pause').mockImplementation(() => {});
  document.body.appendChild(el);
  return { el, pause };
}

/** A start message, whatever its clock reading. */
const started = { type: SOUND_STARTED_MESSAGE_TYPE, startedAt: expect.any(Number) };

describe('installOneSound', () => {
  const posted: unknown[] = [];
  const parent = { postMessage: (message: unknown) => posted.push(message) };

  beforeEach(() => {
    posted.length = 0;
    pushHandlers.clear();
    document.body.replaceChildren();
    _resetOneSoundForTesting();
    Object.defineProperty(window, 'parent', { configurable: true, value: parent });
    installOneSound();
  });

  afterEach(() => {
    Object.defineProperty(window, 'parent', { configurable: true, value: window });
  });

  it('tells the host when audible media starts', () => {
    const { el } = media('audio', { playing: true });
    el.dispatchEvent(new Event('play'));
    expect(posted).toEqual([started]);
  });

  it('tells the host when playing media is unmuted', () => {
    const { el } = media('video', { playing: true, muted: true });
    el.dispatchEvent(new Event('play'));
    el.muted = false;
    el.dispatchEvent(new Event('volumechange'));
    expect(posted).toEqual([started]);
  });

  it('says nothing for a volume step on a clip already sounding', () => {
    const { el } = media('audio', { playing: true });
    el.dispatchEvent(new Event('play'));
    el.volume = 0.5;
    el.dispatchEvent(new Event('volumechange'));
    expect(posted).toEqual([started]);
  });

  it('says nothing for a stream with no audio track', () => {
    class FakeStream { getAudioTracks() { return []; } }
    vi.stubGlobal('MediaStream', FakeStream);
    const { el } = media('video', { playing: true });
    Object.defineProperty(el, 'srcObject', { configurable: true, value: new FakeStream() });
    el.dispatchEvent(new Event('play'));
    vi.unstubAllGlobals();
    expect(posted).toEqual([]);
  });

  it('says nothing for muted media, or a volume change while paused', () => {
    media('video', { playing: true, muted: true }).el.dispatchEvent(new Event('play'));
    media('audio').el.dispatchEvent(new Event('volumechange'));
    expect(posted).toEqual([]);
  });

  it('ignores a pause sent before the host saw its latest start', () => {
    const { el, pause } = media('audio', { playing: true });
    el.dispatchEvent(new Event('play'));
    const { startedAt } = posted[0] as { startedAt: number };
    pushHandlers.get(SOUND_PAUSE_CHANNEL)?.({ seenStartedAt: 0 });
    expect(pause).not.toHaveBeenCalled();
    pushHandlers.get(SOUND_PAUSE_CHANNEL)?.({ seenStartedAt: startedAt });
    expect(pause).toHaveBeenCalledOnce();
  });

  it('pauses audible playing media on the host push, and leaves muted media alone', () => {
    const audible = media('audio', { playing: true });
    const muted = media('video', { playing: true, muted: true });
    pushHandlers.get(SOUND_PAUSE_CHANNEL)?.({ seenStartedAt: 0 });
    expect(audible.pause).toHaveBeenCalledOnce();
    expect(muted.pause).not.toHaveBeenCalled();
  });
});

describe('pauseAudibleMedia', () => {
  beforeEach(() => document.body.replaceChildren());

  it('keeps the element it is told to keep', () => {
    const kept = media('audio', { playing: true });
    const other = media('audio', { playing: true });
    pauseAudibleMedia(document, kept.el);
    expect(kept.pause).not.toHaveBeenCalled();
    expect(other.pause).toHaveBeenCalledOnce();
  });
});
