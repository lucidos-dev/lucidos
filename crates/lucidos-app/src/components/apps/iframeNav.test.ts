import { describe, it, expect, vi } from 'vitest';
import { navigateAppIframe, setAppFrameHash, splitFrameSrc } from './iframeNav';

const APP_DOC = 'https://host.example/ws/app/pr-understanding/';

/** A stub frame whose live URL can drift, the way a real app moves itself with
 *  `history.replaceState`. `replace` writes through, so a test can assert both
 *  where the frame ended up and that the document part never changed. */
function stubFrame(hash = '') {
  const location = {
    href: `${APP_DOC}${hash}`,
    replace: vi.fn((url: string) => { location.href = url; }),
  };
  return {
    location,
    replace: location.replace,
    iframe: { contentWindow: { location } } as unknown as HTMLIFrameElement,
  };
}

describe('navigateAppIframe', () => {
  it('uses contentWindow.location.replace() — never mutates iframe.src', () => {
    // Setting iframe.src adds an entry to the joint session history per HTML
    // spec; iOS Safari's edge-swipe-back gesture replays those entries and
    // restores prior app states under the user's swipe (WebKit #9166). Using
    // location.replace() updates the iframe URL without extending history.
    const replaceSpy = vi.fn();
    let srcMutated = false;
    const iframe = {
      contentWindow: { location: { replace: replaceSpy } },
      get src() {
        return 'unused';
      },
      set src(_v: string) {
        srcMutated = true;
      },
    } as unknown as HTMLIFrameElement;

    const ok = navigateAppIframe(iframe, '/app/notes-app/');

    expect(ok).toBe(true);
    expect(replaceSpy).toHaveBeenCalledTimes(1);
    expect(replaceSpy).toHaveBeenCalledWith('/app/notes-app/');
    expect(srcMutated).toBe(false);
  });

  it('returns false without throwing when contentWindow is null', () => {
    // A detached iframe (mid-unmount, removed between layout and effect
    // flushes) has contentWindow === null. The previous non-null assertion
    // would throw a TypeError on `.location` that AppUiInline never caught.
    const iframe = { contentWindow: null } as unknown as HTMLIFrameElement;

    expect(() => navigateAppIframe(iframe, '/x')).not.toThrow();
    expect(navigateAppIframe(iframe, '/x')).toBe(false);
  });
});

describe('setAppFrameHash', () => {
  it('moves the frame to the fragment without changing its document', () => {
    // Same document means the browser treats it as a fragment navigation: the
    // app is not reloaded and `hashchange` fires. It also means no `load`,
    // which is why the caller must raise no cover.
    const frame = stubFrame('');

    expect(setAppFrameHash(frame.iframe, 'pr-1645')).toBe(true);

    expect(frame.replace).toHaveBeenCalledWith(`${APP_DOC}#pr-1645`);
    expect(frame.location.href.split('#')[0]).toBe(APP_DOC);
  });

  it('replaces rather than pushes, so the joint history stays clean', () => {
    // A plain `location.hash = …` PUSHES an entry. That is the bug
    // `navigateAppIframe` exists to avoid: on an iOS PWA the edge-swipe-back
    // gesture replays those entries.
    const frame = stubFrame('');
    let pushed = false;
    Object.defineProperty(frame.location, 'hash', {
      get: () => '',
      set: () => { pushed = true; },
    });

    setAppFrameHash(frame.iframe, 'pr-1645');

    expect(pushed).toBe(false);
    expect(frame.replace).toHaveBeenCalledTimes(1);
  });

  it('moves a frame whose live hash has DRIFTED back onto the target', () => {
    // The same link clicked twice. PR Understanding reflects its selection back
    // with `history.replaceState`. By the second click the frame sits on a
    // different report than the one the link names.
    const frame = stubFrame('#pr-1700');

    expect(setAppFrameHash(frame.iframe, 'pr-1645')).toBe(true);

    expect(frame.location.href).toBe(`${APP_DOC}#pr-1645`);
  });

  it('keeps the frame query, so a WIP preview is not dropped', () => {
    // The WIP-preview thread rides in the query. Rebuilding the URL from the
    // frame's own href is what keeps it.
    const frame = stubFrame('');
    frame.location.href = `${APP_DOC}?thread_id=wip-7#pr-1700`;

    setAppFrameHash(frame.iframe, 'pr-1645');

    expect(frame.location.href).toBe(`${APP_DOC}?thread_id=wip-7#pr-1645`);
  });

  it('carries a `?` inside the fragment through to the frame', () => {
    // The `?` belongs to the fragment, so it must not land in the query.
    const frame = stubFrame('');

    setAppFrameHash(frame.iframe, 'report?tab=files');

    expect(frame.location.href).toBe(`${APP_DOC}#report?tab=files`);
  });

  it('does nothing when the frame is already on that fragment', () => {
    // Idempotence is what lets both delivery sites write without fighting.
    const frame = stubFrame('#pr-1645');

    expect(setAppFrameHash(frame.iframe, 'pr-1645')).toBe(false);
    expect(frame.replace).not.toHaveBeenCalled();
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
    // This is the test the frame's layout effect makes: same doc means hand
    // over the hash, a different doc means navigate and re-cover.
    const before = splitFrameSrc('/app/x/#a');
    expect(splitFrameSrc('/app/x/#b').doc).toBe(before.doc);
    expect(splitFrameSrc('/app/y/#a').doc).not.toBe(before.doc);
    expect(splitFrameSrc('/app/x/?thread_id=t7#a').doc).not.toBe(before.doc);
  });
});
