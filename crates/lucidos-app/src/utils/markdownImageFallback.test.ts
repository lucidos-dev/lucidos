// @vitest-environment jsdom
// The handler is delegated off `document` and reads the DOM through `closest`.

/**
 * A markdown image that cannot load drew WebKit's broken-image box: an empty
 * frame with a `?` in it and no words. An agent had pasted
 * `![Uninstall panel after the cleanup](/tmp/plugin-mock.png)`, a path on its
 * own disk, and the reader saw a blank box with no hint of what was meant.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { installMarkdownImageFallback, IMAGE_UNAVAILABLE_CLASS } from './markdownImageFallback';

const SRC = '/tmp/plugin-mock.png';

let stop: () => void;

/** `wrapper` is the class of the image's parent. The markdown renderer wraps
 *  every image it writes in `.image-scroll-wrapper`. */
function mountImage(opts: { alt?: string; src?: string; wrapper?: string } = {}): HTMLImageElement {
  document.body.innerHTML = `<div class="markdown-content"><p><span class="${opts.wrapper ?? 'image-scroll-wrapper'}"></span></p></div>`;
  const img = document.createElement('img');
  img.setAttribute('src', opts.src ?? SRC);
  if (opts.alt !== undefined) img.setAttribute('alt', opts.alt);
  document.querySelector('p > span')!.append(img);
  return img;
}

/** jsdom never loads an image, so the browser's own events are simulated. They
 *  are dispatched without bubbling, exactly as a resource event arrives. */
function fail(img: HTMLImageElement): void {
  img.dispatchEvent(new Event('error'));
}

function succeed(img: HTMLImageElement): void {
  img.dispatchEvent(new Event('load'));
}

function notices(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>(`.${IMAGE_UNAVAILABLE_CLASS}`));
}

beforeEach(() => {
  document.body.innerHTML = '';
  stop = installMarkdownImageFallback();
});

afterEach(() => {
  stop();
});

describe('installMarkdownImageFallback', () => {
  it('replaces a failed image with a notice naming what it showed and where it pointed', () => {
    const img = mountImage({ alt: 'Uninstall panel after the cleanup' });
    fail(img);
    expect(img.hasAttribute('data-load-failed'), 'the broken box is hidden').toBe(true);
    const [notice] = notices();
    expect(notice.textContent).toContain('Image not available');
    expect(notice.textContent).toContain('Uninstall panel after the cleanup');
    expect(notice.textContent).toContain(SRC);
    expect(img.nextElementSibling, 'the notice takes the image\'s place').toBe(notice);
  });

  it('says only that the image is unavailable when it has no alt text', () => {
    fail(mountImage());
    expect(notices()[0].textContent).toBe(`Image not available ${SRC}`);
  });

  it('keeps one notice through repeated failures, since the retry re-requests', () => {
    const img = mountImage({ alt: 'shot' });
    fail(img);
    fail(img);
    fail(img);
    expect(notices()).toHaveLength(1);
  });

  it('brings the image back when a later retry loads it', () => {
    const img = mountImage({ alt: 'shot' });
    fail(img);
    succeed(img);
    expect(img.hasAttribute('data-load-failed')).toBe(false);
    expect(notices()).toHaveLength(0);
  });

  it('writes alt text as text, never as markup', () => {
    fail(mountImage({ alt: '<b>bold</b>' }));
    expect(notices()[0].querySelector('b')).toBeNull();
    expect(notices()[0].textContent).toContain('<b>bold</b>');
  });

  it('names no source for in-memory bytes, which would dump a base64 payload', () => {
    fail(mountImage({ alt: 'chart', src: 'data:image/png;base64,iVBORw0KGgo' }));
    expect(notices()[0].textContent).toBe('Image not available: chart');
    expect(notices()[0].querySelector('code')).toBeNull();
  });

  it('leaves an image the renderer did not write alone, since Preact owns its siblings', () => {
    // GeneratedImage renders a JSX `<img>` inside `.markdown-content`.
    const img = mountImage({ wrapper: 'generated-image' });
    fail(img);
    expect(img.hasAttribute('data-load-failed')).toBe(false);
    expect(notices()).toHaveLength(0);
  });

  it('stops answering once uninstalled', () => {
    stop();
    fail(mountImage());
    expect(notices()).toHaveLength(0);
    stop = installMarkdownImageFallback();
  });
});
