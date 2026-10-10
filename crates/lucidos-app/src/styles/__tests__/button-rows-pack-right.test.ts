/**
 * A row of action buttons packs to the right, primary last, as the confirm
 * dialog does (`.claude/rules/frontend-css.md` § Action buttons). A flex row
 * whose class ends in `-actions`, `-buttons`, `-btns`, `-foot` or `-footer`
 * must place its buttons at the end: `justify-content`, or a `margin-left:
 * auto` that sends the row to the right edge. A row whose parent already
 * places it states that with its own `justify-content` and a comment.
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const src: string = resolve(here, '../..');

const ROW_CLASS = /\.[\w-]*-(?:actions|buttons|btns|foot|footer)$/;

/** The selector's subject compound, pseudo-classes stripped. */
function subject(selector: string): string {
  const last = selector.trim().split(/[\s>+~]+/).pop() ?? '';
  return last.replace(/:[\w-]+(\([^)]*\))?/g, '');
}

/** Every row selector with every declaration any sheet gives it, merged. */
function rowDeclarations(): Map<string, Map<string, string>> {
  const rows = new Map<string, Map<string, string>>();
  for (const path of styleSheetPaths(src)) {
    for (const rule of cssRules(readFileSync(path, 'utf8'))) {
      for (const one of rule.selector.split(',').map((s) => s.trim())) {
        if (!ROW_CLASS.test(subject(one))) continue;
        const merged = rows.get(one) ?? new Map<string, string>();
        rule.props.forEach((value, prop) => merged.set(prop, value));
        rows.set(one, merged);
      }
    }
  }
  return rows;
}

function packsLeft(props: Map<string, string>): boolean {
  if (!/^(inline-)?flex$/.test(props.get('display') ?? '')) return false;
  if ((props.get('flex-direction') ?? '').startsWith('column')) return false;
  if (props.has('justify-content')) return false;
  return props.get('margin-left') !== 'auto' && props.get('margin-inline-start') !== 'auto';
}

describe('button rows pack right', () => {
  it('has no left-packed row', () => {
    const leftPacked = [...rowDeclarations()].filter(([, props]) => packsLeft(props)).map(([sel]) => sel);
    expect(leftPacked.sort()).toEqual([]);
  });
});
