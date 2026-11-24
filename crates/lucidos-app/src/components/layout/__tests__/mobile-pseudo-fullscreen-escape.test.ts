import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';


const here: string = dirname(fileURLToPath(import.meta.url));
const mobileCss = readFileSync(
  resolve(here, '../../../styles/mobile.css'),
  'utf-8',
);

function resetsTransform(className: string): boolean {
  const re = new RegExp(
    `:root\\[data-pseudo-fullscreen\\][^{}]*\\.${className}[^{}]*\\{[^}]*transform\\s*:\\s*none`,
  );
  return re.test(mobileCss);
}

describe('pseudo-fullscreen escape: containing-block ancestors reset', () => {
  it.each([
    'mobile-swipe-container',
    'mobile-swipe-track',
    'mobile-swipe-pane',
  ])('resets transform on .%s under data-pseudo-fullscreen', (className) => {
    expect(resetsTransform(className)).toBe(true);
  });
});
