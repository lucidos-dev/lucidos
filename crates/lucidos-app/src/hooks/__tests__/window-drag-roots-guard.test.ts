/**
 * Every render root carries a window drag region.
 *
 * Under `titleBarStyle: "Overlay"` the webview owns the full window height, so
 * macOS hands it every press and the OS title bar the user would drag by is not
 * there to be grabbed. A surface that does not opt into `useWindowDragRegion`
 * (or `data-tauri-drag-region`, which our capability ACL denies) is a surface
 * whose window cannot be MOVED at all. That shipped: `main.tsx` mounts either
 * `<App/>` or `<WorkspacePicker/>`, and only the app had one, so a packaged
 * client sitting on the workspace picker was pinned where it launched.
 *
 * A source scan rather than a render test because these two are the app's whole
 * roots: `<App/>` mounts the entire shell and `<WorkspacePicker/>` fires a
 * workspace list, a gateway-status poll and a restore-status poll on mount, so
 * neither is unit-rendered anywhere in this suite. What needs pinning is one
 * fact per file, which reading the source states directly.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../..');

/** The components `main.tsx` renders as a root, keyed by source path. */
const RENDER_ROOTS = {
  '<App/>': 'App.tsx',
  '<WorkspacePicker/>': 'components/picker/WorkspacePicker.tsx',
} as const;

describe('window drag regions', () => {
  it('main.tsx renders exactly the roots this guard knows about', () => {
    // The list above is hand-maintained, so a third root added to the boot
    // switch must land here too rather than silently skipping the check.
    const main = readFileSync(resolve(SRC, 'main.tsx'), 'utf8');
    // Scoped to the `render(…)` arguments, so a component merely NAMED in a
    // comment elsewhere in the file is not read as a root.
    const call = /\brender\(([\s\S]*?)\);/.exec(main);
    expect(call, 'expected a render(…) call in main.tsx').not.toBeNull();
    const rendered = [...call![1].matchAll(/<(\w+)\s*\/>/g)].map((m) => m[1]);
    expect(new Set(rendered)).toEqual(new Set(['App', 'WorkspacePicker']));
  });

  for (const [root, file] of Object.entries(RENDER_ROOTS)) {
    it(`${root} makes some part of itself a window drag region`, () => {
      const src = readFileSync(resolve(SRC, file), 'utf8');
      expect(src, `${file} must import useWindowDragRegion`).toContain('useWindowDragRegion');
      // Imported and actually called: an unused import would satisfy a bare
      // substring check while leaving the window immovable.
      expect(src).toMatch(/useWindowDragRegion\(/);
    });
  }

  it('the picker keeps an open popover out of its drag region', () => {
    // The picker makes its WHOLE background the region, so the gate is the only
    // thing standing between the window moving and every other press on screen.
    // `<Overlay>` portals to document.body only when asked and none of the
    // picker's overlays ask, so a panel is a DOM descendant of that region:
    // without the exclusion, dragging to select the tailnet address out of the
    // Network access popover picks the window up instead.
    //
    // Asserted on the source because this suite has no jsdom (vitest runs in
    // node against hand-rolled stubs in test-setup.ts), so `closest` and
    // `instanceof Element` have nothing real to answer.
    const src = readFileSync(resolve(SRC, RENDER_ROOTS['<WorkspacePicker/>']), 'utf8');
    const gate = /const pickerCanDragStart[\s\S]*?;\n/.exec(src);
    expect(gate, 'expected a pickerCanDragStart gate').not.toBeNull();
    expect(gate![0]).toContain('isInteractiveTarget');
    expect(gate![0]).toContain('[data-overlay-panel]');
  });

  it('a conditionally rendered root does not hand the hook a plain useRef', () => {
    // The hook reads `ref.current` once, under deps that never change. The
    // picker's root is behind an early `return null` (the auto-open branch), so
    // a `useRef` would leave the effect holding that null and the picker it
    // later reveals would be immovable. See the hook's own doc comment.
    const src = readFileSync(resolve(SRC, RENDER_ROOTS['<WorkspacePicker/>']), 'utf8');
    const call = /useWindowDragRegion\(\s*(\w+)/.exec(src);
    expect(call, 'expected a useWindowDragRegion call in the picker').not.toBeNull();
    const refName = call![1];
    expect(src).toMatch(new RegExp(`const ${refName} = useMemo\\(`));
    expect(src).not.toMatch(new RegExp(`const ${refName} = useRef`));
  });
});
