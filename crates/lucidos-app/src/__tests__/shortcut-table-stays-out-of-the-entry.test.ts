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
