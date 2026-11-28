// @vitest-environment jsdom

import { describe, it, expect, beforeEach } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { renderMarkdown, renderMarkdownInline } from '../../utils/renderMarkdown';
import { inlineMarkdownImage, openImagePopupFromGroup, popupImage } from '../imagePopup';

const here: string = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf-8');

const DIAGRAM = 'https://example.test/diagram.png';
const CHART = 'https://example.test/chart.png';

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

  it('claims an image on a question card, in the question and in an option', () => {
    document.body.innerHTML = `
      <div class="question-body">
        <div class="question-text markdown-content">${renderMarkdown(`Which? ![All](${DIAGRAM})`)}</div>
        <button class="question-option">
          <span class="question-option-preview">${renderMarkdownInline(`![One](${CHART})`)}</span>
        </button>
      </div>`;
    const imgs = [...document.querySelectorAll<HTMLImageElement>('img')];
    expect(imgs).toHaveLength(2);
    for (const img of imgs) expect(inlineMarkdownImage(img)).toBe(img);
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
    const imgs = mount(
      'response-content markdown-content',
      renderMarkdown(`![Diagram](${DIAGRAM})\n\nSome prose.\n\n![Chart](${CHART})`),
    );
    expect(imgs).toHaveLength(2);

    openImagePopupFromGroup(imgs[0].src, imgs[0]);
    expect(popupImage.value).toEqual({ images: [DIAGRAM, CHART], index: 0 });
  });

  it('pages from a reply image to the question card pictures below it', () => {
    document.body.innerHTML = `
      <div class="thread-content">
        <div class="response-content markdown-content">${renderMarkdown(`![Diagram](${DIAGRAM})`)}</div>
        <div class="question-body">
          <button class="question-option">
            <span class="question-option-preview">${renderMarkdownInline(`![Chart](${CHART})`)}</span>
          </button>
        </div>
      </div>`;
    const card = document.querySelector<HTMLImageElement>('.question-body img')!;

    openImagePopupFromGroup(card.src, card);
    expect(popupImage.value).toEqual({ images: [DIAGRAM, CHART], index: 1 });
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

describe('the click and the cursor cover the same images', () => {
  const source = read('../imagePopup.ts');
  const selector = source.match(/const INLINE_MARKDOWN_IMAGE = '([^']+)'/)?.[1];

  it('names the inline image once, as a constant both readers share', () => {
    expect(selector, 'INLINE_MARKDOWN_IMAGE not declared in imagePopup.ts').toBeTruthy();
    expect(source.match(/INLINE_MARKDOWN_IMAGE/g)).toHaveLength(3);
  });

  it('cursors exactly that selector, in the host-only stylesheet', () => {
    const css = read('../../styles/global/host-components.css');
    expect(css).toContain(`${selector} {\n    cursor: zoom-in;`);
    expect(read('../../styles/global/shared-components.css')).not.toContain('zoom-in');
  });

  it('is delegated from the one global click handler', () => {
    const startup = read('../globalClick.ts');
    expect(startup).toContain('inlineMarkdownImage(target)');
    expect(startup).toContain('openImagePopupFromGroup(inlineImage.src, inlineImage)');
  });
});
