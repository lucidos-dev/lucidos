// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { SOUND_PAUSE_CHANNEL, SOUND_STARTED_MESSAGE_TYPE } from '@lucidos/one-sound';

const appFrameFor = vi.fn();
const mountedAppFrames = vi.fn();
vi.mock('../../utils/appFrame', () => ({ appFrameFor, mountedAppFrames }));
const postToAppFrame = vi.fn();
vi.mock('./app-bridge', () => ({ postToAppFrame }));

const { handleAppSoundMessage, watchShellSound } = await import('./app-sound-bridge');

const message = (data: unknown, source: unknown) => ({ data, source }) as unknown as MessageEvent;

/** jsdom plays nothing, so `paused` is set by hand and `pause` is a spy. */
function shellAudio() {
  const el = document.createElement('audio');
  Object.defineProperty(el, 'paused', { configurable: true, value: false });
  const pause = vi.spyOn(el, 'pause').mockImplementation(() => {});
  document.body.appendChild(el);
  return { el, pause };
}

describe('app media bridge', () => {
  const playing = { contentWindow: { name: 'the playing frame' } };
  const other = { contentWindow: { name: 'another frame' } };

  beforeEach(() => {
    document.body.replaceChildren();
    postToAppFrame.mockReset();
    mountedAppFrames.mockReset().mockReturnValue([playing, other]);
    appFrameFor.mockReset().mockImplementation((s) => [playing, other].find((f) => f.contentWindow === s) ?? null);
  });

  it('pauses every other frame and the shell, never the frame that started', () => {
    const shell = shellAudio();
    handleAppSoundMessage(message({ type: SOUND_STARTED_MESSAGE_TYPE, startedAt: 1 }, playing.contentWindow));
    expect(postToAppFrame).toHaveBeenCalledTimes(1);
    expect(postToAppFrame).toHaveBeenCalledWith(other, SOUND_PAUSE_CHANNEL, { seenStartedAt: 0 });
    expect(shell.pause).toHaveBeenCalledOnce();
  });

  it('tells each frame the latest start it has seen from that frame', () => {
    handleAppSoundMessage(message({ type: SOUND_STARTED_MESSAGE_TYPE, startedAt: 7 }, playing.contentWindow));
    postToAppFrame.mockReset();
    handleAppSoundMessage(message({ type: SOUND_STARTED_MESSAGE_TYPE, startedAt: 9 }, other.contentWindow));
    expect(postToAppFrame).toHaveBeenCalledWith(playing, SOUND_PAUSE_CHANNEL, { seenStartedAt: 7 });
  });

  it('ignores a window that is not a mounted app frame', () => {
    const shell = shellAudio();
    handleAppSoundMessage(message({ type: SOUND_STARTED_MESSAGE_TYPE, startedAt: 1 }, { name: 'an embed' }));
    expect(postToAppFrame).not.toHaveBeenCalled();
    expect(shell.pause).not.toHaveBeenCalled();
  });

  describe('the shell starting a sound', () => {
    let unwatch: () => void;
    beforeEach(() => { unwatch = watchShellSound(); });
    afterEach(() => unwatch());

    it('pauses every frame and the shell\'s other players', () => {
      const started = shellAudio();
      const sibling = shellAudio();
      started.el.dispatchEvent(new Event('play'));
      expect(postToAppFrame).toHaveBeenCalledWith(playing, SOUND_PAUSE_CHANNEL, { seenStartedAt: expect.any(Number) });
      expect(postToAppFrame).toHaveBeenCalledWith(other, SOUND_PAUSE_CHANNEL, { seenStartedAt: expect.any(Number) });
      expect(started.pause).not.toHaveBeenCalled();
      expect(sibling.pause).toHaveBeenCalledOnce();
    });
  });
});
