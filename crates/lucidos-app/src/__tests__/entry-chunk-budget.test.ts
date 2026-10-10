import { describe, it, expect, vi } from 'vitest';
import {
  DEV_BUILD_WATCH_ENV,
  ENTRY_CHUNK_LINE_PREFIX,
  entryChunkBudget,
  entryChunkLine,
  entryChunkVerdict,
  underDevBuildWatch,
} from '../../vite/entryChunkBudget';

/**
 * The entry chunk's first-paint budget guard. The module under test is build
 * config under `crates/lucidos-app/vite/`, tested from here because Vitest's
 * `include` covers `src/` only.
 */
describe('entryChunkVerdict', () => {
  it('stays silent up to the soft line, 90 % of the budget', () => {
    expect(entryChunkVerdict({ fileName: 'assets/index-a.js', bytes: 480_000 }, 600, false))
      .toEqual({ kind: 'within' });
    expect(entryChunkVerdict({ fileName: 'assets/index-a.js', bytes: 540_000 }, 600, false))
      .toEqual({ kind: 'within' });
  });

  it('warns past the soft line, naming the headroom left, without failing', () => {
    for (const reportOnly of [false, true]) {
      const verdict = entryChunkVerdict({ fileName: 'assets/index-a.js', bytes: 571_500 }, 600, reportOnly);
      expect(verdict.kind).toBe('low-headroom');
      if (verdict.kind === 'within') return;
      expect(verdict.message).toContain('571.50 kB');
      expect(verdict.message).toContain('28.50 kB');
      expect(verdict.message).toContain('540.00 kB');
    }
    expect(entryChunkVerdict({ fileName: 'assets/index-a.js', bytes: 540_001 }, 600, false).kind)
      .toBe('low-headroom');
    expect(entryChunkVerdict({ fileName: 'assets/index-a.js', bytes: 600_000 }, 600, false).kind)
      .toBe('low-headroom');
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

  it('only reports when asked to, so the build-watch still publishes', () => {
    const verdict = entryChunkVerdict({ fileName: 'assets/index-a.js', bytes: 612_345 }, 600, true);
    expect(verdict.kind).toBe('report');
    if (verdict.kind === 'within') return;
    expect(verdict.message).toContain('612.35 kB');
    expect(verdict.message).toContain('OVER BUDGET');
  });
});

describe('underDevBuildWatch', () => {
  it('is true only when the variable names this process\'s parent', () => {
    expect(underDevBuildWatch({ [DEV_BUILD_WATCH_ENV]: '4242' }, 4242)).toBe(true);
  });

  it('stays strict for a variable leaked from another watcher or exported in a shell', () => {
    // The release, e2e and /harden builds run under `npx`, so their parent is
    // never the watcher that set the variable, even when they inherit it.
    expect(underDevBuildWatch({ [DEV_BUILD_WATCH_ENV]: '4242' }, 5151)).toBe(false);
    expect(underDevBuildWatch({ [DEV_BUILD_WATCH_ENV]: '1' }, 5151)).toBe(false);
    expect(underDevBuildWatch({ [DEV_BUILD_WATCH_ENV]: 'true' }, 5151)).toBe(false);
  });

  it('stays strict with the variable unset or empty', () => {
    expect(underDevBuildWatch({}, 4242)).toBe(false);
    expect(underDevBuildWatch({ [DEV_BUILD_WATCH_ENV]: '' }, 4242)).toBe(false);
  });
});

describe('entryChunkLine', () => {
  it('carries the measurement and the budget in bytes, behind the prefix', () => {
    const line = entryChunkLine({ fileName: 'assets/index-a.js', bytes: 512_000 }, 600);
    expect(line.startsWith(ENTRY_CHUNK_LINE_PREFIX)).toBe(true);
    expect(JSON.parse(line.slice(ENTRY_CHUNK_LINE_PREFIX.length)))
      .toEqual({ fileName: 'assets/index-a.js', bytes: 512_000, budgetBytes: 600_000 });
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

  function run(bundle: Record<string, unknown>, watchMode: boolean, devBuildWatch = false) {
    const plugin = entryChunkBudget(devBuildWatch) as unknown as Hooks;
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
    expect((entryChunkBudget(false) as unknown as Hooks).generateBundle.order).toBe('post');
  });

  it('measures only the entry chunk', () => {
    const result = run({
      'assets/index-a.js': chunk('assets/index-a.js', true, 500_000),
      'assets/SettingsView-b.js': chunk('assets/SettingsView-b.js', false, 700_000),
    }, false);
    expect(result).toEqual({ errors: [], warnings: [], threw: false });
  });

  it('warns without failing between the soft line and the budget', () => {
    const result = run({ 'assets/index-a.js': chunk('assets/index-a.js', true, 560_000) }, false);
    expect(result.threw).toBe(false);
    expect(result.errors).toEqual([]);
    expect(result.warnings[0]).toContain('560.00 kB');
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

  it('warns without failing under the dev build-watch, whose builds are one-shot', () => {
    const result = run({ 'assets/index-a.js': chunk('assets/index-a.js', true, 650_000) }, false, true);
    expect(result.threw).toBe(false);
    expect(result.warnings[0]).toContain('650.00 kB');
  });

  it('prints the measurement line only under the dev build-watch', () => {
    const logged: string[] = [];
    const spy = vi.spyOn(console, 'log').mockImplementation((line: string) => { logged.push(line); });
    try {
      run({ 'assets/index-a.js': chunk('assets/index-a.js', true, 500_000) }, false, false);
      expect(logged).toEqual([]);
      run({ 'assets/index-a.js': chunk('assets/index-a.js', true, 500_000) }, false, true);
      expect(logged).toEqual([entryChunkLine({ fileName: 'assets/index-a.js', bytes: 500_000 }, 600)]);
    } finally {
      spy.mockRestore();
    }
  });
});
