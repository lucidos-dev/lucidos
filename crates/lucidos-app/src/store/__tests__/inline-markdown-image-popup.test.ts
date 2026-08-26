// @vitest-environment jsdom
// The resolver reads a real DOM through `closest`, and `renderMarkdown` runs
// its output through DOMPurify, which needs one too.

/**
 * An `![alt](src)` image in a chat turn is a picture the reader wants to look
 * at. The transcript caps it at 24rem so a screenshot fits beside its prose.
 * The image popup is the only place it can be read at full size, zoomed.
 * Nothing connected the two: clicking an inline image did nothing at all,
 * while a generated image (`.image-thumbnail`) and a user attachment
 * (`.user-image-thumb`) both opened it.
 *
 * The resolver is delegated from the one global click handler rather than
 * bound per component, because every markdown surface emits the same wrapper:
 * a chat turn, a rendered `.md` preview, a notification body.
 */
import { describe, it, expect, beforeEach } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { inlineMarkdownImage, openImagePopupFromGroup, popupImage } from '../imagePopup';

const here: string = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf-8');

const DIAGRAM = 'https://example.test/diagram.png';
const CHART = 'https://example.test/chart.png';

/** Mount `html` under a container carrying `containerClass`, and hand back the
 *  images the markdown produced. */
function mount(containerClass: string, html: string): HTMLImageElement[] {
  document.body.innerHTML = `<div class="${containerClass}">${html}</div>`;
  return [...document.querySelectorAll('img')];
}

beforeEach(() => {
  popupImage.value = null;
  document.body.innerHTML = '';
});

describe('inlineMarkdownImage', () => {
  it('claims the image markdown renders inside its scroll wrapper', () => {
    const [img] = mount('markdown-content', renderMarkdown(`![Diagram](${DIAGRAM})`));
    expect(img, 'renderMarkdown produced no <img> to click').toBeTruthy();
    expect(img.closest('.image-scroll-wrapper'), 'the wrapper is the marker the resolver keys on').toBeTruthy();
    expect(inlineMarkdownImage(img)).toBe(img);
  });

  it('leaves a linked image to its link, which already says where the click goes', () => {
    const [img] = mount('markdown-content', renderMarkdown(`[![Diagram](${DIAGRAM})](https://example.test/page)`));
    expect(img).toBeTruthy();
    expect(inlineMarkdownImage(img)).toBeNull();
  });

  it('ignores an image outside a markdown surface', () => {
    document.body.innerHTML = '<img class="user-image-thumb" src="https://example.test/a.png">';
    expect(inlineMarkdownImage(document.querySelector('img'))).toBeNull();
  });

  it('ignores a target that is not an element', () => {
    expect(inlineMarkdownImage(null)).toBeNull();
  });
});

describe('inline images join the popup nav group', () => {
  it('collects every inline image in the transcript, opening at the clicked one', () => {
    document.body.innerHTML = `
      <div class="thread-content">
        <div class="response-content markdown-content">${renderMarkdown(`![Diagram](${DIAGRAM})`)}</div>
        <div class="response-content markdown-content">${renderMarkdown(`![Chart](${CHART})`)}</div>
      </div>`;
    const imgs = [...document.querySelectorAll<HTMLImageElement>('img')];
    expect(imgs).toHaveLength(2);

    openImagePopupFromGroup(imgs[1].src, imgs[1]);
    expect(popupImage.value).toEqual({ images: [DIAGRAM, CHART], index: 1 });
  });

  it('groups a rendered markdown document outside the thread pane', () => {
    // A `.md` file preview and a notification body have no `.thread-content`
    // around them, so the markdown block itself is the group.
    const imgs = mount(
      'response-content markdown-content',
      renderMarkdown(`![Diagram](${DIAGRAM})\n\nSome prose.\n\n![Chart](${CHART})`),
    );
    expect(imgs).toHaveLength(2);

    openImagePopupFromGroup(imgs[0].src, imgs[0]);
    expect(popupImage.value).toEqual({ images: [DIAGRAM, CHART], index: 0 });
  });

  it('mixes an inline image with the thumbnails already in the thread', () => {
    document.body.innerHTML = `
      <div class="thread-content">
        <img class="user-image-thumb" src="${CHART}">
        <div class="response-content markdown-content">${renderMarkdown(`![Diagram](${DIAGRAM})`)}</div>
      </div>`;
    const inline = document.querySelector<HTMLImageElement>('.image-scroll-wrapper > img')!;

    openImagePopupFromGroup(inline.src, inline);
    expect(popupImage.value).toEqual({ images: [CHART, DIAGRAM], index: 1 });
  });
});

/** The selector exists three times: the resolver, the group collector, and the
 *  `zoom-in` cursor. Drift between them advertises a click that never lands, or
 *  hides one that does. */
describe('the click and the cursor cover the same images', () => {
  const source = read('../imagePopup.ts');
  const selector = source.match(/const INLINE_MARKDOWN_IMAGE = '([^']+)'/)?.[1];

  it('names the inline image once, as a constant both readers share', () => {
    expect(selector, 'INLINE_MARKDOWN_IMAGE not declared in imagePopup.ts').toBeTruthy();
    expect(source.match(/INLINE_MARKDOWN_IMAGE/g)).toHaveLength(3);
  });

  it('cursors exactly that selector, in the host-only stylesheet', () => {
    // Host-only: an app iframe gets `.image-scroll-wrapper` from the shared
    // layer but has no popup, so a zoom cursor there would promise nothing.
    const css = read('../../styles/global/host-components.css');
    expect(css).toContain(`${selector} {\n    cursor: zoom-in;`);
    expect(read('../../styles/global/shared-components.css')).not.toContain('zoom-in');
  });

  it('is delegated from the one global click handler', () => {
    const startup = read('../../hooks/useStartup.ts');
    expect(startup).toContain('inlineMarkdownImage(target)');
    expect(startup).toContain('openImagePopupFromGroup(inlineImage.src, inlineImage)');
  });
});
