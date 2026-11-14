import { describe, it, expect } from 'vitest';
import { chainTo, srcPath, staticGraph } from './staticImportGraph';

describe('question answers stay in the shell chunk', () => {
  const entry = staticGraph(srcPath('main.tsx'));
  const answers = srcPath('store/actions/chat-claude-code.ts');

  it('keeps pending decisions available to the data layer', () => {
    expect(entry.has(srcPath('store/pendingDecisions.ts'))).toBe(true);
  });

  it('keeps rendering hooks out of the data layer', () => {
    const loading = srcPath('hooks/useDelayedLoading.ts');
    expect(entry.has(loading), entry.has(loading) ? chainTo(entry, loading) : '').toBe(false);
  });

  it('does not load answer actions before client startup', () => {
    expect(entry.has(answers), entry.has(answers) ? chainTo(entry, answers) : '').toBe(false);
  });

  it('loads answer actions with the shell before the first interaction', () => {
    expect(staticGraph(srcPath('App.tsx')).has(answers)).toBe(true);
    expect(staticGraph(srcPath('App.tsx')).has(srcPath('hooks/usePickMark.ts'))).toBe(true);
  });
});
