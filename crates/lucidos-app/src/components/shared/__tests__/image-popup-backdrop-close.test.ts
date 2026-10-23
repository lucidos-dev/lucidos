import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ImagePopup.tsx'), 'utf-8');

// Background: the strip's `click` listener used to toggle chrome on every
// click — including clicks on the dark area outside the centered image.
// Users expect the dark backdrop area to close the popup (matching the
// modal-overlay's outer-padding behaviour). The strip handler must now
// distinguish a click on the <img> (toggle chrome) from a click on the
// surrounding slide background (close the popup).
describe('image popup — clicking outside the image closes it', () => {
  // Extract the body of the strip's click handler so we can assert on its
  // logic without false positives from elsewhere in the file.
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
    // The toggle must be guarded by an HTMLImageElement check; a bare
    // setChromeHidden call regresses to the old "any click toggles".
    expect(onClickBody).toMatch(/HTMLImageElement/);
    expect(onClickBody).toMatch(/setChromeHidden/);
  });

  it('closes the popup when the click landed on the slide background', () => {
    // Closing means clearing the popupImage signal — same as the close
    // button and the <Overlay> backdrop dismiss do.
    expect(onClickBody).toMatch(/popupImage\.value\s*=\s*null/);
  });
});
