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
  frontendPreview.value = { status: 'not-loaded' };
});

const PATH = '/~preview/0123456789abcdef/dev/';

describe('previewHref', () => {
  it('opens on the reached gateway origin, preserving its protocol and port', () => {
    for (const origin of ['http://localhost:5251', 'https://gateway.example:8443', 'https://[::1]:5252']) {
      expect(previewHref({ path: PATH }, origin)).toBe(`${origin}${PATH}`);
    }
  });

  it('lands on the preview thread without handing over a device id', () => {
    const href = previewHref({ path: PATH }, 'https://gateway.example', THREAD);
    expect(href).toBe(`https://gateway.example${PATH}#thread=${THREAD}`);
    expect(THREAD_HASH_RE.exec(new URL(href!).hash)?.[1]).toBe(THREAD);
  });

  it('refuses missing paths and paths outside the preview route', () => {
    for (const path of [undefined, '//attacker.example/', 'https://attacker.example/', '/dev/', PATH + '?device-id=x']) {
      expect(previewHref({ path }, 'https://gateway.example')).toBeNull();
    }
  });
});

describe('the SSE handlers', () => {
  it('a start records the thread and the path', () => {
    handleFrontendPreviewStarted({ thread_id: THREAD, path: PATH });
    expect(frontendPreview.value).toEqual({ status: 'loaded', data: { running: true, thread_id: THREAD, path: PATH } });
  });

  it('a stop for the running thread clears the slot', () => {
    handleFrontendPreviewStarted({ thread_id: THREAD, path: PATH });
    handleFrontendPreviewStopped({ thread_id: THREAD });
    expect(frontendPreview.value).toEqual({ status: 'loaded', data: { running: false } });
  });

  it('a late stop for the PREVIOUS thread does not erase the preview just moved', () => {
    handleFrontendPreviewStarted({ thread_id: THREAD, path: PATH });
    handleFrontendPreviewStopped({ thread_id: OTHER });
    expect(frontendPreview.value).toEqual({ status: 'loaded', data: { running: true, thread_id: THREAD, path: PATH } });
  });

  it('a stop with no thread named still clears, since it names no survivor', () => {
    handleFrontendPreviewStarted({ thread_id: THREAD, path: PATH });
    handleFrontendPreviewStopped({});
    expect(frontendPreview.value).toEqual({ status: 'loaded', data: { running: false } });
  });
});
