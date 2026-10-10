/**
 * Text on an accent fill takes the theme's `--text-on-accent`, never a literal.
 *
 * The reported bug: a theme with a bright green accent set `--text-on-accent`
 * to black, but the needs-attention count stayed white on green and could not
 * be read. `.badge` and the drawer's attention pill both wrote `color: white`.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const srcDir: string = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const sheets = styleSheetPaths(srcDir)
  .map(path => [relative(srcDir, path), readFileSync(path, 'utf-8')] as const);

describe('ink on the accent fill', () => {
  it('every rule painting the accent inks its text with --text-on-accent', () => {
    const painted = sheets.flatMap(([file, css]) => cssRules(css)
      .filter(r => (r.props.get('background') ?? r.props.get('background-color')) === 'var(--accent)')
      .filter(r => r.props.has('color'))
      .map(r => ({ file, r })));
    expect(painted.length, 'the scan found no accent fills').toBeGreaterThan(0);
    for (const { file, r } of painted) {
      expect(r.props.get('color'), `${file}: ${r.selector}`).toBe('var(--text-on-accent)');
    }
  });

  it('a badge never inks with a literal, even where its fill comes from .badge', () => {
    // A badge that sets no background of its own takes its fill from `.badge`,
    // so the scan above cannot see it. Only the header bar's badges repaint
    // the fill.
    const allowed = ['var(--text-on-accent)', 'var(--header-badge-fg)'];
    const inked = sheets.flatMap(([file, css]) => rulesTargeting(css, 'badge')
      .filter(r => r.props.has('color'))
      .map(r => ({ file, r })));
    expect(inked.map(({ r }) => r.selector), 'the scan lost the base badge')
      .toContain('.badge');
    for (const { file, r } of inked) {
      expect(allowed, `${file}: ${r.selector}`).toContain(r.props.get('color'));
    }
  });
});
