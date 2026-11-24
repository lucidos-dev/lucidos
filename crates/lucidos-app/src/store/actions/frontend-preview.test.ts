import { describe, it, expect, beforeEach } from 'vitest';
import {
  frontendPreview,
  previewHref,
  previewIsElsewhere,
  handleFrontendPreviewStarted,
  handleFrontendPreviewStopped,
} from './frontend-preview';
import { THREAD_HASH_RE } from './cross-workspace';

const THREAD = '2951200f-0652-4ee2-baa3-433d608983d8';
const OTHER = '11111111-1111-1111-1111-111111111111';

beforeEach(() => {
  frontendPreview.value = { status: 'not-loaded' };
});

describe('previewHref', () => {
  it('keeps the host the page is already on, so a phone gets its own name', () => {
    expect(
      previewHref({ port: 6173 }, 'my-laptop.tailnet.ts.net'),
    ).toBe('https://my-laptop.tailnet.ts.net:6173/');
    expect(previewHref({ port: 6173 }, 'localhost')).toBe('https://localhost:6173/');
  });

  it('is always https, since the preview never serves plaintext', () => {
    expect(previewHref({ port: 6173 }, '192.168.1.10')).toBe('https://192.168.1.10:6173/');
  });

  it('links a loopback-only preview on this machine, and nowhere else', () => {
    const local = { port: 6173, loopback_only: true };
    expect(previewHref(local, '127.0.0.1')).toBe('https://localhost:6173/');
    expect(previewHref(local, 'localhost')).toBe('https://localhost:6173/');
    expect(previewHref(local, 'my-laptop.tailnet.ts.net')).toBeNull();
    expect(previewIsElsewhere(local, 'my-laptop.tailnet.ts.net')).toBe(true);
    expect(previewIsElsewhere(local, 'localhost')).toBe(false);
    expect(previewIsElsewhere({ loopback_only: false }, 'my-laptop.tailnet.ts.net')).toBe(false);
  });

  it('re-brackets an IPv6 hostname, which `location.hostname` reports bare', () => {
    expect(previewHref({ port: 6173 }, '::1')).toBe(
      'https://[::1]:6173/',
    );
  });

  it('has no href without a port, rather than one pointing at the page itself', () => {
    expect(previewHref({}, 'localhost')).toBeNull();
  });

  it('hands the device id over, since the preview origin has its own storage', () => {
    expect(
      previewHref({ port: 6173 }, 'localhost', 'd7c58d4e-7825-42ff-871a-7e4a0bc95c7d'),
    ).toBe('https://localhost:6173/?device-id=d7c58d4e-7825-42ff-871a-7e4a0bc95c7d');
  });

  it('omits the parameter when this page has no device id yet', () => {
    expect(previewHref({ port: 6173 }, 'localhost', null)).toBe(
      'https://localhost:6173/',
    );
  });

  it('lands on the thread the preview serves, via the `#thread=` channel', () => {
    expect(
      previewHref({ port: 6173 }, 'localhost', null, THREAD),
    ).toBe(`https://localhost:6173/#thread=${THREAD}`);
  });

  it('puts the device id in the query and the thread in the fragment, in that order', () => {
    expect(
      previewHref({ port: 6173 }, 'localhost', 'd7c58d4e-7825-42ff-871a-7e4a0bc95c7d', THREAD),
    ).toBe(
      `https://localhost:6173/?device-id=d7c58d4e-7825-42ff-871a-7e4a0bc95c7d#thread=${THREAD}`,
    );
  });

  it('produces a fragment the landing channel actually matches', () => {
    const href = previewHref({ port: 6173 }, 'localhost', null, THREAD);
    expect(THREAD_HASH_RE.exec(new URL(href!).hash)?.[1]).toBe(THREAD);
  });
});

describe('the SSE handlers', () => {
  it('a start records the thread and the port', () => {
    handleFrontendPreviewStarted({ thread_id: THREAD, port: 6173 });
    expect(frontendPreview.value).toEqual({ status: 'loaded', data: { running: true, thread_id: THREAD, port: 6173 } });
  });

  it('a start carries whether only this machine can open the preview', () => {
    handleFrontendPreviewStarted({ thread_id: THREAD, port: 6173, loopback_only: true });
    const loaded = frontendPreview.value;
    expect(loaded.status === 'loaded' && loaded.data.loopback_only).toBe(true);
  });

  it('a stop for the running thread clears the slot', () => {
    handleFrontendPreviewStarted({ thread_id: THREAD, port: 6173 });
    handleFrontendPreviewStopped({ thread_id: THREAD });
    expect(frontendPreview.value).toEqual({ status: 'loaded', data: { running: false } });
  });

  it('a late stop for the PREVIOUS thread does not erase the preview just moved', () => {
    handleFrontendPreviewStarted({ thread_id: THREAD, port: 6173 });
    handleFrontendPreviewStopped({ thread_id: OTHER });
    expect(frontendPreview.value).toEqual({ status: 'loaded', data: { running: true, thread_id: THREAD, port: 6173 } });
  });

  it('a stop with no thread named still clears, since it names no survivor', () => {
    handleFrontendPreviewStarted({ thread_id: THREAD, port: 6173 });
    handleFrontendPreviewStopped({});
    expect(frontendPreview.value).toEqual({ status: 'loaded', data: { running: false } });
  });
});
