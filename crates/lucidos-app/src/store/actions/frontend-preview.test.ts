import { describe, it, expect, beforeEach } from 'vitest';
import {
  frontendPreview,
  previewHref,
  handleFrontendPreviewStarted,
  handleFrontendPreviewStopped,
} from './frontend-preview';
import { THREAD_HASH_RE } from './cross-workspace';

const THREAD = '2951200f-0652-4ee2-baa3-433d608983d8';
const OTHER = '11111111-1111-1111-1111-111111111111';

beforeEach(() => {
  frontendPreview.value = null;
});

describe('previewHref', () => {
  it('keeps the host the page is already on, so a phone gets its own name', () => {
    // The whole reason the page composes this rather than trusting the engine:
    // the engine's `url` reflects whichever request last hit the endpoint, and
    // that may have been the CLI on the host machine.
    expect(
      previewHref(6173, { protocol: 'https:', hostname: 'my-laptop.tailnet.ts.net' }),
    ).toBe('https://my-laptop.tailnet.ts.net:6173/');
    expect(previewHref(6173, { protocol: 'http:', hostname: 'localhost' })).toBe(
      'http://localhost:6173/',
    );
  });

  it('re-brackets an IPv6 hostname, which `location.hostname` reports bare', () => {
    expect(previewHref(6173, { protocol: 'https:', hostname: '::1' })).toBe(
      'https://[::1]:6173/',
    );
  });

  it('has no href without a port, rather than one pointing at the page itself', () => {
    expect(previewHref(undefined, { protocol: 'https:', hostname: 'localhost' })).toBeNull();
  });

  it('hands the device id over, since the preview origin has its own storage', () => {
    // Without it the preview registers as a NEW device and renders with none of
    // this one's device-scoped preferences (UI scale among them), which is the
    // one thing a surface for looking at UI must not get wrong.
    expect(
      previewHref(6173, { protocol: 'https:', hostname: 'localhost' }, 'd7c58d4e-7825-42ff-871a-7e4a0bc95c7d'),
    ).toBe('https://localhost:6173/?device-id=d7c58d4e-7825-42ff-871a-7e4a0bc95c7d');
  });

  it('omits the parameter when this page has no device id yet', () => {
    expect(previewHref(6173, { protocol: 'https:', hostname: 'localhost' }, null)).toBe(
      'https://localhost:6173/',
    );
  });

  it('lands on the thread the preview serves, via the `#thread=` channel', () => {
    // Own origin, own navigation state: without this the preview opens on the
    // empty compose view and the user has to find the thread again to see the
    // change they opened the preview to look at.
    expect(
      previewHref(6173, { protocol: 'https:', hostname: 'localhost' }, null, THREAD),
    ).toBe(`https://localhost:6173/#thread=${THREAD}`);
  });

  it('puts the device id in the query and the thread in the fragment, in that order', () => {
    // A fragment before the query string would swallow it: everything after the
    // `#` is the fragment, so `location.search` on the preview would be empty
    // and the device id would never be adopted.
    expect(
      previewHref(6173, { protocol: 'https:', hostname: 'localhost' }, 'd7c58d4e-7825-42ff-871a-7e4a0bc95c7d', THREAD),
    ).toBe(
      `https://localhost:6173/?device-id=d7c58d4e-7825-42ff-871a-7e4a0bc95c7d#thread=${THREAD}`,
    );
  });

  it('produces a fragment the landing channel actually matches', () => {
    // THREAD_HASH_RE is anchored at both ends, so an href that carried anything
    // else in the fragment would open the preview and silently go nowhere.
    const href = previewHref(6173, { protocol: 'https:', hostname: 'localhost' }, null, THREAD);
    expect(THREAD_HASH_RE.exec(new URL(href!).hash)?.[1]).toBe(THREAD);
  });
});

describe('the SSE handlers', () => {
  it('a start records the thread and the port', () => {
    handleFrontendPreviewStarted({ thread_id: THREAD, port: 6173 });
    expect(frontendPreview.value).toEqual({ running: true, thread_id: THREAD, port: 6173 });
  });

  it('a stop for the running thread clears the slot', () => {
    handleFrontendPreviewStarted({ thread_id: THREAD, port: 6173 });
    handleFrontendPreviewStopped({ thread_id: THREAD });
    expect(frontendPreview.value).toEqual({ running: false });
  });

  it('a late stop for the PREVIOUS thread does not erase the preview just moved', () => {
    // Moving the single slot emits a stop for the old thread and a start for
    // the new one, and SSE orders neither against the other. Applied blindly, a
    // stop arriving second would leave the UI saying nothing is running while a
    // Vite server is serving.
    handleFrontendPreviewStarted({ thread_id: THREAD, port: 6173 });
    handleFrontendPreviewStopped({ thread_id: OTHER });
    expect(frontendPreview.value).toEqual({ running: true, thread_id: THREAD, port: 6173 });
  });

  it('a stop with no thread named still clears, since it names no survivor', () => {
    handleFrontendPreviewStarted({ thread_id: THREAD, port: 6173 });
    handleFrontendPreviewStopped({});
    expect(frontendPreview.value).toEqual({ running: false });
  });
});
