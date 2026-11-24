import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readdirSync, readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, join, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesRoot: string = resolve(here, '..');

function filesUnder(dir: string, ext: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...filesUnder(path, ext));
    else if (entry.name.endsWith(ext)) out.push(path);
  }
  return out;
}

const sheets: Array<{ file: string; css: string }> = filesUnder(stylesRoot, '.css')
  .map((f: string) => ({ file: relative(stylesRoot, f), css: readFileSync(f, 'utf-8') }));

const GAP_PROPS = ['gap', 'column-gap', 'grid-column-gap'];

describe('the composer action row spends no gap of its own', () => {
  it('is styled somewhere, so the scan below has something to check', () => {
    const declaring = sheets.filter(s => rulesTargeting(s.css, 'prompt-actions-row').length > 0);
    expect(declaring.map(s => s.file)).toContain('chat/input-messages.css');
  });

  it('carries no gap in any host stylesheet, media queries included', () => {
    for (const { file, css } of sheets) {
      for (const rule of rulesTargeting(css, 'prompt-actions-row')) {
        for (const prop of GAP_PROPS) {
          expect(
            rule.props.get(prop),
            `${file}: "${rule.selector}" sets ${prop}. `
            + 'useFitsInOneRow charges gaps only inside .prompt-actions-right, '
            + 'so a gap here is width the check cannot see.',
          ).toBeUndefined();
        }
      }
    }
  });

  it('keeps the gap on the cluster the check does read', () => {
    const right = sheets
      .flatMap(s => rulesTargeting(s.css, 'prompt-actions-right'))
      .map(r => r.props.get('gap'))
      .filter(Boolean);
    expect(right).toContain('0.5rem');
  });
});
