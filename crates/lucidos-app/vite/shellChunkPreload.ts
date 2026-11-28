
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
