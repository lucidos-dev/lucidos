/**
 * Start the shell chunk's download with the entry's, from `index.html`.
 *
 * The shell chunk is `<App/>`, which `main.tsx` imports dynamically so the data
 * layer can start its fetches first (ADR 0288). Vite preloads a dynamic
 * import's chunks only when the `import()` runs, which is after the whole entry
 * has downloaded and evaluated. A `modulepreload` link starts it at HTML parse
 * instead, so the two downloads run side by side.
 *
 * The shell chunk's own imports are preloaded too, except those the entry
 * already has, because a browser need not follow a preloaded module's imports.
 */

import type { Plugin } from 'vite';

const SHELL_MODULE = '/src/App.tsx';

type BundleChunk = { type: 'chunk'; fileName: string; moduleIds: string[]; imports: string[]; isEntry: boolean };
type BundleEntry = BundleChunk | { type: 'asset' };

export function shellPreloadHrefs(bundle: Record<string, BundleEntry>, base: string): string[] {
  const chunks = Object.values(bundle).filter((o): o is BundleChunk => o.type === 'chunk');
  // By membership, not `facadeModuleId`: Rollup leaves that null when the chunk
  // also exports shared code to the chunks that import it.
  const shell = chunks.find((c) => c.moduleIds.some((id) => id.endsWith(SHELL_MODULE)));
  if (!shell) {
    throw new Error(
      `No shell chunk for ${SHELL_MODULE}. main.tsx must import('./App') dynamically, `
      + 'or the entry chunk carries the whole UI again (ADR 0288).',
    );
  }
  const entry = chunks.find((c) => c.isEntry);
  const loadedByEntry = new Set([entry?.fileName, ...(entry?.imports ?? [])]);
  const files = [shell.fileName, ...shell.imports.filter((f) => !loadedByEntry.has(f))];
  return files.map((f) => `${base}${f}`);
}

export function shellChunkPreload(): Plugin {
  let base = './';
  return {
    name: 'lucidos-shell-chunk-preload',
    apply: 'build',
    configResolved(config) {
      base = config.base;
    },
    transformIndexHtml: {
      order: 'post',
      handler(_html, ctx) {
        if (!ctx.bundle) return;
        return shellPreloadHrefs(ctx.bundle as Record<string, BundleEntry>, base).map((href) => ({
          tag: 'link',
          attrs: { rel: 'modulepreload', crossorigin: true, href },
          injectTo: 'head',
        }));
      },
    },
  };
}
