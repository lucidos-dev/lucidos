/**
 * The anchored popover shell must load BEFORE the surfaces that override it.
 *
 * `.anchored-popover` and a surface class (`.waiting-panel`, `.todo-panel`,
 * `.explainer-popover`) land on the SAME element at the same specificity. So
 * only source order decides which `max-width` wins. A shell loaded after a
 * surface beat its caps: the waiting panel once grew to the full viewport
 * width that way, running out of the thread pane.
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
const SRC = resolve(STYLES, '..');
const SHELL = 'anchored-popover.css';

/** Import specifiers of a stylesheet, in source order. */
function imports(file: string): string[] {
  const css = readFileSync(join(STYLES, file), 'utf8');
  return [...css.matchAll(/@import\s+'\.\/([^']+)'/g)].map((m) => m[1]);
}

/** Every stylesheet under `styles/`, as a path relative to it. */
function sheets(dir = STYLES, prefix = ''): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e: { name: string; isDirectory(): boolean }) =>
    e.isDirectory()
      ? e.name === '__tests__' ? [] : sheets(join(dir, e.name), `${prefix}${e.name}/`)
      : e.name.endsWith('.css') ? [`${prefix}${e.name}`] : []);
}

describe('anchored popover shell cascade', () => {
  it('loads before the explainer, in global.css', () => {
    const order = imports('global.css');
    const shellAt = order.indexOf(`global/${SHELL}`);
    expect(shellAt, `global.css must import global/${SHELL}`).toBeGreaterThanOrEqual(0);
    expect(order.indexOf('global/host-components.css')).toBeGreaterThan(shellAt);
  });

  it('loads before the chat surfaces, since main.tsx imports global.css first', () => {
    const main = readFileSync(join(SRC, 'main.tsx'), 'utf8');
    expect(main.indexOf("'./styles/global.css'")).toBeGreaterThanOrEqual(0);
    expect(main.indexOf("'./styles/global.css'")).toBeLessThan(main.indexOf("'./styles/chat.css'"));
    for (const f of ['chat/waiting-indicator.css', 'chat/todo-list.css']) {
      expect(imports('chat.css'), `chat.css must import ${f}`).toContain(f);
    }
  });

  it('declares the shell rules in exactly one file', () => {
    const declaring = sheets().filter((f) =>
      /^\.anchored-popover[\w-]*[\s,{]/m.test(readFileSync(join(STYLES, f), 'utf8')));
    expect(declaring).toEqual([`global/${SHELL}`]);
  });
});
