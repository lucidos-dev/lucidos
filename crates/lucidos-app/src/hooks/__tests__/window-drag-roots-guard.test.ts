import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../..');

const RENDER_ROOTS = {
  '<App/>': 'App.tsx',
  '<WorkspacePicker/>': 'components/picker/WorkspacePicker.tsx',
} as const;

describe('window drag regions', () => {
  it('main.tsx renders exactly the roots this guard knows about', () => {
    const main = readFileSync(resolve(SRC, 'main.tsx'), 'utf8');
    const call = /\brender\(([\s\S]*?)\);/.exec(main);
    expect(call, 'expected a render(…) call in main.tsx').not.toBeNull();
    const rendered = [...call![1].matchAll(/<(\w+)\s*\/>/g)].map((m) => m[1]);
    expect(new Set(rendered)).toEqual(new Set(['App', 'WorkspacePicker']));
  });

  for (const [root, file] of Object.entries(RENDER_ROOTS)) {
    it(`${root} makes some part of itself a window drag region`, () => {
      const src = readFileSync(resolve(SRC, file), 'utf8');
      expect(src, `${file} must import useWindowDragRegion`).toContain('useWindowDragRegion');
      expect(src).toMatch(/useWindowDragRegion\(/);
    });
  }

  it('the picker keeps an open popover out of its drag region', () => {
    const src = readFileSync(resolve(SRC, RENDER_ROOTS['<WorkspacePicker/>']), 'utf8');
    const gate = /const pickerCanDragStart[\s\S]*?;\n/.exec(src);
    expect(gate, 'expected a pickerCanDragStart gate').not.toBeNull();
    expect(gate![0]).toContain('isInteractiveTarget');
    expect(gate![0]).toContain('[data-overlay-panel]');
  });

  it('a conditionally rendered root does not hand the hook a plain useRef', () => {
    const src = readFileSync(resolve(SRC, RENDER_ROOTS['<WorkspacePicker/>']), 'utf8');
    const call = /useWindowDragRegion\(\s*(\w+)/.exec(src);
    expect(call, 'expected a useWindowDragRegion call in the picker').not.toBeNull();
    const refName = call![1];
    expect(src).toMatch(new RegExp(`const ${refName} = useMemo\\(`));
    expect(src).not.toMatch(new RegExp(`const ${refName} = useRef`));
  });
});
