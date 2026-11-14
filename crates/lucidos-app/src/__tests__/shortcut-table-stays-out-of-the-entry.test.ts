/**
 * The shortcut table ships in the shell chunk, never the entry. Only drawn UI
 * reads it: the keyboard handler, the tooltips, Settings, and the macOS View
 * menu's accelerators, which `startShell` installs. One static import of
 * `utils/shortcuts.ts` from anything `main.tsx` reaches puts it back on first
 * paint with `keybindings.ts` and the SDK tooltip, about 19 kB.
 *
 * This walks the static import graph from `main.tsx`, as Rollup does for the
 * entry chunk (`staticImportGraph.ts`).
 */
import { describe, it, expect } from 'vitest';
import { chainTo, srcPath, staticGraph } from './staticImportGraph';

describe('the entry chunk leaves the shortcut table to the shell', () => {
  const graph = staticGraph(srcPath('main.tsx'));

  it('reaches the store startup, so the walk itself works', () => {
    expect(graph.has(srcPath('store/startup.ts'))).toBe(true);
  });

  for (const module of ['utils/shortcuts.ts', 'store/actions/keybindings.ts', 'store/actions/view-menu.ts']) {
    it(`never reaches ${module} by a static import`, () => {
      const path = srcPath(module);
      expect(graph.has(path), graph.has(path) ? chainTo(graph, path) : '').toBe(false);
    });
  }
});
