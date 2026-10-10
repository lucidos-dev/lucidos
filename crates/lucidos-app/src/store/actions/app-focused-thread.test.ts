// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { installAppFocusedThreadSync } from './app-focused-thread';
import { setFocusedThread } from '../store';
import { BRIDGE_PUSH_TYPE, FOCUSED_THREAD_CHANNEL } from '../../../../../packages/lucidos-sdk/src/_bridge';

/** Mount an app frame the way `AppFrame` does, and record what it is sent. */
function mountFrame(): unknown[] {
  const frame = document.createElement('iframe');
  frame.setAttribute('data-role', 'app-ui-frame');
  document.body.appendChild(frame);
  const posted: unknown[] = [];
  vi.spyOn(frame.contentWindow as Window, 'postMessage').mockImplementation((m: unknown) => { posted.push(m); });
  return posted;
}

const pushOf = (threadId: string | null) =>
  ({ type: BRIDGE_PUSH_TYPE, channel: FOCUSED_THREAD_CHANNEL, data: { threadId } });

describe('installAppFocusedThreadSync', () => {
  let stop: () => void;

  beforeEach(() => {
    setFocusedThread('thread-a');
  });

  afterEach(() => {
    stop();
    document.body.replaceChildren();
    setFocusedThread(null);
  });

  it('pushes each change to every mounted app frame', () => {
    const canvas = mountFrame();
    const widgetWindow = mountFrame();
    stop = installAppFocusedThreadSync();

    setFocusedThread('thread-b');
    setFocusedThread(null);

    for (const posted of [canvas, widgetWindow]) {
      expect(posted).toEqual([pushOf('thread-a'), pushOf('thread-b'), pushOf(null)]);
    }
  });

  it('skips a frame that is not an app frame', () => {
    const other = document.createElement('iframe');
    document.body.appendChild(other);
    const spy = vi.spyOn(other.contentWindow as Window, 'postMessage');
    stop = installAppFocusedThreadSync();
    setFocusedThread('thread-b');
    expect(spy).not.toHaveBeenCalled();
  });

  it('pushes nothing after the teardown', () => {
    const posted = mountFrame();
    stop = installAppFocusedThreadSync();
    stop();
    setFocusedThread('thread-b');
    expect(posted).toEqual([pushOf('thread-a')]);
  });
});
