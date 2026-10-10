/**
 * A multiline `<copy>` block wraps RENDERED MARKDOWN, not preformatted text, so
 * its container must not preserve newlines.
 *
 * `postprocessCopyBlocks` (utils/renderMarkdown.ts) hands marked the raw
 * markdown between its comment markers and wraps whatever comes back, so the
 * `.copyable-block-multi` div's children are marked's block elements with
 * marked's own `\n` between them. Two consequences, and they point the same way:
 *
 *  - The line breaks are already carried by the block structure. The repo sets
 *    `breaks: true` globally (utils/markedConfig.ts), so even a SOFT line break
 *    inside a paragraph comes back as a `<br>`. A `white-space` that preserves
 *    newlines buys nothing.
 *  - It costs a blank line everywhere marked separates two blocks: one between
 *    consecutive paragraphs, one between every pair of `<li>`, and two before a
 *    list (the newline after `</p>` plus the one after `<ul>`). That shipped as
 *    `white-space: pre-wrap` and rendered a draft post with a visible gap
 *    between every bullet.
 *
 * So this asserts both halves: that marked really does leave those newlines in,
 * and that no rule reintroduces a `white-space` that would render them.
 */
import { describe, it, expect } from 'vitest';
import { marked } from 'marked';
import postcss from 'postcss';
// Side-effect import: this is what applies the app's `breaks: true` / `gfm`
// options, exactly as renderMarkdown depends on it.
import '../../utils/markedConfig';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const RESPONSE_CSS = resolve(here, '../chat/response.css');

/** Every `white-space` value that renders a source newline as a line break. */
const PRESERVES_NEWLINES = /^(pre|pre-wrap|pre-line|break-spaces)$/;

describe('multiline copy block whitespace', () => {
  it('is handed markdown whose block structure already carries the line breaks', () => {
    const html = marked.parse(
      'Things people here could use it for:\n\n- one\n- two\n\nIt uses whatever\nyou already have.',
      { async: false },
    ) as string;

    // A soft break is a <br>, so nothing depends on the newline surviving.
    expect(html).toContain('It uses whatever<br>you already have.');
    // And marked separates its blocks with newlines that a preserving
    // `white-space` would render as blank lines.
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
