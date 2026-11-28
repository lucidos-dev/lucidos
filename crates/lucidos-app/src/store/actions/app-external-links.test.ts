import { afterEach, describe, expect, it, vi } from 'vitest';
import { signal } from '@preact/signals';
import type { ExternalLinkTarget } from './preferences';
import { EXTERNAL_LINKS_CHANNEL } from '../../../../../packages/lucidos-sdk/src/externalLinks';

const client = vi.hoisted(() => ({ tauri: false, iosPwa: false }));
const target = vi.hoisted(() => ({ current: null as null | { value: ExternalLinkTarget } }));
const frames = vi.hoisted(() => ({ list: [] as HTMLIFrameElement[] }));

vi.mock('../../utils/platform', () => ({
  isTauri: () => client.tauri,
  isIOSPwa: () => client.iosPwa,
}));
vi.mock('./preferences', () => ({
  currentExternalLinkTarget: () => target.current!.value,
}));
vi.mock('../../utils/appFrame', () => ({ mountedAppFrames: () => frames.list }));

target.current = signal<ExternalLinkTarget>('safari');

const { frameOpensExternalLinks, pushExternalLinksToFrame, installAppExternalLinksSync } =
  await import('./app-external-links');

function fakeFrame() {
  const postMessage = vi.fn();
  return { frame: { contentWindow: { postMessage } } as unknown as HTMLIFrameElement, postMessage };
}

afterEach(() => {
  client.tauri = false;
  client.iosPwa = false;
  target.current!.value = 'safari';
  frames.list = [];
});

describe('frameOpensExternalLinks', () => {
  it('is tab in a plain browser tab, whatever the stored target', () => {
    for (const t of ['safari', 'ask', 'in-app'] as const) {
      target.current!.value = t;
      expect(frameOpensExternalLinks()).toBe('tab');
    }
  });

  it('is host in the desktop app, whose web view drops window.open', () => {
    client.tauri = true;
    target.current!.value = 'in-app';
    expect(frameOpensExternalLinks()).toBe('host');
  });

  it('is safari in an iOS PWA in safari mode', () => {
    client.iosPwa = true;
    target.current!.value = 'safari';
    expect(frameOpensExternalLinks()).toBe('safari');
  });

  it('is host in an iOS PWA in ask mode, whose share sheet needs the host', () => {
    client.iosPwa = true;
    target.current!.value = 'ask';
    expect(frameOpensExternalLinks()).toBe('host');
  });

  it('is tab in an iOS PWA in in-app mode', () => {
    client.iosPwa = true;
    target.current!.value = 'in-app';
    expect(frameOpensExternalLinks()).toBe('tab');
  });
});

describe('pushExternalLinksToFrame', () => {
  it('hands the frame the answer on the external-links channel', () => {
    const { frame, postMessage } = fakeFrame();
    pushExternalLinksToFrame(frame);
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ channel: EXTERNAL_LINKS_CHANNEL, data: { frameOpens: 'tab' } }),
      '*',
    );
  });
});

describe('installAppExternalLinksSync', () => {
  it('re-pushes every mounted frame when the preference changes', () => {
    client.iosPwa = true;
    const { frame, postMessage } = fakeFrame();
    frames.list = [frame];
    const stop = installAppExternalLinksSync();
    expect(postMessage).toHaveBeenLastCalledWith(
      expect.objectContaining({ data: { frameOpens: 'safari' } }),
      '*',
    );

    target.current!.value = 'in-app';

    expect(postMessage).toHaveBeenLastCalledWith(
      expect.objectContaining({ channel: EXTERNAL_LINKS_CHANNEL, data: { frameOpens: 'tab' } }),
      '*',
    );
    stop();
  });
});
