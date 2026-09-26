/**
 * A field that commits on blur and cancels on Escape must carry
 * `data-escape-self`, and its Escape branch must call `preventDefault`.
 *
 * The central Escape policy (`dispatchEscape` in `hooks/useKeyboardShortcuts.ts`)
 * runs in the capture phase and blurs a focused text input. The blur fires the
 * field's commit before its own Escape handler runs, so Escape saved the edit
 * it was meant to cancel. `data-escape-self` tells the policy to leave the key
 * to the field. The field then spends the key, or `InlineForm` reads it as
 * unspent and closes the form around it.
 *
 * A source scan over every `<input />` whose handlers are written inline.
 * A field with named handlers, such as `ThreadTitleEditor`, is not covered.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync, statSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const COMPONENTS_ROOT: string = resolve(here, '..');

function walkTsx(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const path = resolve(dir, entry);
    if (statSync(path).isDirectory()) out.push(...walkTsx(path));
    else if (path.endsWith('.tsx')) out.push(path);
  }
  return out;
}

/** Every self-closing `<input …/>` tag in `source`. */
function fieldTags(source: string): string[] {
  const tags: string[] = [];
  const opener = /<input\b/g;
  let match: RegExpExecArray | null;
  while ((match = opener.exec(source)) !== null) {
    const end = source.indexOf('/>', match.index);
    if (end < 0) break;
    tags.push(source.slice(match.index, end));
  }
  return tags;
}

/** Whether a tag commits on blur and handles Escape itself. A blur that
 *  cancels (`cancelEditingUrl`, `setEditingPath(false)`) is harmless. */
function commitsOnBlurAndCancelsOnEscape(tag: string): boolean {
  const blur = /\bonBlur=\{([^}]*)\}/.exec(tag);
  return blur !== null && /commit|save/i.test(blur[1]) && tag.includes("'Escape'");
}

describe('Escape on a blur-commit field', () => {
  it('finds the fields it guards', () => {
    const guarded = walkTsx(COMPONENTS_ROOT).flatMap((file) =>
      fieldTags(readFileSync(file, 'utf-8')).filter(commitsOnBlurAndCancelsOnEscape),
    );
    expect(guarded.length).toBeGreaterThanOrEqual(5);
  });

  it('leaves the key to the field, so the blur cannot commit first', () => {
    const offenders = walkTsx(COMPONENTS_ROOT).flatMap((file) =>
      fieldTags(readFileSync(file, 'utf-8'))
        .filter(commitsOnBlurAndCancelsOnEscape)
        .filter((tag) => !tag.includes('data-escape-self'))
        .map(() => relative(COMPONENTS_ROOT, file)),
    );
    expect(offenders).toEqual([]);
  });

  it('spends the Escape, so an open inline form stays open', () => {
    const offenders = walkTsx(COMPONENTS_ROOT).flatMap((file) =>
      fieldTags(readFileSync(file, 'utf-8'))
        .filter(commitsOnBlurAndCancelsOnEscape)
        .filter((tag) => !/'Escape'\)\s*\{\s*e\.preventDefault\(\)/.test(tag))
        .map(() => relative(COMPONENTS_ROOT, file)),
    );
    expect(offenders).toEqual([]);
  });
});
