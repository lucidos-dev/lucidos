import { describe, it, expect } from 'vitest';
import { entryChunkVerdict, entryChunkBudget } from '../../vite/entryChunkBudget';

/**
 * The entry chunk's first-paint budget guard. The module under test is build
 * config under `crates/lucidos-app/vite/`, tested from here because Vitest's
 * `include` covers `src/` only.
 */
describe('entryChunkVerdict', () => {
  it('passes a chunk at or under the budget', () => {
    expect(entryChunkVerdict({ fileName: 'assets/index-a.js', bytes: 480_000 }, 600, false))
      .toEqual({ kind: 'within' });
    expect(entryChunkVerdict({ fileName: 'assets/index-a.js', bytes: 600_000 }, 600, false))
      .toEqual({ kind: 'within' });
  });

  it('fails a single-shot build over budget, naming the budget and the measured size', () => {
    const verdict = entryChunkVerdict({ fileName: 'assets/index-a.js', bytes: 612_345 }, 600, false);
    expect(verdict.kind).toBe('fail');
    if (verdict.kind === 'within') return;
    expect(verdict.message).toContain('612.35 kB');
    expect(verdict.message).toContain('600 kB');
    expect(verdict.message).toContain('assets/index-a.js');
    expect(verdict.message).toContain('chunkSizeWarningLimit');
  });

  it('only reports in watch mode, so the build-watch still publishes', () => {
    const verdict = entryChunkVerdict({ fileName: 'assets/index-a.js', bytes: 612_345 }, 600, true);
    expect(verdict.kind).toBe('report');
    if (verdict.kind === 'within') return;
    expect(verdict.message).toContain('612.35 kB');
  });
});

describe('entryChunkBudget plugin', () => {
  type Hooks = {
    configResolved: (config: { build: { chunkSizeWarningLimit: number } }) => void;
    generateBundle: {
      order: string;
      handler: (this: unknown, options: unknown, bundle: Record<string, unknown>) => void;
    };
  };

  function run(bundle: Record<string, unknown>, watchMode: boolean) {
    const plugin = entryChunkBudget() as unknown as Hooks;
    plugin.configResolved({ build: { chunkSizeWarningLimit: 600 } });
    const errors: string[] = [];
    const warnings: string[] = [];
    const ctx = {
      meta: { watchMode },
      error: (msg: string) => { errors.push(msg); throw new Error(msg); },
      warn: (msg: string) => { warnings.push(msg); },
    };
    let threw = false;
    try {
      plugin.generateBundle.handler.call(ctx, {}, bundle);
    } catch {
      threw = true;
    }
    return { errors, warnings, threw };
  }

  const chunk = (fileName: string, isEntry: boolean, size: number) => ({
    type: 'chunk', fileName, isEntry, code: 'x'.repeat(size),
  });

  it('runs after the other generateBundle hooks, so it measures what Vite prints', () => {
    expect((entryChunkBudget() as unknown as Hooks).generateBundle.order).toBe('post');
  });

  it('measures only the entry chunk', () => {
    const result = run({
      'assets/index-a.js': chunk('assets/index-a.js', true, 500_000),
      'assets/SettingsView-b.js': chunk('assets/SettingsView-b.js', false, 700_000),
    }, false);
    expect(result).toEqual({ errors: [], warnings: [], threw: false });
  });

  it('fails the build when the entry chunk is over budget', () => {
    const result = run({ 'assets/index-a.js': chunk('assets/index-a.js', true, 650_000) }, false);
    expect(result.threw).toBe(true);
    expect(result.errors[0]).toContain('650.00 kB');
  });

  it('warns without failing under watch', () => {
    const result = run({ 'assets/index-a.js': chunk('assets/index-a.js', true, 650_000) }, true);
    expect(result.threw).toBe(false);
    expect(result.warnings[0]).toContain('650.00 kB');
  });
});
