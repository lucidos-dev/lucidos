// @vitest-environment jsdom
/**
 * WebKit shares one in-flight fetch of a subresource between frames. It fails
 * that fetch for all of them when the frame that started it goes away. Every
 * app frame loads the SDK script and stylesheet, so a frame that leaves
 * mid-load can strand a sibling with neither. So the host reloads every frame
 * still loading when one leaves before its `load` (appFrameSharedLoads.ts).
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { AppFrame } from '../AppFrame';
import { APP_FRAME_READY_EVENT } from '../../../store/actions/app-ready-bridge';
import type { AppReveal } from '../../../store/types';

let hosts: HTMLElement[];

function mount(src: string, reveal: AppReveal = 'on-load'): HTMLElement {
  const host = document.createElement('div');
  document.body.appendChild(host);
  hosts.push(host);
  act(() => { render(<AppFrame src={src} reveal={reveal} />, host); });
  return host;
}

const frameIn = (host: HTMLElement) => host.querySelector('iframe')!;
const raisedCover = (host: HTMLElement) => host.querySelector('.app-ui-cover:not(.is-clearing)');
const fireLoad = (host: HTMLElement) => act(() => { frameIn(host).dispatchEvent(new Event('load')); });
const unmount = (host: HTMLElement) => act(() => { render(null, host); });

beforeEach(() => { hosts = []; });

afterEach(() => {
  for (const host of hosts) {
    render(null, host);
    host.remove();
  }
});

describe('a frame that leaves while loading', () => {
  it('reloads every sibling still loading, on a fresh element at the same url', () => {
    const leaving = mount('/app/sound/?a=1');
    const waiting = mount('/app/sound/?a=2');
    const before = frameIn(waiting);

    unmount(leaving);

    const after = frameIn(waiting);
    expect(after).not.toBe(before);
    expect(after.getAttribute('src')).toBe('/app/sound/?a=2');
  });

  it('leaves a sibling that already loaded alone', () => {
    const leaving = mount('/app/sound/?a=1');
    const loaded = mount('/app/sound/?a=2');
    fireLoad(loaded);
    const before = frameIn(loaded);

    unmount(leaving);

    expect(frameIn(loaded)).toBe(before);
  });

  it('reloads nobody when the frame that leaves had loaded', () => {
    const leaving = mount('/app/sound/?a=1');
    const waiting = mount('/app/sound/?a=2');
    fireLoad(leaving);
    const before = frameIn(waiting);

    unmount(leaving);

    expect(frameIn(waiting)).toBe(before);
  });

  it('covers the fresh element again, even when the old one had revealed', () => {
    const leaving = mount('/app/sound/?a=1');
    const ready = mount('/app/board/', 'on-ready');
    act(() => { frameIn(ready).dispatchEvent(new Event(APP_FRAME_READY_EVENT)); });
    expect(raisedCover(ready)).toBeNull();

    unmount(leaving);

    expect(raisedCover(ready)).not.toBeNull();
  });

  it('does not count a reload as leaving, so reloads never cascade', () => {
    const leaving = mount('/app/sound/?a=1');
    const first = mount('/app/sound/?a=2');
    const second = mount('/app/sound/?a=3');
    const firstBefore = frameIn(first);
    const secondBefore = frameIn(second);

    unmount(leaving);
    const firstReloaded = frameIn(first);
    const secondReloaded = frameIn(second);
    expect(firstReloaded).not.toBe(firstBefore);
    expect(secondReloaded).not.toBe(secondBefore);

    // Once a reloaded frame loads, a later leave reloads only the one still loading.
    fireLoad(first);
    const third = mount('/app/sound/?a=4');
    unmount(third);
    expect(frameIn(first)).toBe(firstReloaded);
    expect(frameIn(second)).not.toBe(secondReloaded);
  });
});
