import { describe, it, expect } from 'vitest';
import { shellPreloadHrefs } from '../../vite/shellChunkPreload';

/**
 * The shell chunk's `modulepreload` links (ADR 0288). The module under test is
 * build config under `crates/lucidos-app/vite/`, tested from here because
 * Vitest's `include` covers `src/` only.
 */
type Chunk = { type: 'chunk'; fileName: string; moduleIds: string[]; imports: string[]; isEntry: boolean };

const chunk = (fileName: string, over: Partial<Chunk> = {}): Chunk => ({
  type: 'chunk', fileName, moduleIds: [], imports: [], isEntry: false, ...over,
});

describe('shellPreloadHrefs', () => {
  const bundle = {
    'assets/index-a.js': chunk('assets/index-a.js', { isEntry: true, imports: ['assets/vendor-v.js'] }),
    'assets/App-s.js': chunk('assets/App-s.js', {
      moduleIds: ['/repo/crates/lucidos-app/src/components/layout/AppHeader.tsx', '/repo/crates/lucidos-app/src/App.tsx'],
      imports: ['assets/index-a.js', 'assets/vendor-v.js', 'assets/shared-x.js'],
    }),
    'assets/vendor-v.js': chunk('assets/vendor-v.js'),
    'assets/shared-x.js': chunk('assets/shared-x.js'),
    'assets/SettingsView-z.js': chunk('assets/SettingsView-z.js', {
      moduleIds: ['/repo/crates/lucidos-app/src/components/settings/SettingsView.tsx'],
    }),
  };

  it('preloads the shell chunk and the chunks it imports that the entry does not', () => {
    expect(shellPreloadHrefs(bundle, './')).toEqual(['./assets/App-s.js', './assets/shared-x.js']);
  });

  it('never preloads a lazy view', () => {
    expect(shellPreloadHrefs(bundle, './')).not.toContain('./assets/SettingsView-z.js');
  });

  it('fails the build when there is no shell chunk, since the split has gone', () => {
    const { 'assets/App-s.js': _shell, ...unsplit } = bundle;
    expect(() => shellPreloadHrefs(unsplit, './')).toThrow(/shell chunk/);
  });
});
