import { describe, expect, it } from 'vitest';
import { build } from 'esbuild';

const root = decodeURIComponent(new URL('../../../../../', import.meta.url).pathname);

describe('SDK model picker dependency boundary', () => {
  it('bundles the shared picker without host stores or runtime adapters', async () => {
    const result = await build({
      absWorkingDir: root,
      entryPoints: ['crates/lucidos-app/src/components/shared/ModelSelectionPicker.tsx',
        'crates/lucidos-app/src/components/shared/dropdownPresentation.tsx',
        'packages/lucidos-sdk/src/modelPicker.tsx'],
      bundle: true,
      write: false,
      outdir: 'unused',
      metafile: true,
      jsx: 'automatic',
      jsxImportSource: 'preact',
      alias: {
        '@lucidos/motion': './packages/lucidos-sdk/src/motion.ts',
        '@lucidos/appearance': './packages/lucidos-sdk/src/appearance.ts',
        '@lucidos/tap-press': './packages/lucidos-sdk/src/tapPress.ts',
        '@lucidos/geometry': './packages/lucidos-sdk/src/geometry.ts',
      },
    });
    const inputs = Object.keys(result.metafile!.inputs);
    expect(inputs.filter(path => /\/(store\/models|utils\/(viewport|tauri|motion)|hooks\/useHidePanelWebviewWhile)\.ts$/.test(path))).toEqual([]);
    expect(inputs.filter(path => path.includes('@tauri-apps'))).toEqual([]);
  });
});
