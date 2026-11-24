// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';
import { chainTo, srcPath, staticGraph } from './staticImportGraph';

const ENGINE_MODULES = ['utils/markdownEngine.ts', 'utils/markedConfig.ts'];
const ENGINE_PACKAGES = /^\s*import\s+(?!type\s)[^'"]*?['"](marked|dompurify)['"]/m;

describe('the first-paint set leaves the markdown engine out', () => {
  const graphs = { entry: staticGraph(srcPath('main.tsx')), shell: staticGraph(srcPath('App.tsx')) };

  it('reaches the markdown facade from the entry, so the walk itself works', () => {
    expect(graphs.entry.has(srcPath('utils/renderMarkdown.ts'))).toBe(true);
  });

  for (const [name, graph] of Object.entries(graphs)) {
    for (const module of ENGINE_MODULES) {
      it(`never reaches ${module} from the ${name} by a static import`, () => {
        const path = srcPath(module);
        expect(graph.has(path), graph.has(path) ? chainTo(graph, path) : '').toBe(false);
      });
    }

    it(`never imports marked or DOMPurify from a module the ${name} reaches`, () => {
      const importers = [...graph.keys()].filter((file) => ENGINE_PACKAGES.test(readFileSync(file, 'utf8')));
      expect(importers.map((file) => chainTo(graph, file))).toEqual([]);
    });
  }
});
