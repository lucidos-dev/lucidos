import { describe, it, expect } from 'vitest';
import { marked } from 'marked';
import postcss from 'postcss';
import '../../utils/markedConfig';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const RESPONSE_CSS = resolve(here, '../chat/response.css');

const PRESERVES_NEWLINES = /^(pre|pre-wrap|pre-line|break-spaces)$/;

describe('multiline copy block whitespace', () => {
  it('is handed markdown whose block structure already carries the line breaks', () => {
    const html = marked.parse(
      'Things people here could use it for:\n\n- one\n- two\n\nIt uses whatever\nyou already have.',
      { async: false },
    ) as string;

    expect(html).toContain('It uses whatever<br>you already have.');
    expect(html).toContain('</p>\n<ul>\n<li>');
    expect(html).toContain('</li>\n<li>');
  });

  it('is not styled with a newline-preserving white-space', () => {
    const css = readFileSync(RESPONSE_CSS, 'utf8');
    const offenders: string[] = [];
    postcss.parse(css, { from: RESPONSE_CSS }).walkRules((rule) => {
      if (!rule.selector.includes('.copyable-block')) return;
      rule.walkDecls('white-space', (decl) => {
        if (PRESERVES_NEWLINES.test(decl.value.trim())) {
          offenders.push(`${rule.selector} { white-space: ${decl.value.trim()} }`);
        }
      });
    });
    expect(
      offenders,
      'A copy block wraps rendered markdown, so preserving newlines renders the ones '
      + 'marked leaves between its block elements: a blank line between every paragraph '
      + 'and every list item. Let the block inherit `normal` and style the markdown.',
    ).toEqual([]);
  });
});
