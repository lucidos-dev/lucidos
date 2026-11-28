import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ImagePopup.tsx'), 'utf-8');

describe('image popup — clicking outside the image closes it', () => {
  const onClickBody: string = (() => {
    const match = source.match(/function onClick\(e: MouseEvent\) \{([\s\S]*?)\n\s{4}\}/);
    if (!match) throw new Error('strip onClick handler not found');
    return match[1];
  })();

  it('inspects the click target to distinguish image from backdrop', () => {
    expect(onClickBody, 'onClick must read e.target to tell img from slide background')
      .toMatch(/e\.target/);
    expect(onClickBody, 'use instanceof to narrow the target type, not stringly-typed tagName')
      .not.toMatch(/tagName/);
  });

  it('toggles chrome only when the click landed on the <img>', () => {
    expect(onClickBody).toMatch(/HTMLImageElement/);
    expect(onClickBody).toMatch(/setChromeHidden/);
  });

  it('closes the popup when the click landed on the slide background', () => {
    expect(onClickBody).toMatch(/popupImage\.value\s*=\s*null/);
  });
});
