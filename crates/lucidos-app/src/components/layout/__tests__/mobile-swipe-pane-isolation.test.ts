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

function paneRuleBody(): string {
  const m = mobileCss.match(/(^|\n)\s*\.mobile-swipe-pane\s*\{([^}]*)\}/);
  if (!m) throw new Error('.mobile-swipe-pane base rule not found');
  return m[2];
}

describe('.mobile-swipe-pane visual isolation', () => {
  it('has an opaque background so adjacent panes cannot show through', () => {
    expect(paneRuleBody()).toMatch(/(^|\s|;)background(-color)?\s*:/);
  });

  it('establishes its own stacking context to contain z-indexes', () => {
    expect(paneRuleBody()).toMatch(
      /isolation\s*:\s*isolate|contain\s*:[^;]*\bpaint\b|transform\s*:\s*translateZ/,
    );
  });

  it('forces its own compositor layer so iframe descendants clip during swipes', () => {
    expect(paneRuleBody()).toMatch(
      /transform\s*:\s*translate(Z|3d)|contain\s*:[^;]*\bpaint\b/,
    );
  });
});
