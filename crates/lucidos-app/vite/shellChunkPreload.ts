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
import { bundleChunks, findShellChunk, type BundleEntry } from './shellChunk';

export function shellPreloadHrefs(bundle: Record<string, BundleEntry>, base: string): string[] {
  const chunks = bundleChunks(bundle);
  const shell = findShellChunk(chunks);
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
