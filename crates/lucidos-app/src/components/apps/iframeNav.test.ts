import { describe, it, expect, vi } from 'vitest';
import { setAppFrameHash, splitFrameSrc } from './iframeNav';
import { APP_FRAME_ISOLATED } from './appFrameSandbox';


function stubFrame() {
  const posted: unknown[] = [];
  let srcMutated = false;
  const iframe = {
    contentWindow: { postMessage: vi.fn((msg: unknown) => { posted.push(msg); }) },
    get src() { return 'unused'; },
    set src(_v: string) { srcMutated = true; },
  } as unknown as HTMLIFrameElement;
  return { iframe, posted, srcMutated: () => srcMutated };
}

it('the app frame really is isolated, which is what the cases below assume', () => {
  expect(APP_FRAME_ISOLATED).toBe(true);
});

describe('setAppFrameHash', () => {
  it('asks the frame for the fragment, carrying it verbatim', () => {
    const frame = stubFrame();

    expect(setAppFrameHash(frame.iframe, 'report?tab=files')).toBe(true);

    expect(frame.posted).toEqual([
      { type: 'lucidos:bridge:host', op: 'hash', args: { fragment: 'report?tab=files' } },
    ]);
  });

  it('returns false without throwing when contentWindow is null', () => {
    const iframe = { contentWindow: null } as unknown as HTMLIFrameElement;

    expect(() => setAppFrameHash(iframe, 'x')).not.toThrow();
    expect(setAppFrameHash(iframe, 'x')).toBe(false);
  });
});

describe('splitFrameSrc', () => {
  it.each([
    ['/app/pr-understanding/', '/app/pr-understanding/', ''],
    ['/app/pr-understanding/#pr-1645', '/app/pr-understanding/', 'pr-1645'],
    ['/app/x/?thread_id=t7#frag', '/app/x/?thread_id=t7', 'frag'],
    ['/app/x/#', '/app/x/', ''],
  ])('splits %s', (src, doc, fragment) => {
    expect(splitFrameSrc(src)).toEqual({ doc, fragment });
  });

  it('tells a fragment-only change from a document change', () => {
    const before = splitFrameSrc('/app/x/#a');
    expect(splitFrameSrc('/app/x/#b').doc).toBe(before.doc);
    expect(splitFrameSrc('/app/y/#a').doc).not.toBe(before.doc);
    expect(splitFrameSrc('/app/x/?thread_id=t7#a').doc).not.toBe(before.doc);
  });
});
