import { describe, it, expect, vi } from 'vitest';
import { navigateAppIframe } from './iframeNav';

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
