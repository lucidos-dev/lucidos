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

function containerRuleBody(): string {
  const m = mobileCss.match(/(^|\n)\s*\.mobile-swipe-container\s*\{([^}]*)\}/);
  if (!m) throw new Error('.mobile-swipe-container base rule not found');
  return m[2];
}

describe('.mobile-swipe-container compositor clip', () => {
  it('clips overflow with hidden, never clip', () => {
    const body = containerRuleBody();
    expect(body).toMatch(/overflow\s*:\s*hidden;/);
    expect(body).not.toMatch(/overflow\s*:\s*clip/);
  });

  it('forces its own compositor layer so the clip holds the will-change track', () => {
    expect(containerRuleBody()).toMatch(
      /transform\s*:\s*translate(Z|3d)|will-change\s*:[^;]*\btransform\b|contain\s*:[^;]*\bpaint\b/,
    );
  });
});
