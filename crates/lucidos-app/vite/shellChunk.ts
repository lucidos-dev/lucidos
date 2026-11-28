
const SHELL_MODULE = '/src/App.tsx';

export type BundleChunk = { type: 'chunk'; fileName: string; moduleIds: string[]; imports: string[]; isEntry: boolean };
export type BundleEntry = BundleChunk | { type: 'asset' };

export function bundleChunks<C extends BundleChunk>(bundle: Record<string, C | { type: 'asset' }>): C[] {
  return Object.values(bundle).filter((o): o is C => o.type === 'chunk');
}

export function findShellChunk<C extends BundleChunk>(chunks: C[]): C {
  const shell = chunks.find((c) => c.moduleIds.some((id) => id.endsWith(SHELL_MODULE)));
  if (!shell) {
    throw new Error(
      `No shell chunk for ${SHELL_MODULE}. main.tsx must import('./App') dynamically, `
      + 'or the entry chunk carries the whole UI again (ADR 0288).',
    );
  }
  return shell;
}
