import { describe, it, expect, beforeEach } from 'vitest';
import type { VNode } from 'preact';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { PreviewImage } from '../PreviewImage';
import { popupImage } from '../../../store/store';

const here: string = dirname(fileURLToPath(import.meta.url));
const inlineSource = readFileSync(resolve(here, '../FilePreviewInline.tsx'), 'utf-8');
const repoSource = readFileSync(resolve(here, '../RepoFilePreview.tsx'), 'utf-8');
const css = readFileSync(resolve(here, '../../../styles/panels/previews.css'), 'utf-8');

const SRC = '/data/artifacts/docs/architecture.png';

/** Hookless, so it can be called as a plain function and its vnode read
 *  (same approach as FilePreviewModal.test.ts). */
function image(): VNode<Record<string, () => unknown>> {
  return PreviewImage({ src: SRC, alt: 'artifacts/docs/architecture.png' }) as VNode<Record<string, () => unknown>>;
}

function keyEvent(key: string): { key: string; prevented: boolean; preventDefault(): void } {
  return { key, prevented: false, preventDefault() { this.prevented = true; } };
}

describe('a preview image opens the full-size popup', () => {
  beforeEach(() => { popupImage.value = null; });

  it('opens the popup on click, whatever the device', () => {
    (image().props.onClick as () => void)();
    expect(popupImage.value).toEqual({ images: [SRC], index: 0 });
  });

  it('opens on Enter and on Space, so the pane is reachable by keyboard', () => {
    for (const key of ['Enter', ' ']) {
      popupImage.value = null;
      const e = keyEvent(key);
      (image().props.onKeyDown as unknown as (e: unknown) => void)(e);
      expect(popupImage.value, `${key} did not open the popup`).toEqual({ images: [SRC], index: 0 });
      expect(e.prevented, `${key} must not also scroll or submit`).toBe(true);
    }
  });

  it('leaves every other key alone', () => {
    const e = keyEvent('a');
    (image().props.onKeyDown as unknown as (e: unknown) => void)(e);
    expect(popupImage.value).toBeNull();
    expect(e.prevented).toBe(false);
  });

  it('announces itself as the control it is', () => {
    const props = image().props as unknown as Record<string, unknown>;
    expect(props.role).toBe('button');
    expect(props.tabIndex).toBe(0);
    expect(props.class).toBe('preview-image');
  });
});

describe('every file preview renders its image through PreviewImage', () => {
  it('the workspace-data preview does', () => {
    expect(inlineSource).toMatch(/<PreviewImage\s/);
  });

  it('the repository preview does, for a binary image and for a rendered SVG', () => {
    expect(repoSource.match(/<PreviewImage\s/g) ?? []).toHaveLength(2);
  });

  it('neither keeps a bare clickable <img>, which would drop the popup', () => {
    for (const [name, source] of [['FilePreviewInline', inlineSource], ['RepoFilePreview', repoSource]] as const) {
      expect(source, `${name} still renders its own <img onClick=…>`).not.toMatch(/<img[^>]*onClick/);
    }
  });

  it('no device gate survives: the popup is how a desktop reader zooms too', () => {
    for (const [name, source] of [['FilePreviewInline', inlineSource], ['RepoFilePreview', repoSource]] as const) {
      expect(source, `${name} still calls isMobile()`).not.toMatch(/isMobile\(/);
    }
  });

  it('the CSS advertises the click', () => {
    const block = css.match(/\.preview-image\s*\{[^}]*\}/);
    expect(block, 'no .preview-image rule found').not.toBeNull();
    expect(block![0]).toMatch(/cursor:\s*zoom-in/);
  });
});
