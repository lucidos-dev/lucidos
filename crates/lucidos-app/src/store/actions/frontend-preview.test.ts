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
    // The whole reason the page composes this rather than trusting the engine:
    // the engine's `url` reflects whichever request last hit the endpoint, and
    // that may have been the CLI on the host machine.
    expect(
      previewHref({ port: 6173 }, 'my-laptop.tailnet.ts.net'),
    ).toBe('https://my-laptop.tailnet.ts.net:6173/');
    expect(previewHref({ port: 6173 }, 'localhost')).toBe('https://localhost:6173/');
  });

  it('is always https, since the preview never serves plaintext', () => {
    // A page reached over plain HTTP on the LAN still links the TLS preview.
    expect(previewHref({ port: 6173 }, '192.168.1.10')).toBe('https://192.168.1.10:6173/');
  });

  it('links a loopback-only preview on this machine, and nowhere else', () => {
    // Without a user certificate Vite binds loopback, and its leaf names only
    // `localhost`, so another device gets no link rather than a dead one.
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
    // Without it the preview registers as a NEW device and renders with none of
    // this one's device-scoped preferences (UI scale among them), which is the
    // one thing a surface for looking at UI must not get wrong.
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
    // Own origin, own navigation state: without this the preview opens on the
    // empty compose view and the user has to find the thread again to see the
    // change they opened the preview to look at.
    expect(
      previewHref({ port: 6173 }, 'localhost', null, THREAD),
    ).toBe(`https://localhost:6173/#thread=${THREAD}`);
  });

  it('puts the device id in the query and the thread in the fragment, in that order', () => {
    // A fragment before the query string would swallow it: everything after the
    // `#` is the fragment, so `location.search` on the preview would be empty
    // and the device id would never be adopted.
    expect(
      previewHref({ port: 6173 }, 'localhost', 'd7c58d4e-7825-42ff-871a-7e4a0bc95c7d', THREAD),
    ).toBe(
      `https://localhost:6173/?device-id=d7c58d4e-7825-42ff-871a-7e4a0bc95c7d#thread=${THREAD}`,
    );
  });

  it('produces a fragment the landing channel actually matches', () => {
    // THREAD_HASH_RE is anchored at both ends, so an href that carried anything
    // else in the fragment would open the preview and silently go nowhere.
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
    // Moving the single slot emits a stop for the old thread and a start for
    // the new one, and SSE orders neither against the other. Applied blindly, a
    // stop arriving second would leave the UI saying nothing is running while a
    // Vite server is serving.
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
