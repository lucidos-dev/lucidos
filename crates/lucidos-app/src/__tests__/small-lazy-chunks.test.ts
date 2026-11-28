import { expect, it } from 'vitest';
import { assertSmallLazyChunks, smallLazyChunk, smallLazyChunks } from '../../vite/smallLazyChunks';
const chunk = (fileName: string, moduleIds: string[], imports: string[] = [], isEntry = false, bytes = 20000) => ({ type: 'chunk' as const, fileName, moduleIds, imports, isEntry, code: 'x'.repeat(bytes) });
const bundle = () => ({
  entry: chunk('entry.js', ['/app/src/main.tsx'], [], true),
  shell: chunk('shell.js', ['/app/src/App.tsx']),
  large: chunk('large.js', ['/app/src/large.ts']),
});
it('groups audited source modules independently of checkout path', () => {
  expect(smallLazyChunk('/another/crates/lucidos-app/src/components/shared/ConfirmAndPromptDialogs.tsx')).toBe('shell');
  expect(smallLazyChunk('/another/crates/lucidos-app/src/components/picker/PairingScanner.tsx')).toBe('shell');
  expect(smallLazyChunk('/another/crates/lucidos-app/src/components/files/FilePreviewInline.tsx')).toBeUndefined();
});
it('rejects shared deferred dependencies below the strict threshold', () => {
  const b = bundle();
  expect(() => assertSmallLazyChunks(b)).not.toThrow();
  b.large.code = 'x'.repeat(19999);
  expect(() => assertSmallLazyChunks(b)).toThrow(/20,000/);
});
it('rejects shell promotion into entry and heavy libraries into first paint', () => {
  const b = bundle();
  b.entry.imports.push('shell.js');
  expect(() => assertSmallLazyChunks(b)).toThrow(/entry/);
  b.entry.imports.length = 0;
  b.shell.imports.push('large.js');
  b.large.moduleIds.push('/app/node_modules/marked/lib/marked.esm.js');
  expect(() => assertSmallLazyChunks(b)).toThrow(/Heavy/);
});
it('requires every audited emitted module to reside with App', () => {
  const b = bundle();
  b.large.moduleIds.push('/app/src/components/shared/ConfirmAndPromptDialogs.tsx');
  expect(() => assertSmallLazyChunks(b)).toThrow(/shell/);
});

it('reports graph violations without freezing the development build-watch', () => {
  const invalid = bundle();
  invalid.large.code = 'tiny';
  for (const [devBuildWatch, watchMode] of [[true, false], [false, true], [false, false]]) {
    const plugin = smallLazyChunks(devBuildWatch);
    const hook = plugin.generateBundle as { handler: (this: unknown, options: unknown, output: unknown) => void };
    const warnings: string[] = [];
    const run = () => hook.handler.call({ meta: { watchMode }, warn: (message: string) => warnings.push(message) }, {}, invalid);
    if (devBuildWatch || watchMode) {
      expect(run).not.toThrow();
      expect(warnings[0]).toContain('20,000');
    } else expect(run).toThrow(/20,000/);
  }
});
