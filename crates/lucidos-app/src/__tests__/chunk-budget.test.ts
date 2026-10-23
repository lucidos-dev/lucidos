import { describe, it, expect, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import {
  BUDGET_LINE_PREFIX,
  DEV_BUILD_WATCH_ENV,
  ENTRY_BUDGET,
  FIRST_PAINT_BUDGET,
  FIRST_PAINT_REPORT_FILE,
  HEADROOM_SHARE,
  LAZY_CHUNK_LIMIT_KB,
  budgetLine,
  budgetVerdict,
  chunkBudget,
  firstPaintChunks,
  lazyChunkVerdict,
  underDevBuildWatch,
  type Budget,
} from '../../vite/chunkBudget';

/**
 * The chunk budgets (ADR 0436): the entry chunk and the first-paint set fail a
 * single-shot build, and a lazy chunk only warns. The module under test is
 * build config under `crates/lucidos-app/vite/`, tested from here because
 * Vitest's `include` covers `src/` only.
 */

/** A budget whose numbers the verdict tests can read at a glance. */
const SIX_HUNDRED: Budget = { name: 'entry', label: 'Entry chunk', kb: 600, fix: 'Move the next thing out.' };

describe('budgetVerdict', () => {
  it('stays silent up to the soft line, 90 % of the budget', () => {
    expect(budgetVerdict({ subject: 'assets/index-a.js', bytes: 480_000 }, SIX_HUNDRED, false))
      .toEqual({ kind: 'within' });
    expect(budgetVerdict({ subject: 'assets/index-a.js', bytes: 540_000 }, SIX_HUNDRED, false))
      .toEqual({ kind: 'within' });
  });

  it('warns past the soft line, naming the headroom left, without failing', () => {
    for (const reportOnly of [false, true]) {
      const verdict = budgetVerdict({ subject: 'assets/index-a.js', bytes: 571_500 }, SIX_HUNDRED, reportOnly);
      expect(verdict.kind).toBe('low-headroom');
      if (verdict.kind === 'within') return;
      expect(verdict.message).toContain('Entry chunk assets/index-a.js');
      expect(verdict.message).toContain('571.50 kB');
      expect(verdict.message).toContain('28.50 kB');
      expect(verdict.message).toContain('540.00 kB');
      expect(verdict.message).toContain(SIX_HUNDRED.fix);
    }
    expect(budgetVerdict({ subject: 'a.js', bytes: 540_001 }, SIX_HUNDRED, false).kind).toBe('low-headroom');
    expect(budgetVerdict({ subject: 'a.js', bytes: 600_000 }, SIX_HUNDRED, false).kind).toBe('low-headroom');
  });

  it('fails a single-shot build over budget, naming the budget, the size and where it lives', () => {
    const verdict = budgetVerdict({ subject: 'assets/index-a.js', bytes: 612_345 }, SIX_HUNDRED, false);
    expect(verdict.kind).toBe('fail');
    if (verdict.kind === 'within') return;
    expect(verdict.message).toContain('612.35 kB');
    expect(verdict.message).toContain('600 kB');
    expect(verdict.message).toContain('assets/index-a.js');
    expect(verdict.message).toContain('vite/chunkBudget.ts');
    expect(verdict.message).toContain('Do not raise the number');
  });

  it('only reports when asked to, so the build-watch still publishes', () => {
    const verdict = budgetVerdict({ subject: 'assets/index-a.js', bytes: 612_345 }, SIX_HUNDRED, true);
    expect(verdict.kind).toBe('report');
    if (verdict.kind === 'within') return;
    expect(verdict.message).toContain('612.35 kB');
    expect(verdict.message).toContain('ENTRY CHUNK OVER BUDGET');
  });

  it('names the first-paint set by its own label and fix', () => {
    const bytes = FIRST_PAINT_BUDGET.kb * 1000 + 1;
    const verdict = budgetVerdict({ subject: '(2 chunks: a.js, b.js)', bytes }, FIRST_PAINT_BUDGET, false);
    expect(verdict.kind).toBe('fail');
    if (verdict.kind === 'within') return;
    expect(verdict.message).toContain('First-paint set (2 chunks: a.js, b.js)');
    expect(verdict.message).toContain(FIRST_PAINT_BUDGET.fix);
  });
});

describe('lazyChunkVerdict', () => {
  it('stays silent up to the limit', () => {
    expect(lazyChunkVerdict({ fileName: 'assets/SettingsView-z.js', bytes: LAZY_CHUNK_LIMIT_KB * 1000 }))
      .toEqual({ kind: 'within' });
  });

  it('warns past the limit, naming the chunk, its size and the limit', () => {
    const verdict = lazyChunkVerdict({ fileName: 'assets/SettingsView-z.js', bytes: 412_345 });
    expect(verdict.kind).toBe('over');
    if (verdict.kind === 'within') return;
    expect(verdict.message).toContain('assets/SettingsView-z.js');
    expect(verdict.message).toContain('412.35 kB');
    expect(verdict.message).toContain(`${LAZY_CHUNK_LIMIT_KB} kB`);
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

describe('budgetLine', () => {
  it('carries the budget, the measurement and the budget in bytes, behind the prefix', () => {
    const line = budgetLine(ENTRY_BUDGET, { subject: 'assets/index-a.js', bytes: 512_000 });
    expect(line.startsWith(BUDGET_LINE_PREFIX)).toBe(true);
    expect(JSON.parse(line.slice(BUDGET_LINE_PREFIX.length))).toEqual({
      budget: ENTRY_BUDGET.name,
      label: ENTRY_BUDGET.label,
      subject: 'assets/index-a.js',
      bytes: 512_000,
      budgetBytes: ENTRY_BUDGET.kb * 1000,
    });
  });
});

type TestChunk = {
  type: 'chunk'; fileName: string; isEntry: boolean; moduleIds: string[]; imports: string[];
  dynamicImports: string[]; code: string;
};

const APP_MODULE = '/repo/crates/lucidos-app/src/App.tsx';

function chunk(fileName: string, size: number, over: Partial<TestChunk> = {}): TestChunk {
  return {
    type: 'chunk', fileName, isEntry: false, moduleIds: [], imports: [], dynamicImports: [],
    code: 'x'.repeat(size), ...over,
  };
}

function bundleOf(...chunks: TestChunk[]): Record<string, TestChunk> {
  return Object.fromEntries(chunks.map((c) => [c.fileName, c]));
}

describe('firstPaintChunks', () => {
  const bundle = bundleOf(
    chunk('assets/index-a.js', 10, { isEntry: true, imports: ['assets/vendor-v.js'], dynamicImports: ['assets/App-s.js'] }),
    chunk('assets/vendor-v.js', 10),
    chunk('assets/App-s.js', 10, {
      moduleIds: [APP_MODULE],
      imports: ['assets/index-a.js', 'assets/vendor-v.js', 'assets/marked-m.js'],
      dynamicImports: ['assets/SettingsView-z.js'],
    }),
    chunk('assets/marked-m.js', 10, { imports: ['assets/purify-p.js'] }),
    chunk('assets/purify-p.js', 10),
    chunk('assets/SettingsView-z.js', 10, { imports: ['assets/vendor-v.js'] }),
  );

  it('is the entry, the shell, and everything either reaches through static imports', () => {
    expect(firstPaintChunks(bundle).map((c) => c.fileName).sort()).toEqual([
      'assets/App-s.js', 'assets/index-a.js', 'assets/marked-m.js', 'assets/purify-p.js', 'assets/vendor-v.js',
    ]);
  });

  it('counts a chunk both of them import once', () => {
    const names = firstPaintChunks(bundle).map((c) => c.fileName);
    expect(names.filter((n) => n === 'assets/vendor-v.js')).toHaveLength(1);
  });

  it('leaves out a lazy chunk, even one that imports a first-paint chunk', () => {
    expect(firstPaintChunks(bundle).map((c) => c.fileName)).not.toContain('assets/SettingsView-z.js');
  });

  it('fails when there is no shell chunk, since the split has gone', () => {
    const { 'assets/App-s.js': _shell, ...unsplit } = bundle;
    expect(() => firstPaintChunks(unsplit)).toThrow(/shell chunk/);
  });
});

describe('chunkBudget plugin', () => {
  type Hooks = {
    generateBundle: {
      order: string;
      handler: (this: unknown, options: unknown, bundle: Record<string, unknown>) => void | Promise<void>;
    };
  };

  function run(bundle: Record<string, unknown>, watchMode: boolean, devBuildWatch = false) {
    const plugin = chunkBudget(devBuildWatch) as unknown as Hooks;
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

  /** The files a build within budget emits, once the handler has settled. */
  async function emittedBy(bundle: Record<string, unknown>, report: boolean) {
    const plugin = chunkBudget(false, report) as unknown as Hooks;
    const emitted: { fileName: string; source: string }[] = [];
    const ctx = {
      meta: { watchMode: false },
      error: (msg: string) => { throw new Error(msg); },
      warn: () => {},
      emitFile: (file: { fileName: string; source: string }) => { emitted.push(file); },
    };
    await plugin.generateBundle.handler.call(ctx, {}, bundle);
    return emitted;
  }

  /** An entry, a shell and one lazy chunk, sized in bytes. */
  function app(entry: number, shell: number, lazy = 1000, shared?: number) {
    return bundleOf(
      chunk('assets/index-a.js', entry, { isEntry: true, dynamicImports: ['assets/App-s.js'] }),
      chunk('assets/App-s.js', shell, {
        moduleIds: [APP_MODULE],
        imports: ['assets/index-a.js', ...(shared === undefined ? [] : ['assets/shared-x.js'])],
        dynamicImports: ['assets/SettingsView-z.js'],
      }),
      ...(shared === undefined ? [] : [chunk('assets/shared-x.js', shared)]),
      chunk('assets/SettingsView-z.js', lazy),
    );
  }

  const ENTRY = ENTRY_BUDGET.kb * 1000;
  const FIRST_PAINT = FIRST_PAINT_BUDGET.kb * 1000;
  const LAZY = LAZY_CHUNK_LIMIT_KB * 1000;

  it('runs after the other generateBundle hooks, so it measures what Vite prints', () => {
    expect((chunkBudget(false) as unknown as Hooks).generateBundle.order).toBe('post');
  });

  it('stays silent with every class within its number', () => {
    expect(run(app(ENTRY * 0.8, FIRST_PAINT * 0.3, LAZY * 0.5), false))
      .toEqual({ errors: [], warnings: [], threw: false });
  });

  it('never judges the shell against the lazy number', () => {
    // The shell alone is past the lazy limit, which is not its number.
    expect(run(app(ENTRY * 0.5, LAZY * 1.5), false)).toEqual({ errors: [], warnings: [], threw: false });
  });

  it('warns without failing when the first-paint set passes its soft line', () => {
    const result = run(app(ENTRY * 0.8, FIRST_PAINT * 0.95 - ENTRY * 0.8), false);
    expect(result.threw).toBe(false);
    expect(result.warnings).toHaveLength(1);
    expect(result.warnings[0]).toContain('First-paint set');
  });

  it('fails the build when the first-paint set is over budget, with the entry within its own', () => {
    const result = run(app(ENTRY * 0.8, FIRST_PAINT), false);
    expect(result.threw).toBe(true);
    expect(result.errors[0]).toContain('First-paint set');
  });

  it('counts a chunk the shell imports statically against the first-paint set', () => {
    // Splitting the shell into a statically imported piece moves no byte off
    // the first frame, so it must not get it under the budget.
    const shell = FIRST_PAINT * 0.5;
    const result = run(app(ENTRY * 0.8, shell, 1000, FIRST_PAINT - shell), false);
    expect(result.threw).toBe(true);
    expect(result.errors[0]).toContain('assets/shared-x.js');
  });

  it('fails the build when the entry chunk is over budget', () => {
    const result = run(app(ENTRY + 50_000, 1000), false);
    expect(result.threw).toBe(true);
    expect(result.errors[0]).toContain('Entry chunk');
    expect(result.errors[0]).toContain(`${((ENTRY + 50_000) / 1000).toFixed(2)} kB`);
  });

  it('only warns for a lazy chunk past its limit, and never counts it as first paint', () => {
    const result = run(app(ENTRY * 0.5, 1000, FIRST_PAINT), false);
    expect(result.threw).toBe(false);
    expect(result.warnings).toHaveLength(1);
    expect(result.warnings[0]).toContain('assets/SettingsView-z.js');
    expect(result.warnings[0]).not.toContain('First-paint set');
  });

  it('warns without failing under watch', () => {
    const result = run(app(ENTRY + 50_000, FIRST_PAINT), true);
    expect(result.threw).toBe(false);
    expect(result.warnings.join('\n')).toContain('ENTRY CHUNK OVER BUDGET');
    expect(result.warnings.join('\n')).toContain('FIRST-PAINT SET OVER BUDGET');
  });

  it('warns without failing under the dev build-watch, whose builds are one-shot', () => {
    const result = run(app(ENTRY * 0.8, FIRST_PAINT), false, true);
    expect(result.threw).toBe(false);
    expect(result.warnings[0]).toContain('FIRST-PAINT SET OVER BUDGET');
  });

  it('emits the first-paint report when asked, from the measurement the budget judges', async () => {
    const emitted = await emittedBy(app(500_000, 400_000, 1000, 60_000), true);
    expect(emitted.map((f) => f.fileName)).toEqual([FIRST_PAINT_REPORT_FILE]);
    const written = JSON.parse(emitted[0].source);
    expect(written.version).toBe(1);
    expect(written.entry).toEqual({ file: 'assets/index-a.js', bytes: 500_000 });
    expect(written.firstPaint.bytes).toBe(960_000);
    expect(written.firstPaint.budgetBytes).toBe(FIRST_PAINT_BUDGET.kb * 1000);
    expect(written.firstPaint.gzipBytes).toBeGreaterThan(0);
    expect(written.firstPaint.gzipBytes).toBeLessThan(written.firstPaint.bytes);
    expect(written.firstPaint.chunks.map((c: { file: string }) => c.file).sort())
      .toEqual(['assets/App-s.js', 'assets/index-a.js', 'assets/shared-x.js']);
  });

  it('emits nothing unless asked', async () => {
    expect(await emittedBy(app(500_000, 400_000), false)).toEqual([]);
  });

  it('prints one measurement line per budget, only under the dev build-watch', () => {
    const logged: string[] = [];
    const spy = vi.spyOn(console, 'log').mockImplementation((line: string) => { logged.push(line); });
    try {
      run(app(500_000, 400_000), false, false);
      expect(logged).toEqual([]);
      run(app(500_000, 400_000), false, true);
      expect(logged).toEqual([
        budgetLine(ENTRY_BUDGET, { subject: 'assets/index-a.js', bytes: 500_000 }),
        budgetLine(FIRST_PAINT_BUDGET, {
          subject: '(2 chunks: assets/index-a.js, assets/App-s.js)', bytes: 900_000,
        }),
      ]);
    } finally {
      spy.mockRestore();
    }
  });
});

describe('the docs that restate the numbers', () => {
  // Raw docs cannot import the constants, so this pins them (ADR 0368).
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
  const read = (path: string) => readFileSync(resolve(root, path), 'utf8');

  it('name the same budgets as vite/chunkBudget.ts', () => {
    const glossary = read('docs/glossary.md');
    expect(glossary).toContain(`\`ENTRY_BUDGET\` (${ENTRY_BUDGET.kb} kB)`);
    expect(glossary).toContain(`\`FIRST_PAINT_BUDGET\` (${FIRST_PAINT_BUDGET.kb} kB raw)`);
    expect(glossary).toContain(`\`LAZY_CHUNK_LIMIT_KB\` (${LAZY_CHUNK_LIMIT_KB} kB)`);
    const skill = read('.claude/skills/clean-build/SKILL.md');
    expect(skill).toContain(`| Entry chunk | ${ENTRY_BUDGET.kb} kB |`);
    expect(skill).toContain(`| First-paint set | ${FIRST_PAINT_BUDGET.kb} kB |`);
    expect(skill).toContain(`| Each lazy chunk | ${LAZY_CHUNK_LIMIT_KB} kB |`);
    expect(skill).toContain(`Past ${100 - HEADROOM_SHARE * 100} %`);
  });
});
