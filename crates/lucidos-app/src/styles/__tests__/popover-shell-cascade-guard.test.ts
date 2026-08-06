/**
 * The prompt-bar popover shell must load BEFORE the surfaces that override it.
 *
 * `.prompt-bar-popover` and a surface class (`.event-wait-panel`,
 * `.todo-panel`) land on the SAME element, one via `panelClass`, at the same
 * specificity (one class each). Nothing but source order decides which
 * `max-width` wins. While the shell lived inside `todo-list.css` it therefore
 * beat every rule in `event-waits.css`, which `chat.css` imports first: the
 * subscription panel's `max-width: min(26rem, fit)` lost to the shell's bare
 * fit cap and the panel grew to the full viewport width, running out of the
 * thread pane and leaving its description on one unbroken line. `.todo-panel`
 * hid the bug, being declared after the shell inside one file.
 *
 * Neither `tsc` nor `vite build` can see this: the stylesheet is valid CSS and
 * builds clean. Only the rendered result is wrong, so the ordering gets a
 * source-scan tripwire instead.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
/** `crates/lucidos-app/src/styles/`, from `src/styles/__tests__/`. */
const STYLES = resolve(here, '..');
const SHELL = 'prompt-bar-popover.css';

/** Import specifiers in `chat.css`, in source order. */
function chatImports(): string[] {
  const css = readFileSync(join(STYLES, 'chat.css'), 'utf8');
  return [...css.matchAll(/@import\s+'\.\/chat\/([^']+)'/g)].map((m) => m[1]);
}

describe('prompt-bar popover shell cascade', () => {
  it('is imported before every stylesheet that overrides it', () => {
    const imports = chatImports();
    const shellAt = imports.indexOf(SHELL);
    expect(shellAt, `chat.css must import chat/${SHELL}`).toBeGreaterThanOrEqual(0);

    // Any chat stylesheet mentioning the shell class is a surface that may
    // override it, so it has to come later. Discovered from disk rather than
    // listed, so a third popover surface is covered the day it is added.
    const overriders = readdirSync(join(STYLES, 'chat'))
      .filter((f: string) => f.endsWith('.css') && f !== SHELL)
      .filter((f: string) => readFileSync(join(STYLES, 'chat', f), 'utf8').includes('prompt-bar-popover'));
    expect(overriders.length).toBeGreaterThan(0);
    for (const f of overriders) {
      const at = imports.indexOf(f);
      expect(at, `chat.css must import chat/${f}`).toBeGreaterThanOrEqual(0);
      expect(at, `chat/${f} must be imported after the shell`).toBeGreaterThan(shellAt);
    }
  });

  it('declares the shell rules in exactly one file', () => {
    const declaring = readdirSync(join(STYLES, 'chat'))
      .filter((f: string) => f.endsWith('.css'))
      .filter((f: string) =>
        /^\.prompt-bar-popover[\w-]*[\s,{]/m.test(readFileSync(join(STYLES, 'chat', f), 'utf8')),
      );
    expect(declaring).toEqual([SHELL]);
  });
});
